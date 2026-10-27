//! An instant with the offset it was written in, and every text form the
//! family reads one from.
//!
//! Outside the lake an instant is text, and the text always says its
//! offset: `2024-02-13T08:27:00Z` for the tool's own clock and HTTP dates,
//! `2024-02-13T18:27:00+10:00` for a time nemweb lists in market time. A
//! time without an offset is never written. One that is read (an IIS
//! listing, a zip entry's DOS time) is read in the offset its writer used:
//! AEST (+10:00, no daylight saving) for nemweb and AEMO's zips, AWST
//! (+08:00) for AEMO's WA host.
//!
//! The one `Stamp` of the family: the mirror, the lake and the census all
//! read and write this type. Not on the hot path, but the short IIS form
//! is read in one pass: a listing has thousands of them.

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::civil::{civil_from_days, days_from_civil, days_in_month, weekday};

/// Seconds east of UTC of Australian Eastern Standard Time: the NEM's
/// clock, nemweb's listings and AEMO's zips.
pub const AEST: i32 = 10 * 3600;

/// Seconds east of UTC of Australian Western Standard Time: the WEM's
/// clock and AEMO's WA host.
pub const AWST: i32 = 8 * 3600;

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

const WEEKDAYS: [&str; 7] = [
    "monday",
    "tuesday",
    "wednesday",
    "thursday",
    "friday",
    "saturday",
    "sunday",
];

/// An instant with the offset it was written in. Ordered by the instant,
/// then by the offset.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Stamp {
    /// Whole seconds since the Unix epoch, UTC.
    pub secs_utc: i64,
    /// Nanoseconds within that second.
    pub nanos: u32,
    /// Seconds east of UTC the text gives the time in.
    pub offset_secs: i32,
}

/// Text that is not RFC 3339 with an offset.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("`{0}` is not a time in RFC 3339 form with an offset")]
pub struct BadStamp(pub String);

impl Stamp {
    /// An instant in whole seconds, to be written in `offset_secs`.
    #[must_use]
    pub const fn new(secs_utc: i64, offset_secs: i32) -> Self {
        Self {
            secs_utc,
            nanos: 0,
            offset_secs,
        }
    }

    /// A Unix time in whole seconds, written in UTC.
    #[must_use]
    pub const fn from_unix(secs: i64) -> Self {
        Self::new(secs, 0)
    }

    /// Now, in whole seconds, UTC.
    #[must_use]
    pub fn now() -> Self {
        let secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        Self::from_unix(i64::try_from(secs).unwrap_or(i64::MAX))
    }

    /// A civil date and time read in `offset_secs`; none when a field is
    /// out of range (a leap second reads as the minute's last).
    #[must_use]
    pub fn at(y: i32, m: u32, d: u32, hh: u32, mm: u32, ss: u32, offset_secs: i32) -> Option<Self> {
        if offset_secs.unsigned_abs() >= 86_400 {
            return None;
        }
        let local = local_seconds(y, m, d, hh, mm, ss)?;
        Some(Self::new(local - i64::from(offset_secs), offset_secs))
    }

    /// Seconds since the Unix epoch, UTC.
    #[must_use]
    pub const fn unix(self) -> i64 {
        self.secs_utc
    }

    /// Seconds east of UTC.
    #[must_use]
    pub const fn offset(self) -> i32 {
        self.offset_secs
    }

    /// The same instant written in `offset_secs`.
    #[must_use]
    pub const fn in_offset(self, offset_secs: i32) -> Self {
        Self {
            offset_secs,
            ..self
        }
    }

    /// The instant cut to its whole minute. Every offset the family meets
    /// is whole minutes, so the cut is the same in any of them.
    #[must_use]
    pub const fn to_minute(self) -> Self {
        Self {
            secs_utc: self.secs_utc - self.secs_utc.rem_euclid(60),
            nanos: 0,
            offset_secs: self.offset_secs,
        }
    }

    /// Whether `other` is the same instant, whatever offset each is
    /// written in.
    #[must_use]
    pub const fn same_instant(self, other: Self) -> bool {
        self.secs_utc == other.secs_utc && self.nanos == other.nanos
    }

