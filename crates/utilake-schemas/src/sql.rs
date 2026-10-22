//! `sql TABLE --files FILES.json --from T --to T [--as-of SCHEMA_ID]`: the
//! DuckDB query that reads an interval of a table's Parquet files across
//! its schemas ([`utilake_iceberg::plan`], [`utilake_iceberg::duckdb_sql`]).
//! The table's schemas, breaks and time column are read from what the
//! export published, `nem/{TABLE}/metadata/v1.metadata.json`, and nothing
//! else: the metadata alone plans a read.
//!
//! `FILES.json` lists the table's data files, `[{path, schema_id, min_us,
//! max_us}]`: where each is, the schema it was written under, and its
//! least and greatest time value in microseconds since 1970 (UTC), either
//! null when unknown. A time is RFC 3339, or a day `YYYY-MM-DD`, which is
//! its midnight in the market's time (+10:00). The interval is
//! `[from, to)` on the table's declared time column.

use std::path::Path;

use serde::Deserialize;
use utilake_iceberg::table::TableMetadata;
use utilake_iceberg::{DataFile, PlanError, Target, duckdb_sql, from_metadata, plan};

use crate::export::METADATA;
use utilake_iceberg::Market;

/// Why the query could not be written.
#[derive(Debug, thiserror::Error)]
pub enum SqlError {
    /// No table of that name is published.
    #[error("There is no published table {table} under {market}/.")]
    UnknownTable {
        /// The market.
        market: Market,
        /// The table.
        table: String,
    },
    /// The table's metadata that cannot be read.
    #[error("The table metadata {path} cannot be read: {message}")]
    Metadata {
        /// The file.
        path: String,
        /// What is wrong.
        message: String,
    },
    /// The file list that cannot be read.
    #[error("The file list {path} cannot be read: {message}")]
    Files {
        /// The file.
        path: String,
        /// What is wrong.
        message: String,
    },
    /// A time that is not one.
    #[error("{0:?} is not an RFC 3339 time or a day (YYYY-MM-DD).")]
    Time(String),
    /// An interval that ends before it starts.
    #[error("The interval from {from} to {to} ends before it starts.")]
    Empty {
        /// Its start.
        from: String,
        /// Its end.
        to: String,
    },
    /// A read the planner refuses.
    #[error("The read of {table} cannot be planned: {source}")]
    Plan {
        /// The table.
        table: String,
        /// Cause.
        #[source]
        source: PlanError,
    },
}

/// One data file, as `FILES.json` lists it.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Listed {
    path: String,
    schema_id: u32,
    min_us: Option<i64>,
    max_us: Option<i64>,
}

