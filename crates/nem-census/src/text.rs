//! Names and numbers as AEMO writes them.

/// Normalises a report or column name: strips a UTF-8 BOM, surrounding ASCII
/// whitespace, one pair of double quotes, and upper-cases ASCII letters.
///
/// Returns `None` for an empty result or one containing a control character
/// or a comma; those can never be real names and would corrupt a shape.
/// Writes into `buf`, which the caller reuses, so the hot path allocates once
/// per row at most.
pub fn normalize_name<'b>(raw: &str, buf: &'b mut String) -> Option<&'b str> {
    let s = raw
        .strip_prefix('\u{feff}')
        .unwrap_or(raw)
        .trim_matches([' ', '\t', '\r']);
    let s = match (s.strip_prefix('"'), s.len() >= 2) {
        (Some(inner), true) => inner.strip_suffix('"').unwrap_or(s),
        _ => s,
    }
    .trim_matches([' ', '\t']);
    if s.is_empty()
        || s.bytes()
            .any(|b| b.is_ascii_control() || b == b',' || b == b'"')
    {
        return None;
    }
    buf.clear();
    buf.push_str(s);
    buf.make_ascii_uppercase();
    Some(buf.as_str())
}

/// Parses an `f64`. AEMO writes plain decimals and occasionally exponent
/// forms such as `6E-05`; both are accepted. Leading `+` is accepted.
#[inline]
pub fn parse_f64(s: &[u8]) -> Option<f64> {
    std::str::from_utf8(s).ok()?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn norm(s: &str) -> Option<String> {
        let mut b = String::new();
        normalize_name(s, &mut b).map(str::to_owned)
    }

    #[test]
    fn normalises_names() {
        assert_eq!(norm("settlementdate").as_deref(), Some("SETTLEMENTDATE"));
        assert_eq!(norm(" \"DUID\" \r").as_deref(), Some("DUID"));
        assert_eq!(norm("\u{feff}REGIONID").as_deref(), Some("REGIONID"));
        assert_eq!(norm("\"\"").as_deref(), None);
        assert_eq!(norm(""), None);
        assert_eq!(norm("   "), None);
        assert_eq!(norm("A\tB"), None);
        assert_eq!(norm("A,B"), None);
        assert_eq!(norm("\""), None);
        assert_eq!(norm("héllo").as_deref(), Some("HéLLO"));
    }

    #[test]
    fn parses_numbers() {
        assert_eq!(parse_f64(b"9.199999"), Some(9.199_999));
        assert_eq!(parse_f64(b"6E-05"), Some(6e-5));
        assert_eq!(parse_f64(b"+1"), Some(1.0));
        assert_eq!(parse_f64(b"-0.5"), Some(-0.5));
        for bad in [&b""[..], b"x", b"1e", b" 1", b"1 ", b"\xff"] {
            assert_eq!(parse_f64(bad), None, "{bad:?}");
        }
    }
}
