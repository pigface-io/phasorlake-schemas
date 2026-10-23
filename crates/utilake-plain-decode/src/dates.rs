//! Every date AEMO writes, rewritten into the one form MMS files use.
//!
//! AEMO's feeds write a day as `2018/09/29`, `29/09/2018`, `29-09-2018`,
//! `29 Sep 2018` or `2018-09-29`, and an instant as any of those followed by
//! a time of day, or as RFC 3339 with the market's offset. The MMS form is
//! `YYYY/MM/DD` and `YYYY/MM/DD HH:MM:SS[.f]`. Rewriting a value into it
//! before anything reads it means one profiler, one parser and one lake
//! builder serve every feed, and a column never changes form within a
//! table.
//!
//! Every form is day-first where it is ambiguous: AEMO is Australian, and
//! the position of the four-digit year tells `DD/MM/YYYY` from
//! `YYYY/MM/DD`. An RFC 3339 instant is rewritten only when its offset is
//! the market's; another offset is left as text, so a foreign clock becomes
//! visible instead of silently shifted.

/// The parts of a date, before validation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Parts<'a> {
    year: u16,
    month: u8,
    day: u8,
    time: Option<Time<'a>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Time<'a> {
    hour: u8,
    minute: u8,
    second: u8,
    /// Digits after the seconds' decimal point, empty when none.
    fraction: &'a [u8],
}

/// Writes `v` into `out` in the MMS form when `v` is a date in one of the
/// forms AEMO writes, and says so. `out` is cleared first. `false` when `v`
/// is not a date, or is already in the MMS form: `out` is then empty and
/// `v` stands as it is. `offset` is the market's, as `+10:00`.
pub fn to_mms(v: &[u8], offset: &str, out: &mut Vec<u8>) -> bool {
    out.clear();
    let Some(parts) = parse(v, offset) else {
        return false;
    };
    write(parts, out);
    if out.as_slice() == v {
        out.clear();
        return false;
    }
    true
}

/// Whether `v` is a date in any form this module reads.
pub fn is_date(v: &[u8], offset: &str) -> bool {
    parse(v, offset).is_some()
}

fn write(p: Parts<'_>, out: &mut Vec<u8>) {
    push_digits(out, u32::from(p.year), 4);
    out.push(b'/');
    push_digits(out, u32::from(p.month), 2);
    out.push(b'/');
    push_digits(out, u32::from(p.day), 2);
    if let Some(t) = p.time {
        out.push(b' ');
        push_digits(out, u32::from(t.hour), 2);
        out.push(b':');
        push_digits(out, u32::from(t.minute), 2);
        out.push(b':');
        push_digits(out, u32::from(t.second), 2);
        if !t.fraction.is_empty() {
            out.push(b'.');
            out.extend_from_slice(t.fraction);
        }
    }
}

fn push_digits(out: &mut Vec<u8>, n: u32, width: usize) {
    let s = n.to_string();
    for _ in s.len()..width {
        out.push(b'0');
    }
    out.extend_from_slice(s.as_bytes());
}

fn parse<'a>(v: &'a [u8], offset: &str) -> Option<Parts<'a>> {
    let v = strip_weekday(trim(v));
    if v.len() < 8 {
        return None;
    }
    let p = if v.len() >= 10 && v[4] == b'/' && v[7] == b'/' {
        ymd(v, b'/')?
    } else if v.len() >= 10 && v[4] == b'-' && v[7] == b'-' {
        ymd(v, b'-')?
    } else if v.len() >= 10 && v[2] == b'/' && v[5] == b'/' {
        dmy(v, b'/')?
    } else if v.len() >= 10 && v[2] == b'-' && v[5] == b'-' {
        dmy(v, b'-')?
    } else {
        day_month_name(v)?
    };
    let (mut parts, rest) = p;
    parts.time = time_of(rest, offset)?;
    valid(parts).then_some(parts)
}

/// `v` without a leading day name (`Thursday 5 Aug 2021`), as the GBB's
/// forecast headers write their dates.
fn strip_weekday(v: &[u8]) -> &[u8] {
    const DAYS: [&[u8]; 7] = [
        b"monday",
        b"tuesday",
        b"wednesday",
        b"thursday",
        b"friday",
        b"saturday",
        b"sunday",
    ];
    let Some(sp) = v.iter().position(|b| *b == b' ') else {
        return v;
    };
    let word = v[..sp].to_ascii_lowercase();
    if DAYS.contains(&word.as_slice()) {
        trim(&v[sp + 1..])
    } else {
        v
    }
}

fn trim(v: &[u8]) -> &[u8] {
    let start = v.iter().position(|b| *b != b' ').unwrap_or(v.len());
    let end = v.iter().rposition(|b| *b != b' ').map_or(start, |i| i + 1);
    &v[start..end]
}

fn digits(v: &[u8]) -> Option<u32> {
    if v.is_empty() || !v.iter().all(u8::is_ascii_digit) {
        return None;
    }
    v.iter().try_fold(0u32, |n, b| {
        n.checked_mul(10)?.checked_add(u32::from(b - b'0'))
    })
}

