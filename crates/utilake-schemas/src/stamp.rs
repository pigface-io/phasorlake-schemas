//! Time as text in a nemweb-mirror sidecar: RFC 3339 that says its offset,
//! and the forms an older sidecar kept a time in beside it.
//!
//! A sidecar keeps the server's `Last-Modified` (an HTTP date) and the
//! time the listing showed (IIS text, in market time) as written, with the
//! same instants beside them as RFC 3339 (`last_modified_at`,
//! `listed_modified_at`). One written before the instants were kept lacks
//! them; they are read from the text instead. A form that carries its
//! offset is read in it; one that does not is read in the offset its writer
//! used, market time (+10:00) for nemweb.

use std::fmt;

use crate::sql::{days_from_civil, days_in_month};

/// Market time, the offset nemweb lists in: AEST, +10:00, no daylight
/// saving.
pub const NEM_OFFSET_SECS: i32 = 36_000;

/// An instant with the offset it was written in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stamp {
    /// Whole seconds since the Unix epoch, UTC.
    pub secs_utc: i64,
    /// Nanoseconds within that second.
    pub nanos: u32,
    /// Seconds east of UTC the text gave the time in.
    pub offset_secs: i32,
}

/// RFC 3339: `YYYY-MM-DDTHH:MM:SS[.fraction]`, then `Z` or `+HH:MM`.
impl fmt::Display for Stamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let local = self.secs_utc + i64::from(self.offset_secs);
        let rem = local.rem_euclid(86_400);
        let (y, m, d) = civil_from_days(local.div_euclid(86_400));
        let (hh, mm, ss) = (rem / 3600, (rem / 60) % 60, rem % 60);
        let fraction = if self.nanos == 0 {
            String::new()
        } else {
            format!(".{:09}", self.nanos)
                .trim_end_matches('0')
                .to_owned()
        };
        let abs = self.offset_secs.unsigned_abs();
        let zone = match self.offset_secs {
            0 => "Z".to_owned(),
            o => format!(
                "{}{:02}:{:02}",
                if o < 0 { '-' } else { '+' },
                abs / 3600,
                (abs % 3600) / 60
            ),
        };
        write!(
            f,
            "{y:04}-{m:02}-{d:02}T{hh:02}:{mm:02}:{ss:02}{fraction}{zone}"
        )
    }
}

/// The civil date of a day count from 1970-01-01.
fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

/// Time text in any form a sidecar holds: RFC 3339, an HTTP date, AEMO's
/// WEM `generatedAt`, an IIS listing, or `YYYY-MM-DD HH:MM:SS`; the last
/// two, which carry no offset, read in `naive_offset_secs`.
#[must_use]
pub fn parse(s: &str, naive_offset_secs: i32) -> Option<Stamp> {
    let s = s.trim();
    parse_rfc3339(s)
        .or_else(|| parse_http_date(s))
        .or_else(|| parse_wem_generated(s))
        .or_else(|| parse_iis_listing(s, naive_offset_secs))
        .or_else(|| parse_naive(s, naive_offset_secs))
}

/// `YYYY-MM-DDTHH:MM:SS[.fraction](Z|+HH:MM|-HH:MM)`; the `T` may be a
/// `t` or a space, as RFC 3339 allows. Text without an offset is not one.
#[must_use]
pub fn parse_rfc3339(s: &str) -> Option<Stamp> {
    let b = s.as_bytes();
    let (y, m, d) = civil_date(b.get(..10)?, b'-')?;
    if !matches!(b.get(10), Some(b'T' | b't' | b' ')) {
        return None;
    }
    let (hh, mm, ss, nanos, rest) = time_of_day(&b[11..])?;
    let offset_secs = match rest {
        b"Z" | b"z" => 0,
        [sign @ (b'+' | b'-'), oh0, oh1, b':', om0, om1] => {
            let (oh, om) = (two(&[*oh0, *oh1])?, two(&[*om0, *om1])?);
            if oh > 23 || om > 59 {
                return None;
            }
            let secs = i32::try_from(oh * 3600 + om * 60).ok()?;
            if *sign == b'-' { -secs } else { secs }
        }
        _ => return None,
    };
    at(y, (m, d), (hh, mm, ss, nanos), offset_secs)
}

