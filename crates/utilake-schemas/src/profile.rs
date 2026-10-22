//! What a column's values are, read from every D row: how many are null,
//! what kind they are on a widening lattice, how long and how precise.
//!
//! Nothing is sampled and nothing is taken from AEMO's declarations: this is
//! the evidence the column's Iceberg type is chosen from
//! ([`utilake_iceberg::choose`]) and the declarations are audited against.

use std::cmp::Ordering;

use utilake_iceberg::{Evidence, Kind};

/// One column of one block.
#[derive(Debug, Clone, Default)]
pub struct ColumnProfile {
    /// Rows without a value: an empty field, or no field at all.
    pub nulls: u64,
    /// The kind of every value.
    pub kind: Kind,
    /// The most digits before the point.
    pub int_digits: u8,
    /// The most digits after it, as written.
    pub scale: u8,
    /// The most significant digits of any one value.
    pub sig_digits: u8,
    /// The longest value, in bytes.
    pub max_len: u32,
    /// The least value of a number or time column, as written.
    pub min: Option<Vec<u8>>,
    /// The greatest value of a number or time column, as written.
    pub max: Option<Vec<u8>>,
    /// Where the digits of `min` and `max` are, once a number has been
    /// compared with them: every value is compared with both, and this
    /// spares reading the bounds' digits again each time.
    pub(crate) plain_bounds: [Option<Plain>; 2],
}

/// Two profiles are the same when what they record is: the bounds' digits
/// are only remembered to compare faster.
impl PartialEq for ColumnProfile {
    fn eq(&self, o: &Self) -> bool {
        (
            self.nulls,
            self.kind,
            self.int_digits,
            self.scale,
            self.sig_digits,
            self.max_len,
            &self.min,
            &self.max,
        ) == (
            o.nulls,
            o.kind,
            o.int_digits,
            o.scale,
            o.sig_digits,
            o.max_len,
            &o.min,
            &o.max,
        )
    }
}

impl Eq for ColumnProfile {}

impl ColumnProfile {
    /// Takes one field; `None` when the row has no field there.
    pub fn observe(&mut self, v: Option<&[u8]>) {
        let Some(v) = v.filter(|v| !v.is_empty()) else {
            self.nulls += 1;
            return;
        };
        let (kind, int_digits, scale, sig, pv) = scan(v);
        self.kind = self.kind.join(kind);
        self.int_digits = self.int_digits.max(int_digits);
        self.scale = self.scale.max(scale);
        self.sig_digits = self.sig_digits.max(sig);
        self.max_len = self.max_len.max(u32::try_from(v.len()).unwrap_or(u32::MAX));
        if self.kind == Kind::Text {
            self.min = None;
            self.max = None;
            self.plain_bounds = [None; 2];
            return;
        }
        if !matches!(self.kind, Kind::Integer | Kind::Decimal) {
            // Dates and times order as written.
            for (slot, want) in [
                (&mut self.min, Ordering::Less),
                (&mut self.max, Ordering::Greater),
            ] {
                if slot.as_deref().is_none_or(|m| v.cmp(m) == want) {
                    set(slot, v);
                }
            }
            return;
        }
        // A number: its digits found as it was classified, each bound's
        // kept from when it was set.
        for (i, want) in [(0, Ordering::Less), (1, Ordering::Greater)] {
            let slot = if i == 0 { &mut self.min } else { &mut self.max };
            let better = match slot.as_deref() {
                None => true,
                Some(m) => {
                    let pm = self.plain_bounds[i].or_else(|| plain(m));
                    order_with(v, pv, m, pm) == want
                }
            };
            if better {
                set(slot, v);
                self.plain_bounds[i] = pv;
            }
        }
    }

    /// Takes in another profile of the same column.
    pub fn merge(&mut self, other: &Self) {
        self.nulls += other.nulls;
        self.kind = self.kind.join(other.kind);
        self.int_digits = self.int_digits.max(other.int_digits);
        self.scale = self.scale.max(other.scale);
        self.sig_digits = self.sig_digits.max(other.sig_digits);
        self.max_len = self.max_len.max(other.max_len);
        if self.kind == Kind::Text {
            self.min = None;
            self.max = None;
            return;
        }
        let numeric = matches!(self.kind, Kind::Integer | Kind::Decimal);
        let pick = |mine: &mut Option<Vec<u8>>, theirs: &Option<Vec<u8>>, want: Ordering| {
            if let Some(t) = theirs
                && mine.as_ref().is_none_or(|m| order(t, m, numeric) == want)
            {
                *mine = Some(t.clone());
            }
        };
        pick(&mut self.min, &other.min, Ordering::Less);
        pick(&mut self.max, &other.max, Ordering::Greater);
        self.plain_bounds = [None; 2];
    }

