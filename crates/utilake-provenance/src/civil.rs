//! The proleptic Gregorian calendar in whole days: what every instant in
//! the family is counted in. No calendar library.

/// Days from 1970-01-01 to the given civil date.
///
/// Howard Hinnant's `days_from_civil`, valid for every year an `i32` holds.
#[inline]
#[must_use]
pub const fn days_from_civil(y: i32, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y } as i64;
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m as i64 + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The civil date of a day count from 1970-01-01, as `(year, month, day)`.
#[must_use]
pub const fn civil_from_days(z: i64) -> (i32, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    ((if m <= 2 { y + 1 } else { y }) as i32, m, d)
}

#[inline]
const fn is_leap(y: i32) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

/// The days of month `m` (1..=12) in year `y`; 0 for a month that is not.
#[inline]
#[must_use]
pub const fn days_in_month(y: i32, m: u32) -> u32 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if is_leap(y) {
                29
            } else {
                28
            }
        }
        _ => 0,
    }
}

/// The day of the week of a day count from 1970-01-01, 0 for Monday.
#[inline]
#[must_use]
pub const fn weekday(days: i64) -> u32 {
    // 1970-01-01 was a Thursday.
    (days + 3).rem_euclid(7) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn epoch_and_known_dates() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(days_from_civil(2000, 3, 1), 11_017);
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(-1), (1969, 12, 31));
        assert_eq!(civil_from_days(days_from_civil(-401, 2, 28)), (-401, 2, 28));
    }

    #[test]
    fn round_trips_every_day_of_four_centuries() {
        let start = days_from_civil(1900, 1, 1);
        for z in start..start + 146_097 {
            let (y, m, d) = civil_from_days(z);
            assert!(d >= 1 && d <= days_in_month(y, m), "{y}-{m}-{d}");
            assert_eq!(days_from_civil(y, m, d), z);
        }
    }

    #[test]
    fn months_and_weekdays() {
        assert_eq!(days_in_month(2024, 2), 29);
        assert_eq!(days_in_month(1900, 2), 28);
        assert_eq!(days_in_month(2000, 2), 29);
        assert_eq!(days_in_month(2023, 4), 30);
        assert_eq!(days_in_month(2023, 13), 0);
        // 1970-01-01 a Thursday; 2026-04-15 a Wednesday; before the epoch.
        assert_eq!(weekday(0), 3);
        assert_eq!(weekday(days_from_civil(2026, 4, 15)), 2);
        assert_eq!(weekday(-1), 2);
    }
}