/// The HTTP date of a `Last-Modified`: `Tue, 13 Feb 2024 08:27:00 GMT`.
fn parse_http_date(s: &str) -> Option<Stamp> {
    let t: Vec<&str> = tokens(s).collect();
    let [wday, dd, mon, yyyy, hms, zone] = t[..] else {
        return None;
    };
    if !is_weekday(wday) || !matches!(zone, "GMT" | "UTC") {
        return None;
    }
    let (hh, mm, ss, nanos, rest) = time_of_day(hms.as_bytes())?;
    if !rest.is_empty() {
        return None;
    }
    let (y, m, d) = (number(yyyy)?, month_number(mon)?, number(dd)?);
    at(y, (m, d), (hh, mm, ss, nanos), 0)
}

/// AEMO's WEM manifest `generatedAt`: `Thu Nov 02 2023 11:35:00Z`.
fn parse_wem_generated(s: &str) -> Option<Stamp> {
    let t: Vec<&str> = tokens(s).collect();
    let [wday, mon, dd, yyyy, hms] = t[..] else {
        return None;
    };
    if !is_weekday(wday) {
        return None;
    }
    let (hh, mm, ss, nanos, rest) = time_of_day(hms.as_bytes())?;
    if !matches!(rest, b"Z" | b"z") {
        return None;
    }
    let (y, m, d) = (number(yyyy)?, month_number(mon)?, number(dd)?);
    at(y, (m, d), (hh, mm, ss, nanos), 0)
}

/// The time an IIS directory listing shows, in the server's local time
/// (`offset_secs`): `2/13/2024  8:27 AM`, or `Monday, October 12, 2026
/// 10:45 AM`.
fn parse_iis_listing(s: &str, offset_secs: i32) -> Option<Stamp> {
    if let Some(short) = iis_short(s, offset_secs) {
        return Some(short);
    }
    let t: Vec<&str> = tokens(s).collect();
    let [wday, mon, dd, yyyy, hm, meridiem] = t[..] else {
        return None;
    };
    if !is_weekday(wday) {
        return None;
    }
    let (y, m, d) = (number(yyyy)?, month_number(mon)?, number(dd)?);
    let (hh, mm, ss, nanos, rest) = iis_time_of_day(hm.as_bytes())?;
    let hh = twelve_hour(hh, rest, meridiem)?;
    at(y, (m, d), (hh, mm, ss, nanos), offset_secs)
}

/// The short IIS form, `M/d/yyyy h:mm AM`: its parts apart by spaces, a
/// month and day of one or two digits, a year of up to four, the minutes
/// two.
fn iis_short(s: &str, offset_secs: i32) -> Option<Stamp> {
    let up_to = |p: &str, n: usize| (p.len() <= n).then(|| number(p)).flatten();
    let t: Vec<&str> = s.split(' ').filter(|t| !t.is_empty()).collect();
    let [date, hm, meridiem] = t[..] else {
        return None;
    };
    let mut date = date.split('/');
    let (m, d, y) = (date.next()?, date.next()?, date.next()?);
    if date.next().is_some() {
        return None;
    }
    let (h, mm) = hm.split_once(':')?;
    if mm.len() != 2 {
        return None;
    }
    let hh = twelve_hour(up_to(h, 2)?, b"", meridiem)?;
    let (m, d, y) = (up_to(m, 2)?, up_to(d, 2)?, up_to(y, 4)?);
    at(y, (m, d), (hh, number(mm)?, 0, 0), offset_secs)
}