    /// What the profile says of the column's type.
    pub fn evidence(&self) -> Evidence {
        Evidence {
            kind: self.kind,
            int_digits: self.int_digits,
            scale: self.scale,
            sig_digits: self.sig_digits,
        }
    }
}

/// Replaces a bound, reusing its buffer.
fn set(slot: &mut Option<Vec<u8>>, v: &[u8]) {
    let b = slot.get_or_insert_with(Vec::new);
    b.clear();
    b.extend_from_slice(v);
}

/// Where a plain decimal's digits are: its sign, its whole digits without
/// leading zeros and its fraction digits without trailing zeros, as ranges
/// of the value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Plain {
    neg: bool,
    int: (u32, u32),
    frac: (u32, u32),
}

impl Plain {
    fn parts(self, v: &[u8]) -> (bool, &[u8], &[u8]) {
        let r = |(a, b): (u32, u32)| &v[a as usize..b as usize];
        (self.neg, r(self.int), r(self.frac))
    }
}

/// Orders two values: numbers by value, times as written (AEMO's
/// `YYYY/MM/DD HH:MM:SS` orders as text).
fn order(a: &[u8], b: &[u8], numeric: bool) -> Ordering {
    if numeric {
        order_with(a, plain(a), b, plain(b))
    } else {
        a.cmp(b)
    }
}

/// Orders two numbers whose digits are found: plain decimals by their
/// digits, anything else (an exponent form) by value.
fn order_with(a: &[u8], pa: Option<Plain>, b: &[u8], pb: Option<Plain>) -> Ordering {
    match (pa, pb) {
        (Some(x), Some(y)) => cmp_plain(x.parts(a), y.parts(b)),
        _ => {
            let n = |v: &[u8]| crate::text::parse_f64(v).unwrap_or(f64::NAN);
            n(a).total_cmp(&n(b))
        }
    }
}

/// The digits of a plain decimal; `None` for anything else (an exponent
/// form).
fn plain(v: &[u8]) -> Option<Plain> {
    let start = usize::from(matches!(v.first(), Some(b'-' | b'+')));
    let neg = start == 1 && v[0] == b'-';
    let mut point = None;
    for (i, &c) in v.iter().enumerate().skip(start) {
        match c {
            b'0'..=b'9' => {}
            b'.' if point.is_none() => point = Some(i),
            _ => return None,
        }
    }
    let int_end = point.unwrap_or(v.len());
    let frac_start = point.map_or(v.len(), |p| p + 1);
    let int_start = v[start..int_end]
        .iter()
        .position(|&c| c != b'0')
        .map_or(int_end, |i| start + i);
    let frac_end = v[frac_start..]
        .iter()
        .rposition(|&c| c != b'0')
        .map_or(frac_start, |i| frac_start + i + 1);
    let at = |i: usize| u32::try_from(i).unwrap_or(u32::MAX);
    // Minus zero is zero.
    Some(Plain {
        neg: neg && !(int_start == int_end && frac_start == frac_end),
        int: (at(int_start), at(int_end)),
        frac: (at(frac_start), at(frac_end)),
    })
}

/// Orders two plain decimals by value, without parsing them.
fn cmp_plain(a: (bool, &[u8], &[u8]), b: (bool, &[u8], &[u8])) -> Ordering {
    let magnitude = |(_, ai, af): (bool, &[u8], &[u8]), (_, bi, bf): (bool, &[u8], &[u8])| {
        ai.len().cmp(&bi.len()).then(ai.cmp(bi)).then(af.cmp(bf))
    };
    match (a.0, b.0) {
        (false, true) => Ordering::Greater,
        (true, false) => Ordering::Less,
        (false, false) => magnitude(a, b),
        (true, true) => magnitude(b, a),
    }
}

/// The kind of one value, with its digits before and after the point and
/// its significant digits.
pub fn classify(v: &[u8]) -> (Kind, u8, u8, u8) {
    let (k, i, s, g, _) = scan(v);
    (k, i, s, g)
}

/// [`classify`], and where a plain decimal's digits are.
fn scan(v: &[u8]) -> (Kind, u8, u8, u8, Option<Plain>) {
    if let Some(k) = time_kind(v) {
        return (k, 0, 0, 0, None);
    }
    number(v).unwrap_or((Kind::Text, 0, 0, 0, None))
}