    /// The RFC 3339 text: `YYYY-MM-DDTHH:MM:SS[.fraction]` followed by `Z`
    /// or `+HH:MM`.
    #[must_use]
    pub fn rfc3339(&self) -> String {
        self.to_string()
    }

    /// The time as RFC 3339 without its separators, for a file name:
    /// `20261021T115300+1000`, `20261021T015300Z`. Whole seconds.
    #[must_use]
    pub fn compact(self) -> String {
        let (y, m, d, hh, mm, ss) = self.civil();
        let zone = match self.offset_secs {
            0 => "Z".to_owned(),
            o => {
                let sign = if o < 0 { '-' } else { '+' };
                let abs = o.unsigned_abs();
                format!("{sign}{:02}{:02}", abs / 3600, abs % 3600 / 60)
            }
        };
        format!("{y:04}{m:02}{d:02}T{hh:02}{mm:02}{ss:02}{zone}")
    }

    /// The instant as an HTTP date (RFC 7231), whatever its offset:
    /// `Wed, 15 Apr 2026 04:29:11 GMT`. Whole seconds.
    #[must_use]
    pub fn http_date(self) -> String {
        let utc = self.in_offset(0);
        let (y, m, d, hh, mm, ss) = utc.civil();
        let day = weekday(self.secs_utc.div_euclid(86_400)) as usize;
        let title = |w: &str| format!("{}{}", w[..1].to_ascii_uppercase(), &w[1..3]);
        format!(
            "{}, {d:02} {} {y:04} {hh:02}:{mm:02}:{ss:02} GMT",
            title(WEEKDAYS[day]),
            title(MONTHS[m as usize - 1])
        )
    }

    /// RFC 3339 text with an offset, and nothing else. The `T` may be a
    /// `t` or a space, as RFC 3339 allows.
    pub fn parse(text: &str) -> Result<Self, BadStamp> {
        parse_rfc3339(text).ok_or_else(|| BadStamp(text.chars().take(64).collect()))
    }

    /// The HTTP date a server sends in `Last-Modified`:
    /// `Tue, 13 Feb 2024 08:27:00 GMT`.
    #[must_use]
    pub fn from_http_date(text: &str) -> Option<Self> {
        let mut t = tokens(text);
        let (wday, dd, mon, yyyy, hms, zone) = (
            t.next()?,
            t.next()?,
            t.next()?,
            t.next()?,
            t.next()?,
            t.next()?,
        );
        if t.next().is_some() || !is_weekday(wday) || !matches!(zone, "GMT" | "UTC") {
            return None;
        }
        let (hh, mm, ss, nanos, rest) = time_of_day(hms.as_bytes())?;
        if !rest.is_empty() {
            return None;
        }
        let year = i32::try_from(number(yyyy)?).ok()?;
        let s = Self::at(year, month_number(mon)?, number(dd)?, hh, mm, ss, 0)?;
        Some(Self { nanos, ..s })
    }

    /// AEMO's WEM manifest `generatedAt`: `Thu Nov 02 2023 11:35:00Z`.
    #[must_use]
    pub fn from_wem_generated(text: &str) -> Option<Self> {
        let mut t = tokens(text);
        let (wday, mon, dd, yyyy, hms) = (t.next()?, t.next()?, t.next()?, t.next()?, t.next()?);
        if t.next().is_some() || !is_weekday(wday) {
            return None;
        }
        let (hh, mm, ss, nanos, rest) = time_of_day(hms.as_bytes())?;
        if !matches!(rest, b"Z" | b"z") {
            return None;
        }
        let year = i32::try_from(number(yyyy)?).ok()?;
        let s = Self::at(year, month_number(mon)?, number(dd)?, hh, mm, ss, 0)?;
        Some(Self { nanos, ..s })
    }