/// `YYYY-MM-DD HH:MM:SS` or `YYYY/MM/DD HH:MM:SS` (a `T` between the
/// parts too) with no offset, as a zip entry's time is written: read in
/// `offset_secs`.
fn parse_naive(s: &str, offset_secs: i32) -> Option<Stamp> {
    let b = s.as_bytes();
    let date = b.get(..10)?;
    let sep = if date[4] == b'/' { b'/' } else { b'-' };
    let (y, m, d) = civil_date(date, sep)?;
    if !matches!(b.get(10), Some(b'T' | b't' | b' ')) {
        return None;
    }
    let (hh, mm, ss, nanos, rest) = time_of_day(&b[11..])?;
    if !rest.is_empty() {
        return None;
    }
    at(y, (m, d), (hh, mm, ss, nanos), offset_secs)
}

/// The instant of a civil date and time read in `offset_secs`, checked: a
/// leap second is read as the last second of its minute.
fn at(
    y: u32,
    (m, d): (u32, u32),
    (hh, mm, ss, nanos): (u32, u32, u32, u32),
    offset_secs: i32,
) -> Option<Stamp> {
    let (y, m, d) = (i64::from(y), i64::from(m), i64::from(d));
    let day_ok = (1..=12).contains(&m) && (1..=days_in_month(y, m)).contains(&d);
    if !day_ok || hh > 23 || mm > 59 || ss > 60 {
        return None;
    }
    let local = days_from_civil(y, m, d) * 86_400
        + i64::from(hh) * 3600
        + i64::from(mm) * 60
        + i64::from(ss);
    Some(Stamp {
        secs_utc: local - i64::from(offset_secs),
        nanos,
        offset_secs,
    })
}

/// Words apart by spaces, commas or tabs.
fn tokens(s: &str) -> impl Iterator<Item = &str> {
    s.split([' ', ',', '\t']).filter(|t| !t.is_empty())
}

fn digit(b: u8) -> Option<u32> {
    let d = b.wrapping_sub(b'0');
    (d < 10).then_some(u32::from(d))
}

fn two(s: &[u8]) -> Option<u32> {
    Some(digit(*s.first()?)? * 10 + digit(*s.get(1)?)?)
}

/// One to six decimal digits.
fn number(s: &str) -> Option<u32> {
    if s.is_empty() || s.len() > 6 {
        return None;
    }
    s.bytes().try_fold(0u32, |n, b| Some(n * 10 + digit(b)?))
}

/// `YYYY<sep>MM<sep>DD`, exactly ten bytes.
fn civil_date(b: &[u8], sep: u8) -> Option<(u32, u32, u32)> {
    if b.len() != 10 || b[4] != sep || b[7] != sep {
        return None;
    }
    let y = two(&b[0..2])? * 100 + two(&b[2..4])?;
    Some((y, two(&b[5..7])?, two(&b[8..10])?))
}

/// `HH:MM[:SS[.fraction]]` at the start of `b`: the parts, the fraction as
/// nanoseconds, and what follows.
fn time_of_day(b: &[u8]) -> Option<(u32, u32, u32, u32, &[u8])> {
    from_minutes(two(b)?, b.get(2..)?)
}

/// The same with a one-digit hour allowed, as IIS writes `8:27 AM`.
fn iis_time_of_day(b: &[u8]) -> Option<(u32, u32, u32, u32, &[u8])> {
    if b.get(1) == Some(&b':') {
        return from_minutes(digit(b[0])?, &b[1..]);
    }
    time_of_day(b)
}

/// `:MM[:SS[.fraction]]` at the start of `b`, after the hour `hh`.
fn from_minutes(hh: u32, b: &[u8]) -> Option<(u32, u32, u32, u32, &[u8])> {
    if b.first() != Some(&b':') {
        return None;
    }
    let mm = two(&b[1..])?;
    let mut rest = &b[3..];
    let (mut ss, mut nanos) = (0, 0);
    if rest.first() == Some(&b':') {
        ss = two(&rest[1..])?;
        rest = &rest[3..];
        if rest.first() == Some(&b'.') {
            let frac = &rest[1..];
            let digits = frac.iter().take_while(|c| c.is_ascii_digit()).count();
            if digits == 0 || digits > 9 {
                return None;
            }
            nanos = frac[..digits]
                .iter()
                .fold(0, |n, d| n * 10 + u32::from(d - b'0'));
            nanos *= 10u32.pow(9 - u32::try_from(digits).ok()?);
            rest = &frac[digits..];
        }
    }
    Some((hh, mm, ss, nanos, rest))
}