/// `YYYY/MM/DD` is a date; with ` HH:MM:SS` and an optional fraction, a
/// time.
fn time_kind(v: &[u8]) -> Option<Kind> {
    let digits =
        |r: std::ops::Range<usize>| v.get(r).is_some_and(|s| s.iter().all(u8::is_ascii_digit));
    let date = v.len() >= 10
        && digits(0..4)
        && v[4] == b'/'
        && digits(5..7)
        && v[7] == b'/'
        && digits(8..10);
    if !date {
        return None;
    }
    if v.len() == 10 {
        return Some(Kind::Date);
    }
    let time = v.len() >= 19
        && v[10] == b' '
        && digits(11..13)
        && v[13] == b':'
        && digits(14..16)
        && v[16] == b':'
        && digits(17..19);
    let fraction = || v.len() == 19 || (v[19] == b'.' && v.len() > 20 && digits(20..v.len()));
    (time && fraction()).then_some(Kind::Timestamp)
}

/// A number: an optional sign, digits with at most one point, and an
/// optional exponent. Its digits before and after the point are those of
/// its value written out in full; leading zeros do not count. A number
/// without an exponent comes with where its digits are.
fn number(v: &[u8]) -> Option<(Kind, u8, u8, u8, Option<Plain>)> {
    let start = usize::from(matches!(v.first(), Some(b'-' | b'+')));
    let mut i = start;
    let (mut int_len, mut frac_len, mut leading) = (0i32, 0i32, true);
    let mut sig = 0i32;
    let mut point = None;
    let mut any = false;
    // The first whole digit that is not a leading zero, and the end of the
    // last fraction digit that is not a trailing zero.
    let (mut int_start, mut frac_end) = (None, None);
    while let Some(&c) = v.get(i) {
        match c {
            b'0'..=b'9' => {
                any = true;
                if point.is_some() {
                    frac_len += 1;
                    if c != b'0' {
                        frac_end = Some(i + 1);
                    }
                } else if !(leading && c == b'0') {
                    int_len += 1;
                    int_start.get_or_insert(i);
                }
                if !(leading && c == b'0') {
                    leading = false;
                    sig += 1;
                }
            }
            b'.' if point.is_none() => point = Some(i),
            _ => break,
        }
        i += 1;
    }
    if !any {
        return None;
    }
    let mut exp = 0i32;
    let mut exponent = false;
    if let Some(b'e' | b'E') = v.get(i) {
        exponent = true;
        let rest = std::str::from_utf8(&v[i + 1..]).ok()?;
        exp = rest.parse().ok()?;
    } else if i != v.len() {
        return None;
    }
    let int_digits = int_len.saturating_add(exp).max(0);
    let scale = frac_len.saturating_sub(exp).max(0);
    let kind = if point.is_some() || exponent {
        Kind::Decimal
    } else {
        Kind::Integer
    };
    let cap = |n: i32| u8::try_from(n).unwrap_or(u8::MAX);
    let plain = (!exponent).then(|| {
        let int_end = point.unwrap_or(v.len());
        let frac_start = point.map_or(v.len(), |p| p + 1);
        let int_start = int_start.unwrap_or(int_end);
        let frac_end = frac_end.unwrap_or(frac_start);
        let at = |i: usize| u32::try_from(i).unwrap_or(u32::MAX);
        Plain {
            // Minus zero is zero.
            neg: start == 1 && v[0] == b'-' && !(int_start == int_end && frac_start == frac_end),
            int: (at(int_start), at(int_end)),
            frac: (at(frac_start), at(frac_end)),
        }
    });
    Some((kind, cap(int_digits), cap(scale), cap(sig), plain))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c3(v: &[u8]) -> (Kind, u8, u8) {
        let (k, i, s, _) = classify(v);
        (k, i, s)
    }

    #[test]
    fn classifies_aemo_values() {
        assert_eq!(classify(b"239.99999999999997").3, 17);
        assert_eq!(classify(b"0.000123").3, 3);
        assert_eq!(classify(b"-0042").3, 2);
        assert_eq!(classify(b"N").3, 0);
        assert_eq!(c3(b"2024/08/01 00:05:00"), (Kind::Timestamp, 0, 0));
        assert_eq!(c3(b"2024/08/01 00:05:00.123"), (Kind::Timestamp, 0, 0));
        assert_eq!(c3(b"2024/08/01"), (Kind::Date, 0, 0));
        assert_eq!(c3(b"2024/08/01 00:05"), (Kind::Text, 0, 0));
        assert_eq!(c3(b"2024/08/01 00:05:00."), (Kind::Text, 0, 0));
        assert_eq!(c3(b"2024/08/01X"), (Kind::Text, 0, 0));
        assert_eq!(c3(b"123"), (Kind::Integer, 3, 0));
        assert_eq!(c3(b"-0042"), (Kind::Integer, 2, 0));
        assert_eq!(c3(b"0"), (Kind::Integer, 0, 0));
        assert_eq!(c3(b"15500.12345"), (Kind::Decimal, 5, 5));
        assert_eq!(c3(b".5"), (Kind::Decimal, 0, 1));
        assert_eq!(c3(b"12."), (Kind::Decimal, 2, 0));
        assert_eq!(c3(b"1.5E-05"), (Kind::Decimal, 0, 6));
        assert_eq!(c3(b"2E+3"), (Kind::Decimal, 4, 0));
        assert_eq!(c3(b"2E+x"), (Kind::Text, 0, 0));
        assert_eq!(c3(b"."), (Kind::Text, 0, 0));
        assert_eq!(c3(b"1.2.3"), (Kind::Text, 0, 0));
        assert_eq!(c3(b"NSW1"), (Kind::Text, 0, 0));
        assert_eq!(c3(&[0xff, b'1']), (Kind::Text, 0, 0));
    }

    #[test]
    fn a_column_widens_counts_nulls_and_bounds_its_values() {
        let mut p = ColumnProfile::default();
        for v in [
            Some(&b"5"[..]),
            None,
            Some(b""),
            Some(b"-12.5"),
            Some(b"300"),
        ] {
            p.observe(v);
        }
        assert_eq!(p.nulls, 2);
        assert_eq!(p.kind, Kind::Decimal);
        assert_eq!((p.int_digits, p.scale, p.max_len), (3, 1, 5));
        assert_eq!(p.min.as_deref(), Some(&b"-12.5"[..]));
        assert_eq!(p.max.as_deref(), Some(&b"300"[..]));
        assert_eq!(
            p.evidence(),
            Evidence {
                kind: Kind::Decimal,
                int_digits: 3,
                scale: 1,
                sig_digits: 3
            }
        );

        let mut t = ColumnProfile::default();
        t.observe(Some(b"2024/08/02 00:00:00"));
        t.observe(Some(b"2024/08/01"));
        assert_eq!(t.kind, Kind::Timestamp);
        assert_eq!(t.min.as_deref(), Some(&b"2024/08/01"[..]));
        assert_eq!(t.max.as_deref(), Some(&b"2024/08/02 00:00:00"[..]));
        // Text has no bounds.
        t.observe(Some(b"N/A"));
        assert_eq!(
            (t.kind, t.min.clone(), t.max.clone()),
            (Kind::Text, None, None)
        );
        t.observe(Some(b"OTHER"));
        assert_eq!(t.max_len, 19);

        let never = ColumnProfile::default();
        assert_eq!(never.evidence(), Evidence::default());
    }

    #[test]
    fn profiles_merge_as_if_one() {
        let mut a = ColumnProfile::default();
        a.observe(Some(b"5"));
        a.observe(None);
        let mut b = ColumnProfile::default();
        b.observe(Some(b"-2.25"));
        b.observe(Some(b"40"));
        let mut both = a.clone();
        both.merge(&b);
        let mut once = ColumnProfile::default();
        for v in [Some(&b"5"[..]), None, Some(b"-2.25"), Some(b"40")] {
            once.observe(v);
        }
        assert_eq!(both, once);
        let mut text = ColumnProfile::default();
        text.observe(Some(b"N/A"));
        both.merge(&text);
        assert_eq!((both.kind, both.min, both.max), (Kind::Text, None, None));
        let mut empty = ColumnProfile::default();
        empty.merge(&b);
        assert_eq!(empty, b);
    }

    #[test]
    fn numbers_order_by_value_and_bad_ones_last() {
        assert_eq!(order(b"9", b"10", true), Ordering::Less);
        assert_eq!(order(b"9", b"10", false), Ordering::Greater);
        assert_eq!(order(b"x", b"1", true), Ordering::Greater);
        assert_eq!(order(b"-2.5", b"-10", true), Ordering::Greater);
        assert_eq!(order(b"-0.0", b"0", true), Ordering::Equal);
        assert_eq!(order(b"+007.50", b"7.5", true), Ordering::Equal);
        assert_eq!(order(b"0.25", b"0.3", true), Ordering::Less);
        assert_eq!(order(b"1E+3", b"999", true), Ordering::Greater);
        assert_eq!(order(b"-1", b"2", true), Ordering::Less);
    }

    #[test]
    fn a_column_that_changes_kind_starts_its_bounds_again() {
        let mut p = ColumnProfile::default();
        p.observe(Some(b"2024/08/02"));
        p.observe(Some(b"7"));
        assert_eq!(p.kind, Kind::Text);
        let mut q = ColumnProfile::default();
        q.observe(Some(b"7"));
        q.observe(Some(b"3.5"));
        assert_eq!(
            (q.min.as_deref(), q.max.as_deref()),
            (Some(&b"3.5"[..]), Some(&b"7"[..]))
        );
    }
}
