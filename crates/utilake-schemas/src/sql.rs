//! `sql TABLE --files FILES.json --from T --to T [--as-of SCHEMA_ID]`: the
//! DuckDB query that reads an interval of a table's Parquet files across
//! its schemas ([`utilake_iceberg::plan`], [`utilake_iceberg::duckdb_sql`]).
//!
//! `FILES.json` lists the table's data files, `[{path, schema_id, min_us,
//! max_us}]`: where each is, the schema it was written under, and its
//! least and greatest time value in microseconds since 1970 (UTC), either
//! null when unknown. A time is RFC 3339, or a day `YYYY-MM-DD`, which is
//! its midnight in the market's time (+10:00). The interval is
//! `[from, to)` on the table's declared time column.

use std::path::Path;

use serde::Deserialize;
use utilake_iceberg::{DataFile, PlanError, Target, duckdb_sql, plan};

use crate::catalog::{Catalog, CatalogError};
use crate::export::{ExportError, read_histories};

/// Why the query could not be written.
#[derive(Debug, thiserror::Error)]
pub enum SqlError {
    /// The histories cannot be read.
    #[error(transparent)]
    Histories(#[from] ExportError),
    /// The declarations cannot be read.
    #[error(transparent)]
    Catalog(#[from] CatalogError),
    /// No history of the table.
    #[error("There is no history of {0} under evidence/histories/.")]
    UnknownTable(String),
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
/// offset), or a day, which is its midnight at +10:00.
pub fn parse_time(s: &str) -> Option<i64> {
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
        return Some((day - 36_000) * 1_000_000);
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

fn time(s: &str) -> Result<i64, SqlError> {
    parse_time(s).ok_or_else(|| SqlError::Time(s.to_owned()))
}

/// The DuckDB query reading `[from, to)` of `table`'s files listed in
/// `files`, projected onto schema `as_of` or the latest; the table's
/// history and time column are read under `root`.
pub fn sql(
    root: &Path,
    table: &str,
    files: &Path,
    (from, to): (&str, &str),
    as_of: Option<u32>,
) -> Result<String, SqlError> {
    let mut histories = read_histories(root)?;
    let h = histories
        .remove(table)
        .ok_or_else(|| SqlError::UnknownTable(table.to_owned()))?;
    let catalog = Catalog::read(&root.join("tables"))?;
    let bad = |message: String| SqlError::Files {
        path: files.display().to_string(),
        message,
    };
    let text = std::fs::read(files).map_err(|e| bad(e.to_string()))?;
    let listed: Vec<Listed> = serde_json::from_slice(&text).map_err(|e| bad(e.to_string()))?;
    let interval = (time(from)?, time(to)?);
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
    let reads = plan(&h.schemas, &data, interval, target).map_err(|source| SqlError::Plan {
        table: table.to_owned(),
        source,
    })?;
    let column = catalog.table(table).and_then(|t| t.time_column.as_deref());
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
        let day = parse_time("2024-08-01").unwrap();
        assert_eq!(day, parse_time("2024-08-01T00:00:00+10:00").unwrap());
        assert_eq!(day, parse_time("2024-07-31 14:00:00Z").unwrap());
        assert_eq!(day, 1_722_434_400_000_000);
        assert_eq!(parse_time("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_time("1969-12-31T23:59:59.5-00:00"), Some(-500_000));
        assert_eq!(
            parse_time("2024-02-29t01:02:03.1234567z"),
            Some(1_709_168_523_123_456)
        );
        assert_eq!(
            parse_time("2000-03-01T00:00:00Z"),
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
            assert_eq!(parse_time(bad), None, "{bad}");
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
        let q = |from, to, as_of| sql(root, "DISPATCH_UNIT_SCADA", &files, (from, to), as_of);
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
                "NONE",
                &files,
                ("2024-08-01", "2024-08-02"),
                None
            )),
            "There is no history of NONE under evidence/histories/."
        );
        std::fs::write(&files, r#"[{"path": 1}]"#).unwrap();
        assert!(e(q("2024-08-01", "2024-08-02", None)).starts_with("The file list "));
        std::fs::remove_file(&files).unwrap();
        assert!(e(q("2024-08-01", "2024-08-02", None)).starts_with("The file list "));
        std::fs::write(root.join("tables/BAD.toml"), "x").unwrap();
        assert!(e(q("2024-08-01", "2024-08-02", None)).contains("BAD.toml"));
        std::fs::write(crate::fold::history_path(root, "DISPATCH_UNIT_SCADA"), "[").unwrap();
        assert!(e(q("2024-08-01", "2024-08-02", None)).contains("DISPATCH_UNIT_SCADA.json"));
    }
}