/// A twelve-hour clock reading with its `AM`/`PM`.
fn twelve_hour(hh: u32, rest: &[u8], meridiem: &str) -> Option<u32> {
    if !rest.is_empty() || !(1..=12).contains(&hh) {
        return None;
    }
    match meridiem {
        "AM" | "am" => Some(hh % 12),
        "PM" | "pm" => Some(hh % 12 + 12),
        _ => None,
    }
}

/// A month's number from its name or its first three letters, any case.
fn month_number(name: &str) -> Option<u32> {
    const MONTHS: [&str; 12] = [
        "january",
        "february",
        "march",
        "april",
        "may",
        "june",
        "july",
        "august",
        "september",
        "october",
        "november",
        "december",
    ];
    let lower = name.to_ascii_lowercase();
    let i = MONTHS.iter().position(|m| lower == *m || lower == m[..3])?;
    u32::try_from(i + 1).ok()
}

/// Whether `name` is a weekday, whole or its first three letters, any case.
fn is_weekday(name: &str) -> bool {
    const DAYS: [&str; 7] = [
        "monday",
        "tuesday",
        "wednesday",
        "thursday",
        "friday",
        "saturday",
        "sunday",
    ];
    let lower = name.to_ascii_lowercase();
    DAYS.iter().any(|d| lower == *d || lower == d[..3])
}

#[cfg(test)]
mod tests {
    use super::*;

    const T: i64 = 1_707_812_820; // 2024-02-13T08:27:00Z

    fn stamp(secs_utc: i64, offset_secs: i32) -> Stamp {
        Stamp {
            secs_utc,
            nanos: 0,
            offset_secs,
        }
    }

    #[test]
    fn writes_rfc3339_in_its_offset() {
        assert_eq!(stamp(T, 0).to_string(), "2024-02-13T08:27:00Z");
        assert_eq!(
            stamp(T, NEM_OFFSET_SECS).to_string(),
            "2024-02-13T18:27:00+10:00"
        );
        assert_eq!(
            stamp(T, -3600 * 5 - 1800).to_string(),
            "2024-02-13T02:57:00-05:30"
        );
        assert_eq!(stamp(0, 0).to_string(), "1970-01-01T00:00:00Z");
        assert_eq!(stamp(-1, 0).to_string(), "1969-12-31T23:59:59Z");
        let frac = Stamp {
            nanos: 500_000,
            ..stamp(T, 0)
        };
        assert_eq!(frac.to_string(), "2024-02-13T08:27:00.0005Z");
        // Every day of a leap year and the years around it, both ways.
        for day in days_from_civil(2023, 1, 1)..days_from_civil(2025, 3, 1) {
            let (y, m, d) = civil_from_days(day);
            assert_eq!(days_from_civil(y, m, d), day);
        }
    }