/// Days from 1970-01-01 to a civil date (proleptic Gregorian).
pub(crate) fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The days of month `m` of year `y`.
pub(crate) fn days_in_month(y: i64, m: i64) -> i64 {
    match m {
        2 if (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// A time as microseconds since 1970 (UTC): RFC 3339 (`T` or a space
/// between date and time, a fraction of up to nine digits, `Z` or an
/// offset), or a day, which is its midnight at `offset_secs` from UTC
/// (the market's: 36 000 for +10:00).
pub fn parse_time(s: &str, offset_secs: i32) -> Option<i64> {
    let b = s.as_bytes();
    let num = |r: std::ops::Range<usize>| -> Option<i64> {
        let d = b.get(r)?;
        d.iter()
            .all(u8::is_ascii_digit)
            .then(|| d.iter().fold(0, |n, c| n * 10 + i64::from(c - b'0')))
    };
    let (y, m, d) = (num(0..4)?, num(5..7)?, num(8..10)?);
    if b.get(4) != Some(&b'-') || b.get(7) != Some(&b'-') {
        return None;
    }
    if !(1..=12).contains(&m) || !(1..=days_in_month(y, m)).contains(&d) {
        return None;
    }
    let day = days_from_civil(y, m, d) * 86_400;
    if b.len() == 10 {
        return Some((day - i64::from(offset_secs)) * 1_000_000);
    }
    let (h, mi, sec) = (num(11..13)?, num(14..16)?, num(17..19)?);
    let shaped =
        matches!(b[10], b'T' | b't' | b' ') && b.get(13) == Some(&b':') && b.get(16) == Some(&b':');
    if !shaped || h > 23 || mi > 59 || sec > 59 {
        return None;
    }
    let mut at = 19;
    let mut micros = 0;
    if b.get(at) == Some(&b'.') {
        let digits = b[at + 1..]
            .iter()
            .take_while(|c| c.is_ascii_digit())
            .count();
        if !(1..=9).contains(&digits) {
            return None;
        }
        let frac = num(at + 1..at + 1 + digits.min(6))?;
        micros = frac * 10_i64.pow(6 - u32::try_from(digits.min(6)).ok()?);
        at += 1 + digits;
    }
    let offset = match &b[at..] {
        [b'Z' | b'z'] => 0,
        [sign @ (b'+' | b'-'), _, _, b':', _, _] => {
            let (oh, om) = (num(at + 1..at + 3)?, num(at + 4..at + 6)?);
            if oh > 23 || om > 59 {
                return None;
            }
            let o = oh * 3600 + om * 60;
            if *sign == b'+' { o } else { -o }
        }
        _ => return None,
    };
    Some((day + h * 3600 + mi * 60 + sec - offset) * 1_000_000 + micros)
}

fn time(s: &str, market: Market) -> Result<i64, SqlError> {
    parse_time(s, market.offset_secs()).ok_or_else(|| SqlError::Time(s.to_owned()))
}

/// The DuckDB query reading `[from, to)` of `table`'s files listed in
/// `files`, projected onto schema `as_of` or the current one; the table's
/// metadata is read under `root/nem/`.
pub fn sql(
    root: &Path,
    market: Market,
    table: &str,
    files: &Path,
    (from, to): (&str, &str),
    as_of: Option<u32>,
) -> Result<String, SqlError> {
    let path = root.join(market.id()).join(table).join(METADATA);
    let text = match std::fs::read(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(SqlError::UnknownTable {
                market,
                table: table.to_owned(),
            });
        }
        Err(e) => {
            return Err(SqlError::Metadata {
                path: path.display().to_string(),
                message: e.to_string(),
            });
        }
    };
    let published = serde_json::from_slice::<TableMetadata>(&text)
        .map_err(|e| e.to_string())
        .and_then(|m| from_metadata(&m).map_err(|e| e.to_string()))
        .map_err(|message| SqlError::Metadata {
            path: path.display().to_string(),
            message,
        })?;
    let h = published.history;
    let bad = |message: String| SqlError::Files {
        path: files.display().to_string(),
        message,
    };
    let text = std::fs::read(files).map_err(|e| bad(e.to_string()))?;
    let listed: Vec<Listed> = serde_json::from_slice(&text).map_err(|e| bad(e.to_string()))?;
    let interval = (time(from, market)?, time(to, market)?);
    if interval.1 < interval.0 {
        return Err(SqlError::Empty {
            from: from.to_owned(),
            to: to.to_owned(),
        });
    }
    let data: Vec<DataFile> = listed
        .into_iter()
        .map(|f| DataFile {
            path: f.path,
            schema_id: f.schema_id,
            min_us: f.min_us,
            max_us: f.max_us,
        })
        .collect();
    let target = as_of.map_or(Target::Latest, Target::AsOf);
    let reads = plan(&h, &data, interval, target).map_err(|source| SqlError::Plan {
        table: table.to_owned(),
        source,
    })?;
    let column = published
        .declaration
        .as_ref()
        .and_then(|d| d.time_column.as_deref());
    let mut out = duckdb_sql(&reads, column, interval);
    out.push('\n');
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::export::tests::exported;

    #[test]
    fn times_are_rfc_3339_or_a_market_day() {
        let day = parse_time("2024-08-01", 36_000).unwrap();
        assert_eq!(
            day,
            parse_time("2024-08-01T00:00:00+10:00", 36_000).unwrap()
        );
        assert_eq!(day, parse_time("2024-07-31 14:00:00Z", 36_000).unwrap());
        assert_eq!(day, 1_722_434_400_000_000);
        assert_eq!(
            parse_time("2024-08-01", 28_800).unwrap(),
            day + 2 * 3600 * 1_000_000,
            "a day in Perth begins two hours later"
        );
        assert_eq!(parse_time("1970-01-01T00:00:00Z", 36_000), Some(0));
        assert_eq!(
            parse_time("1969-12-31T23:59:59.5-00:00", 36_000),
            Some(-500_000)
        );
        assert_eq!(
            parse_time("2024-02-29t01:02:03.1234567z", 36_000),
            Some(1_709_168_523_123_456)
        );
        assert_eq!(
            parse_time("2000-03-01T00:00:00Z", 36_000),
            Some(951_868_800_000_000)
        );
        for bad in [
            "",
            "2024-8-01",
            "2024/08/01",
            "2024-13-01",
            "2023-02-29",
            "1900-02-29",
            "2024-04-31",
            "2024-08-01T",
            "2024-08-01X00:00:00Z",
            "2024-08-01T24:00:00Z",
            "2024-08-01T00:60:00Z",
            "2024-08-01T00:00:60Z",
            "2024-08-01T00:00:00",
            "2024-08-01T00:00:00.Z",
            "2024-08-01T00:00:00.1234567890Z",
            "2024-08-01T00:00:00+24:00",
            "2024-08-01T00:00:00+10:60",
            "2024-08-01T00:00:00+1000",
        ] {
            assert_eq!(parse_time(bad, 36_000), None, "{bad}");
        }
    }

    #[test]
    fn writes_the_query_of_an_interval() {
        let dir = exported();
        let root = dir.path();
        let files = root.join("files.json");
        std::fs::write(
            &files,
            r#"[{"path": "a.parquet", "schema_id": 0, "min_us": 0, "max_us": 10},
                {"path": "b.parquet", "schema_id": 1, "min_us": null, "max_us": null}]"#,
        )
        .unwrap();
        let q = |from, to, as_of| {
            sql(
                root,
                Market::Nem,
                "DISPATCH_UNIT_SCADA",
                &files,
                (from, to),
                as_of,
            )
        };
        let text = q("1970-01-01T00:00:00Z", "2024-08-01", None).unwrap();
        assert!(text.starts_with("SELECT \"SETTLEMENTDATE\""), "{text}");
        // One projection: the two schemas differ only in their order.
        assert!(
            text.contains("read_parquet(['a.parquet', 'b.parquet'])"),
            "{text}"
        );
        assert!(
            text.contains("< make_timestamptz(1722434400000000)"),
            "{text}"
        );
        assert!(text.ends_with(")\n"), "{text}");
        let as_of = q("2024-08-01", "2024-08-02", Some(0)).unwrap();
        assert!(
            !as_of.contains("a.parquet") && as_of.contains("b.parquet"),
            "{as_of}"
        );

        let e = |r: Result<String, SqlError>| r.unwrap_err().to_string();
        assert_eq!(
            e(q("x", "2024-08-01", None)),
            "\"x\" is not an RFC 3339 time or a day (YYYY-MM-DD)."
        );
        assert_eq!(
            e(q("2024-08-02", "2024-08-01", None)),
            "The interval from 2024-08-02 to 2024-08-01 ends before it starts."
        );
        assert!(
            e(q("2024-08-01", "2024-08-02", Some(9)))
                .starts_with("The read of DISPATCH_UNIT_SCADA cannot be planned: ")
        );
        assert_eq!(
            e(sql(
                root,
                Market::Nem,
                "NONE",
                &files,
                ("2024-08-01", "2024-08-02"),
                None
            )),
            "There is no published table NONE under nem/."
        );
        std::fs::write(&files, r#"[{"path": 1}]"#).unwrap();
        assert!(e(q("2024-08-01", "2024-08-02", None)).starts_with("The file list "));
        std::fs::remove_file(&files).unwrap();
        assert!(e(q("2024-08-01", "2024-08-02", None)).starts_with("The file list "));
        // Metadata that is not JSON, not a published table, or a directory.
        let meta = crate::fold::published_path(root, Market::Nem, "DISPATCH_UNIT_SCADA");
        std::fs::write(&meta, "[").unwrap();
        assert!(e(q("2024-08-01", "2024-08-02", None)).contains("v1.metadata.json cannot be read"));
        std::fs::write(&meta, "{}").unwrap();
        assert!(e(q("2024-08-01", "2024-08-02", None)).contains("v1.metadata.json cannot be read"));
        std::fs::remove_file(&meta).unwrap();
        std::fs::create_dir(&meta).unwrap();
        assert!(e(q("2024-08-01", "2024-08-02", None)).contains("v1.metadata.json cannot be read"));
    }
}