/// `YYYY<sep>MM<sep>DD` and whatever follows.
fn ymd(v: &[u8], sep: u8) -> Option<(Parts<'_>, &[u8])> {
    debug_assert!(v[4] == sep && v[7] == sep);
    let year = digits(&v[0..4])?;
    let month = digits(&v[5..7])?;
    let day = digits(&v[8..10])?;
    Some((parts(year, month, day), &v[10..]))
}

/// `DD<sep>MM<sep>YYYY` and whatever follows.
fn dmy(v: &[u8], sep: u8) -> Option<(Parts<'_>, &[u8])> {
    debug_assert!(v[2] == sep && v[5] == sep);
    let day = digits(&v[0..2])?;
    let month = digits(&v[3..5])?;
    let year = digits(&v[6..10])?;
    Some((parts(year, month, day), &v[10..]))
}

/// `D Mon YYYY` or `DD Mon YYYY` and whatever follows.
fn day_month_name(v: &[u8]) -> Option<(Parts<'_>, &[u8])> {
    let sp = v.iter().position(|b| *b == b' ')?;
    if sp == 0 || sp > 2 {
        return None;
    }
    let day = digits(&v[..sp])?;
    let rest = &v[sp + 1..];
    if rest.len() < 8 || rest[3] != b' ' {
        return None;
    }
    let month = month_of(&rest[..3])?;
    let year = digits(&rest[4..8])?;
    Some((parts(year, month, day), &rest[8..]))
}

fn month_of(name: &[u8]) -> Option<u32> {
    const MONTHS: [&[u8; 3]; 12] = [
        b"jan", b"feb", b"mar", b"apr", b"may", b"jun", b"jul", b"aug", b"sep", b"oct", b"nov",
        b"dec",
    ];
    let lower: [u8; 3] = [
        name[0].to_ascii_lowercase(),
        name[1].to_ascii_lowercase(),
        name[2].to_ascii_lowercase(),
    ];
    MONTHS
        .iter()
        .position(|m| **m == lower)
        .map(|i| i as u32 + 1)
}

fn parts<'a>(year: u32, month: u32, day: u32) -> Parts<'a> {
    Parts {
        year: year.try_into().unwrap_or(u16::MAX),
        month: month.try_into().unwrap_or(u8::MAX),
        day: day.try_into().unwrap_or(u8::MAX),
        time: None,
    }
}

/// The time of day after a date: nothing, ` H:MM`, ` HH:MM:SS`,
/// ` HH:MM:SS.f`, `THH:MM:SS[.f]` with an optional zone that must be the
/// market's. `None` when the tail is none of these.
fn time_of<'a>(rest: &'a [u8], offset: &str) -> Option<Option<Time<'a>>> {
    let Some((&lead, tail)) = rest.split_first() else {
        return Some(None);
    };
    if lead != b' ' && lead != b'T' {
        return None;
    }
    let (clock, zone) = split_zone(tail);
    if lead == b' ' && !zone.is_empty() {
        return None;
    }
    if !zone.is_empty() && !zone_is(zone, offset) {
        return None;
    }
    let mut it = clock.split(|b| *b == b':');
    let hour = it
        .next()
        .and_then(|h| (h.len() == 1 || h.len() == 2).then(|| digits(h)).flatten())?;
    let minute = it
        .next()
        .and_then(|m| (m.len() == 2).then(|| digits(m)).flatten())?;
    let (second, fraction) = match it.next() {
        None => (0, &b""[..]),
        Some(s) => {
            let (whole, fraction) = match s.iter().position(|b| *b == b'.') {
                Some(dot) => (&s[..dot], &s[dot + 1..]),
                None => (s, &b""[..]),
            };
            if whole.len() != 2 || (!fraction.is_empty() && digits(fraction).is_none()) {
                return None;
            }
            (digits(whole)?, fraction)
        }
    };
    if it.next().is_some() {
        return None;
    }
    Some(Some(Time {
        hour: hour.try_into().unwrap_or(u8::MAX),
        minute: minute.try_into().unwrap_or(u8::MAX),
        second: second.try_into().unwrap_or(u8::MAX),
        fraction,
    }))
}

/// A trailing `Z`, `+HH:MM` or `-HH:MM`, split off the clock.
fn split_zone(tail: &[u8]) -> (&[u8], &[u8]) {
    if tail.last() == Some(&b'Z') {
        return (&tail[..tail.len() - 1], b"Z");
    }
    if tail.len() > 6 {
        let at = tail.len() - 6;
        if matches!(tail[at], b'+' | b'-') && tail[at + 3] == b':' {
            return (&tail[..at], &tail[at..]);
        }
    }
    (tail, b"")
}

fn zone_is(zone: &[u8], offset: &str) -> bool {
    match zone {
        b"Z" => offset == "+00:00",
        z => z == offset.as_bytes(),
    }
}