    #[test]
    fn reads_rfc3339_only_with_an_offset() {
        assert_eq!(parse_rfc3339("2024-02-13T08:27:00Z"), Some(stamp(T, 0)));
        assert_eq!(
            parse_rfc3339("2024-02-13T18:27:00+10:00"),
            Some(stamp(T, NEM_OFFSET_SECS))
        );
        assert_eq!(
            parse_rfc3339("2024-02-13t02:57:00-05:30"),
            Some(stamp(T, -19_800))
        );
        assert_eq!(
            parse_rfc3339("2024-02-13 08:27:00.25z"),
            Some(Stamp {
                nanos: 250_000_000,
                ..stamp(T, 0)
            })
        );
        for naive in [
            "2024-02-13T08:27:00",
            "2024-02-13 08:27:00",
            "2024-02-13",
            "2024-02-13T08:27",
            "2024-02-13T08:27:00+10",
            "2024-02-13T08:27:00+25:00",
            "2024-02-13T08:27:00+10:60",
            "2024-02-13T08:27:00.Z",
            "2024-02-13T08:27:00.1234567890Z",
            "2024-02-30T08:27:00Z",
            "2024-13-01T08:27:00Z",
            "2024-02-13T24:00:00Z",
            "2024-02-13T08:60:00Z",
            "2024-02-13T08:27:61Z",
            "",
            "Z",
        ] {
            assert_eq!(parse_rfc3339(naive), None, "{naive}");
        }
        // A leap second is read as the last second of the minute.
        assert!(parse_rfc3339("2016-12-31T23:59:60Z").is_some());
    }

    #[test]
    fn reads_http_dates() {
        assert_eq!(
            parse_http_date("Tue, 13 Feb 2024 08:27:00 GMT"),
            Some(stamp(T, 0))
        );
        assert_eq!(
            parse_http_date("Mon, 18 May 2026 07:27:50 UTC").map(|t| t.to_string()),
            Some("2026-05-18T07:27:50Z".into())
        );
        for bad in [
            "Tue, 13 Feb 2024 08:27:00",
            "Tue, 13 Feb 2024 08:27:00 AEST",
            "Xyz, 13 Feb 2024 08:27:00 GMT",
            "Tue, 13 Foo 2024 08:27:00 GMT",
            "Tue, 30 Feb 2024 08:27:00 GMT",
            "Tue, 13 Feb 2024 08:27:00 GMT extra",
            "Tue, 13 Feb 2024 08:27:00Z GMT",
        ] {
            assert_eq!(parse_http_date(bad), None, "{bad}");
        }
    }

    #[test]
    fn reads_wem_generated_at() {
        assert_eq!(
            parse_wem_generated("Thu Nov 02 2023 11:35:00Z").map(|t| t.to_string()),
            Some("2023-11-02T11:35:00Z".into())
        );
        assert_eq!(parse_wem_generated("Thu Nov 02 2023 11:35:00"), None);
        assert_eq!(parse_wem_generated("Nov 02 2023 11:35:00Z"), None);
        assert_eq!(parse_wem_generated("Xyz Nov 02 2023 11:35:00Z"), None);
        assert_eq!(parse_wem_generated("Thu Nov 02 2023 11:35:00Z x"), None);
    }

    #[test]
    fn reads_iis_listings_in_the_offset_given() {
        let iis = |s: &str, off: i32| parse_iis_listing(s, off).map(|t| t.to_string());
        // nemweb lists in AEST: 8:27 AM there is 22:27 the day before in UTC.
        let short = parse_iis_listing("2/13/2024  8:27 AM", NEM_OFFSET_SECS).unwrap();
        assert_eq!(short.to_string(), "2024-02-13T08:27:00+10:00");
        assert_eq!(short.secs_utc, T - i64::from(NEM_OFFSET_SECS));
        assert_eq!(
            iis("Monday, October 12, 2026 10:45 AM", NEM_OFFSET_SECS).as_deref(),
            Some("2026-10-12T10:45:00+10:00")
        );
        assert_eq!(
            iis("Monday, May 18, 2026 05:27 PM", NEM_OFFSET_SECS).as_deref(),
            Some("2026-05-18T17:27:00+10:00")
        );
        assert_eq!(
            iis("12/31/2024 12:00 AM", 0).as_deref(),
            Some("2024-12-31T00:00:00Z")
        );
        assert_eq!(
            iis("12/31/2024 12:30 PM", 0).as_deref(),
            Some("2024-12-31T12:30:00Z")
        );
        assert_eq!(
            iis("02/03/2024 08:05 am", 0).as_deref(),
            Some("2024-02-03T08:05:00Z")
        );
        for bad in [
            "2/13/2024 8:27",
            "2/13/2024 8:27 XM",
            "2/13/2024 13:27 PM",
            "2/13/2024 0:27 AM",
            "2/13/2024/1 8:27 AM",
            "2/13/2024 8:27 AM 1",
            "2/13/2024 8:7 AM",
            "2/13/20245 8:27 AM",
            "2/13/2024  8:27AM",
            "2/13/20248:27 AM",
            "2/13 8:27 AM",
            "2/13/2024 827 AM",
            "Funday, October 12, 2026 10:45 AM",
            "Monday, Octember 12, 2026 10:45 AM",
            "Monday, October 12, 2026 10:45 AM 1",
            "Monday, October 12, 2026 x:45 AM",
            "13/2/2024 8:27 AM",
            "",
        ] {
            assert_eq!(parse_iis_listing(bad, 0), None, "{bad}");
        }
    }