    /// The time an IIS directory listing shows, in the clock of the host
    /// that served it (`offset_secs`): `2/13/2024  8:27 AM` or
    /// `Monday, October 12, 2026 10:45 AM`.
    #[must_use]
    pub fn from_listing(text: &str, offset_secs: i32) -> Option<Self> {
        if let Some(local) = iis_short_local(text.as_bytes()) {
            return Some(Self::new(local - i64::from(offset_secs), offset_secs));
        }
        let mut t = tokens(text);
        if !is_weekday(t.next()?) {
            return None;
        }
        let m = month_number(t.next()?)?;
        let d = number(t.next()?)?;
        let y = i32::try_from(number(t.next()?)?).ok()?;
        let (hh, mm, ss, nanos, rest) = iis_time_of_day(t.next()?.as_bytes())?;
        let hh = twelve_hour(hh, rest, t.next()?)?;
        if t.next().is_some() {
            return None;
        }
        let s = Self::at(y, m, d, hh, mm, ss, offset_secs)?;
        Some(Self { nanos, ..s })
    }

    /// `YYYY-MM-DD HH:MM:SS` or `YYYY/MM/DD HH:MM:SS` (a `T` between the
    /// parts too) with no offset, as a zip entry's DOS time and MMS dates
    /// are written: read in `offset_secs`.
    #[must_use]
    pub fn from_naive(text: &str, offset_secs: i32) -> Option<Self> {
        let b = text.as_bytes();
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
        let s = Self::at(y, m, d, hh, mm, ss, offset_secs)?;
        Some(Self { nanos, ..s })
    }

    /// Time text in any form the family meets. A form that carries its
    /// offset (RFC 3339, an HTTP date, AEMO's WEM `generatedAt`) is read in
    /// it; one that does not (an IIS listing, `YYYY-MM-DD HH:MM:SS`) is
    /// read in `naive_offset_secs`, the offset its writer used.
    #[must_use]
    pub fn read(text: &str, naive_offset_secs: i32) -> Option<Self> {
        let s = text.trim();
        parse_rfc3339(s)
            .or_else(|| Self::from_http_date(s))
            .or_else(|| Self::from_wem_generated(s))
            .or_else(|| Self::from_listing(s, naive_offset_secs))
            .or_else(|| Self::from_naive(s, naive_offset_secs))
    }

    /// Year, month, day, hour, minute and second in the stamp's offset.
    fn civil(self) -> (i32, u32, u32, i64, i64, i64) {
        let local = self.secs_utc + i64::from(self.offset_secs);
        let rem = local.rem_euclid(86_400);
        let (y, m, d) = civil_from_days(local.div_euclid(86_400));
        (y, m, d, rem / 3600, rem / 60 % 60, rem % 60)
    }
}

impl fmt::Display for Stamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (y, m, d, hh, mm, ss) = self.civil();
        write!(f, "{y:04}-{m:02}-{d:02}T{hh:02}:{mm:02}:{ss:02}")?;
        if self.nanos != 0 {
            let digits = format!("{:09}", self.nanos);
            write!(f, ".{}", digits.trim_end_matches('0'))?;
        }
        if self.offset_secs == 0 {
            return f.write_str("Z");
        }
        let sign = if self.offset_secs < 0 { '-' } else { '+' };
        let abs = self.offset_secs.unsigned_abs();
        write!(f, "{sign}{:02}:{:02}", abs / 3600, abs % 3600 / 60)
    }
}

/// Written as its RFC 3339 text.
impl Serialize for Stamp {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

/// Read from RFC 3339 text; text without an offset is refused.
impl<'de> Deserialize<'de> for Stamp {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let text = <std::borrow::Cow<'de, str>>::deserialize(d)?;
        Self::parse(&text).map_err(serde::de::Error::custom)
    }
}

/// `YYYY-MM-DDTHH:MM:SS[.fraction](Z|+HH:MM|-HH:MM)`.
fn parse_rfc3339(s: &str) -> Option<Stamp> {
    let b = s.as_bytes();
    let (y, m, d) = civil_date(b.get(..10)?, b'-')?;
    if !matches!(b.get(10), Some(b'T' | b't' | b' ')) {
        return None;
    }
    let (hh, mm, ss, nanos, rest) = time_of_day(&b[11..])?;
    let offset_secs = match rest {
        b"Z" | b"z" => 0,
        [sign @ (b'+' | b'-'), oh0, oh1, b':', om0, om1] => {
            let oh = two(&[*oh0, *oh1])?;
            let om = two(&[*om0, *om1])?;
            if oh > 23 || om > 59 {
                return None;
            }
            let secs = i32::try_from(oh * 3600 + om * 60).ok()?;
            if *sign == b'-' { -secs } else { secs }
        }
        _ => return None,
    };
    let s = Stamp::at(y, m, d, hh, mm, ss, offset_secs)?;
    Some(Stamp { nanos, ..s })
}

