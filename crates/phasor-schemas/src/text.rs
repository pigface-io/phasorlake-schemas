//! Names and numbers as AEMO writes them.

pub use phasor_mms_decode::normalize_name;

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