    #[test]
    fn reads_a_naive_time_in_the_offset_given() {
        let naive = |s: &str, off: i32| parse_naive(s, off).map(|t| t.to_string());
        assert_eq!(
            naive("2017-01-12 12:24:16", NEM_OFFSET_SECS).as_deref(),
            Some("2017-01-12T12:24:16+10:00")
        );
        assert_eq!(
            naive("2017/01/12T12:24:16.5", 0).as_deref(),
            Some("2017-01-12T12:24:16.5Z")
        );
        assert_eq!(parse_naive("2017-01-12T12:24:16Z", 0), None);
        assert_eq!(parse_naive("2017-01-12", 0), None);
        assert_eq!(parse_naive("2017-01-12x12:24:16", 0), None);
        assert_eq!(parse_naive("2017-01-12 12:24:16 x", 0), None);
    }

    #[test]
    fn reads_any_form() {
        let any = |s: &str| parse(s, NEM_OFFSET_SECS).map(|t| t.to_string());
        assert_eq!(
            any("Tue, 13 Feb 2024 08:27:00 GMT").as_deref(),
            Some("2024-02-13T08:27:00Z")
        );
        assert_eq!(
            any(" 2024-02-13T08:27:00Z ").as_deref(),
            Some("2024-02-13T08:27:00Z")
        );
        assert_eq!(
            any("Thu Nov 02 2023 11:35:00Z").as_deref(),
            Some("2023-11-02T11:35:00Z")
        );
        assert_eq!(
            any("Monday, May 18, 2026 05:27 PM").as_deref(),
            Some("2026-05-18T17:27:00+10:00")
        );
        assert_eq!(
            any("2017-01-12 12:24:16").as_deref(),
            Some("2017-01-12T12:24:16+10:00")
        );
        assert_eq!(any("yesterday"), None);
        assert_eq!(any(""), None);
    }

    #[test]
    fn helpers_reject_what_is_not_theirs() {
        assert_eq!(number(""), None);
        assert_eq!(number("1234567"), None);
        assert_eq!(number("12a"), None);
        assert_eq!(month_number("Sept"), None);
        assert_eq!(month_number("DEC"), Some(12));
        assert!(is_weekday("SUNDAY"));
        assert!(!is_weekday("Sun."));
        assert_eq!(time_of_day(b"8:27"), None);
        assert_eq!(time_of_day(b"08-27"), None);
        assert_eq!(time_of_day(b"08:27:0"), None);
        assert_eq!(time_of_day(b"08"), None);
        assert_eq!(iis_time_of_day(b"8:27"), Some((8, 27, 0, 0, &b""[..])));
        assert_eq!(iis_time_of_day(b"08:27"), Some((8, 27, 0, 0, &b""[..])));
        assert_eq!(iis_time_of_day(b":27"), None);
        assert_eq!(iis_time_of_day(b"x:27"), None);
        assert_eq!(at(2023, (2, 29), (0, 0, 0, 0), 0), None);
        assert_eq!(at(2024, (2, 29), (0, 0, 61, 0), 0), None);
    }
}