/// The short IIS form, `M/d/yyyy h:mm AM`, as local seconds, in one pass.
fn iis_short_local(b: &[u8]) -> Option<i64> {
    /// One to `max` digits at `*i`, advancing past them.
    fn num(b: &[u8], i: &mut usize, max: usize) -> Option<u32> {
        let start = *i;
        let mut n = 0u32;
        while *i < b.len() && *i - start < max {
            let Some(d) = digit(b[*i]) else { break };
            n = n * 10 + d;
            *i += 1;
        }
        (*i > start).then_some(n)
    }
    /// The byte `c` at `*i`, advancing past it.
    fn expect(b: &[u8], i: &mut usize, c: u8) -> Option<()> {
        (b.get(*i) == Some(&c)).then(|| *i += 1)
    }
    /// One or more spaces at `*i`, advancing past them.
    fn spaces(b: &[u8], i: &mut usize) -> Option<()> {
        let n = b[*i..].iter().take_while(|c| **c == b' ').count();
        *i += n;
        (n > 0).then_some(())
    }
    let i = &mut 0usize;
    let m = num(b, i, 2)?;
    expect(b, i, b'/')?;
    let d = num(b, i, 2)?;
    expect(b, i, b'/')?;
    let y = num(b, i, 4)?;
    spaces(b, i)?;
    let hh = num(b, i, 2)?;
    expect(b, i, b':')?;
    let start = *i;
    let mm = num(b, i, 2)?;
    (*i - start == 2).then_some(())?;
    spaces(b, i)?;
    let meridiem = std::str::from_utf8(&b[*i..]).ok()?;
    let hh = twelve_hour(hh, b"", meridiem)?;
    local_seconds(i32::try_from(y).ok()?, m, d, hh, mm, 0)
}

/// Words separated by spaces, tabs or commas.
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
fn civil_date(b: &[u8], sep: u8) -> Option<(i32, u32, u32)> {
    if b.len() != 10 || b[4] != sep || b[7] != sep {
        return None;
    }
    let y = two(&b[0..2])? * 100 + two(&b[2..4])?;
    Some((i32::try_from(y).ok()?, two(&b[5..7])?, two(&b[8..10])?))
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
    let mut ss = 0;
    let mut nanos = 0;
    if rest.first() == Some(&b':') {
        ss = two(&rest[1..])?;
        rest = &rest[3..];
        if rest.first() == Some(&b'.') {
            let frac = &rest[1..];
            let digits = frac.iter().take_while(|c| c.is_ascii_digit()).count();
            if digits == 0 || digits > 9 {
                return None;
            }
            for d in &frac[..digits] {
                nanos = nanos * 10 + digit(*d)?;
            }
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

/// A month by its English name or three-letter abbreviation, 1 to 12.
fn month_number(name: &str) -> Option<u32> {
    let at = MONTHS.iter().position(|m| {
        m.eq_ignore_ascii_case(name) || (name.len() == 3 && m[..3].eq_ignore_ascii_case(name))
    })?;
    u32::try_from(at + 1).ok()
}

/// A weekday by its English name or three-letter abbreviation.
fn is_weekday(name: &str) -> bool {
    WEEKDAYS.iter().any(|w| {
        w.eq_ignore_ascii_case(name) || (name.len() == 3 && w[..3].eq_ignore_ascii_case(name))
    })
}

/// Seconds since the epoch of a civil date and time, checked.
fn local_seconds(y: i32, m: u32, d: u32, hh: u32, mm: u32, ss: u32) -> Option<i64> {
    if !(1..=12).contains(&m) || d == 0 || d > days_in_month(y, m) || hh > 23 || mm > 59 || ss > 60
    {
        return None;
    }
    Some(
        days_from_civil(y, m, d) * 86_400
            + i64::from(hh) * 3600
            + i64::from(mm) * 60
            + i64::from(ss),
    )
}

#[cfg(test)]
mod tests;