fn valid(p: Parts<'_>) -> bool {
    let days = match p.month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if p.year.is_multiple_of(4)
            && (!p.year.is_multiple_of(100) || p.year.is_multiple_of(400)) =>
        {
            29
        }
        2 => 28,
        _ => return false,
    };
    (1900..=2999).contains(&p.year)
        && (1..=days).contains(&p.day)
        && p.time
            .is_none_or(|t| t.hour < 24 && t.minute < 60 && t.second < 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mms(v: &str) -> Option<String> {
        let mut out = Vec::new();
        to_mms(v.as_bytes(), "+10:00", &mut out).then(|| String::from_utf8(out).unwrap())
    }

    #[test]
    fn every_form_aemo_writes() {
        assert_eq!(mms("29/09/2018").unwrap(), "2018/09/29");
        assert_eq!(mms("01/01/2024 07:55:00").unwrap(), "2024/01/01 07:55:00");
        assert_eq!(mms("17-03-2023").unwrap(), "2023/03/17");
        assert_eq!(mms("02 Oct 2026").unwrap(), "2026/10/02");
        assert_eq!(mms("2 Oct 2026").unwrap(), "2026/10/02");
        assert_eq!(mms("01 Oct 2026 11:31:18").unwrap(), "2026/10/01 11:31:18");
        assert_eq!(mms("01 Aug 2010 6:00").unwrap(), "2010/08/01 06:00:00");
        assert_eq!(mms("19 OCT 2026 17:00:05").unwrap(), "2026/10/19 17:00:05");
        assert_eq!(mms("2026-09-29").unwrap(), "2026/09/29");
        assert_eq!(mms("2026-10-02 13:02").unwrap(), "2026/10/02 13:02:00");
        assert_eq!(mms("2022-01-01 08:00:00").unwrap(), "2022/01/01 08:00:00");
        assert_eq!(
            mms("2026-10-21T08:00:00+10:00").unwrap(),
            "2026/10/21 08:00:00"
        );
        assert_eq!(
            mms("2026-10-21T08:00:00.5+10:00").unwrap(),
            "2026/10/21 08:00:00.5"
        );
        assert_eq!(mms("2026-10-21T08:00:00").unwrap(), "2026/10/21 08:00:00");
        assert_eq!(mms(" 2018/09/30 13:20 ").unwrap(), "2018/09/30 13:20:00");
        assert_eq!(mms("Thursday 5 Aug 2021").unwrap(), "2021/08/05");
        assert_eq!(mms("Sunday  8 Aug 2021").unwrap(), "2021/08/08");
        assert!(mms("Someday 5 Aug 2021").is_none());
    }

    #[test]
    fn the_mms_form_stands() {
        let mut out = vec![1u8];
        assert!(!to_mms(b"2018/09/29", "+10:00", &mut out));
        assert!(out.is_empty());
        assert!(!to_mms(b"2018/09/30 13:20:39", "+10:00", &mut out));
        assert!(!to_mms(b"2018/09/30 13:20:39.250", "+10:00", &mut out));
        assert!(is_date(b"2018/09/29", "+10:00"));
    }

    #[test]
    fn a_foreign_offset_stays_text() {
        assert!(mms("2026-10-21T08:00:00+08:00").is_none());
        assert!(mms("2026-10-21T08:00:00Z").is_none());
        let mut out = Vec::new();
        assert!(to_mms(b"2026-10-21T08:00:00Z", "+00:00", &mut out));
        assert_eq!(out, b"2026/10/21 08:00:00");
        assert!(!to_mms(b"2026-10-21 08:00:00+10:00", "+10:00", &mut out));
    }

    #[test]
    fn what_is_not_a_date() {
        for v in [
            "",
            "abc",
            "1234567",
            "01-Apr-2023 to 30-Jun-2023",
            "JAN-2024",
            "Dec-2026",
            "30-June",
            "06:00:00",
            "2027-04",
            "31/02/2023",
            "29/02/2023",
            "2023/13/01",
            "00/01/2023",
            "01 Xyz 2026",
            "01 Oct 2026 24:00:00",
            "01 Oct 2026 11:60:00",
            "01 Oct 2026 11:00:60",
            "01 Oct 2026 11",
            "01 Oct 2026 1:0",
            "01 Oct 2026 11:00:00:00",
            "01 Oct 2026 11:00:0x",
            "01 Oct 2026 11:00:00.x",
            "01 Oct 2026 11:00:000",
            "01 Oct 2026x",
            "01 Oct 202",
            "123 Oct 2026",
            " Oct 2026 11",
            "01 Oct2026 11",
            "20x6-10-21",
            "2026-1x-21",
            "2026-10-2x",
            "2x/10/2026",
            "20/1x/2026",
            "20/10/20x6",
            "1899/12/31",
            "99999999999/01/01",
        ] {
            assert!(mms(v).is_none(), "{v:?}");
            assert!(!is_date(v.as_bytes(), "+10:00"), "{v:?}");
        }
    }

    #[test]
    fn leap_days() {
        assert_eq!(mms("29/02/2024").unwrap(), "2024/02/29");
        assert_eq!(mms("29/02/2000").unwrap(), "2000/02/29");
        assert!(mms("29/02/1900").is_none());
        assert!(mms("31/04/2024").is_none());
        assert_eq!(mms("30/04/2024").unwrap(), "2024/04/30");
    }
}
