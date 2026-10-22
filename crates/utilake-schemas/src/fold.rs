//! Every table's history, folded from every census file.
//!
//! The fold reads every month's census (`evidence/census/{YYYY-MM}.toml`),
//! every report folder's (`evidence/census/reports/**`), the data-model
//! releases (`evidence/models/`), the declarations (`tables/`) and the
//! histories last published (`nem/{TABLE}/metadata/v1.metadata.json`),
//! and writes each table's history ([`crate::history`]) from scratch to
//! `evidence/histories/{TABLE}.json`: pretty JSON with its keys sorted, so
//! the same evidence writes the same bytes. Each file is written whole,
//! through a temporary file renamed into place, and a history of a table
//! no census holds any more is removed.
//!
//! Once, a fold may be taken with no published history to keep
//! (`rebaseline`): every id is minted anew, and
//! `evidence/histories/BASELINE.toml` records the data version that
//! began and why ([`Baseline`]). The export carries the version into
//! `manifest.json`, and `compat` holds a version only to the same version.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Deserialize as _;
use utilake_iceberg::table::TableMetadata;
use utilake_iceberg::{History, from_metadata};

use crate::catalog::Catalog;
use crate::census::{CensusError, FileRecord, census_dir, files_under, read_month};
use crate::history::{Declarations, HistoryError, Numeric, Observations};
use crate::models::{ModelError, ModelType, by_first_seen, read_models};
use crate::month::YearMonth;
use crate::reports::{ROOTS, kind, read_folder};
use utilake_iceberg::Market;

/// Why the fold could not be done.
#[derive(Debug, thiserror::Error)]
pub enum FoldError {
    /// A census file that cannot be read.
    #[error(transparent)]
    Census(#[from] CensusError),
    /// A model file that cannot be read.
    #[error(transparent)]
    Models(#[from] ModelError),
    /// A table whose history cannot be folded.
    #[error(transparent)]
    History(#[from] HistoryError),
    /// A published history that cannot be read.
    #[error("The published history {path} cannot be read: {message}")]
    Published {
        /// The file.
        path: String,
        /// What is wrong.
        message: String,
    },
    /// A history file that cannot be written or removed.
    #[error("The history file {path} could not be written.")]
    Write {
        /// The file.
        path: String,
        /// Cause.
        #[source]
        source: std::io::Error,
    },
}

/// The data version the histories are of: raised, with a reason, by the
/// one fold that keeps no published history. `evidence/histories/BASELINE.toml`,
/// absent for version 1.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Baseline {
    /// The data version: 1 before any re-baseline, one more each time.
    pub version: u32,
    /// Why the published history was not kept.
    pub reason: String,
}

impl Baseline {
    /// The version of a root with no record.
    pub const FIRST: Self = Self {
        version: 1,
        reason: String::new(),
    };

    /// The record of `market` under `root`, or the first version when
    /// there is none.
    pub fn read(root: &Path, market: Market) -> Result<Self, FoldError> {
        let path = baseline_path(root, market);
        let bad = |message: String| FoldError::Published {
            path: path.display().to_string(),
            message,
        };
        match std::fs::read_to_string(&path) {
            Ok(text) => toml::from_str(&text).map_err(|e| bad(e.to_string())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::FIRST),
            Err(e) => Err(bad(e.to_string())),
        }
    }

    /// The record as its file holds it.
    pub fn render(&self) -> String {
        toml::to_string(self).expect("a baseline serialises")
    }
}

/// The baseline record of a market.
pub fn baseline_path(root: &Path, market: Market) -> PathBuf {
    histories_dir(root, market).join("BASELINE.toml")
}

/// What a fold found, and what it wrote or would write.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Folded {
    /// Tables with a history.
    pub tables: usize,
    /// Iceberg schemas over every table.
    pub schemas: usize,
    /// Runs over every table.
    pub runs: usize,
    /// Breaking columns over every table.
    pub breaks: usize,
    /// History files whose bytes changed, or were removed.
    pub changed: usize,
}

/// AEMO's `YYYY/MM/DD[ HH:MM:SS[.f]]` as RFC 3339 in market time.
pub fn market_time(text: &str) -> Option<String> {
    let b = text.as_bytes();
    let digits =
        |r: std::ops::Range<usize>| b.get(r).is_some_and(|s| s.iter().all(u8::is_ascii_digit));
    if !(b.len() >= 10
        && digits(0..4)
        && b[4] == b'/'
        && digits(5..7)
        && b[7] == b'/'
        && digits(8..10))
    {
        return None;
    }
    let date = format!("{}-{}-{}", &text[0..4], &text[5..7], &text[8..10]);
    let time = match &text[10..] {
        "" => "00:00:00",
        t => t.strip_prefix(' ').filter(|t| t.len() >= 8)?,
    };
    Some(format!("{date}T{time}+10:00"))
}

/// The directory of a market's histories under the repository root:
/// `evidence/histories/{market}/`.
pub fn histories_dir(root: &Path, market: Market) -> PathBuf {
    root.join("evidence").join("histories").join(market.id())
}

/// The history file of a table.
pub fn history_path(root: &Path, market: Market, table: &str) -> PathBuf {
    histories_dir(root, market).join(format!("{table}.json"))
}

/// The table metadata of the previous export:
/// `{market}/{TABLE}/metadata/v1.metadata.json`.
pub fn published_path(root: &Path, market: Market, table: &str) -> PathBuf {
    root.join(market.id())
        .join(table)
        .join(crate::export::METADATA)
}

fn census_read(path: &Path, e: &std::io::Error) -> CensusError {
    CensusError::Read {
        path: path.display().to_string(),
        message: e.to_string(),
    }
}

/// The entries of a directory, in name order; none when there is none.
fn entries(dir: &Path) -> std::io::Result<Vec<PathBuf>> {
    let mut out = match std::fs::read_dir(dir) {
        Ok(rd) => rd
            .map(|e| e.map(|e| e.path()))
            .collect::<std::io::Result<Vec<_>>>()?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(e) => return Err(e),
    };
    out.sort();
    Ok(out)
}

/// The months a census was recorded for, from the census directory.
pub fn recorded_months(root: &Path, market: Market) -> Result<Vec<YearMonth>, CensusError> {
    let dir = census_dir(root, market);
    let months = entries(&dir)
        .map_err(|e| census_read(&dir, &e))?
        .iter()
        .filter_map(|p| p.file_name()?.to_str()?.strip_suffix(".toml")?.parse().ok())
        .collect();
    Ok(months)
}

/// The folders of report root `root` whose census was recorded, as
/// [`crate::reports::folder_path`] names them.
pub fn recorded_folders(
    repo: &Path,
    market: Market,
    root: &str,
) -> Result<Vec<String>, CensusError> {
    let dir = census_dir(repo, market).join("reports").join(root);
    if !dir.is_dir() {
        return Ok(Vec::new());
    }
    let folders = files_under(&dir)?
        .iter()
        .filter_map(|p| {
            let rel = p.strip_prefix(&dir).ok()?.to_str()?.strip_suffix(".toml")?;
            let rel = rel.replace('\\', "/");
            Some(if rel == "_root" { String::new() } else { rel })
        })
        .collect();
    Ok(folders)
}

/// Every block of every census recorded, by the table the catalog says.
pub fn observe(
    root: &Path,
    market: Market,
    catalog: &Catalog,
) -> Result<Observations, CensusError> {
    let mut obs = Observations::default();
    for month in recorded_months(root, market)? {
        let files = read_month(root, market, month)?.unwrap_or_default();
        for (file, f) in &files {
            for (i, b) in f.block.iter().enumerate() {
                let table = catalog.table_of(&b.report.0, &b.report.1);
                let index = u32::try_from(i).unwrap_or(u32::MAX);
                obs.block(&table, month, (file, index), f, b);
            }
        }
    }
    for root_name in ROOTS {
        for rel in recorded_folders(root, market, root_name)? {
            let census = read_folder(root, market, root_name, &rel)?.unwrap_or_default();
            // A folder's files are not hashed one by one: a run's marks
            // name its first and last file, with theirs.
            let file = FileRecord {
                source: format!("Reports/{root_name}/{rel}"),
                sha256: String::new(),
                size: 0,
                kind: kind(root_name),
                bad_rows: 0,
                footer_mismatches: 0,
                block: Vec::new(),
            };
            for t in &census.blocks {
                let b = &t.block;
                let table = catalog.table_of(&b.report.0, &b.report.1);
                // Records from before the files were hashed name the CSVs.
                let named = |file: &str, csv: &'_ str| {
                    if file.is_empty() {
                        csv.to_owned()
                    } else {
                        file.to_owned()
                    }
                };
                let (first, last) = (
                    named(&t.first_file, &t.first_csv),
                    named(&t.last_file, &t.last_csv),
                );
                let files = (
                    (first.as_str(), t.first_sha256.as_str()),
                    (last.as_str(), t.last_sha256.as_str()),
                    t.csvs,
                );
                obs.run(&table, t.month, files, &file, b);
            }
        }
    }
    Ok(obs)
}

/// What is declared beyond the control files: each declared table's key
/// and overrides, and the numeric columns of every data-model release
/// recorded.
pub fn declarations(
    root: &Path,
    market: Market,
    catalog: &Catalog,
) -> Result<Declarations, ModelError> {
    let models = read_models(root, market)?;
    let releases = by_first_seen(&models)
        .into_iter()
        .map(|m| {
            let tables = m
                .tables
                .iter()
                .map(|(name, t)| {
                    let numeric: Numeric = t
                        .columns
                        .iter()
                        .filter_map(|(n, ty, ..)| match ModelType::parse(ty) {
                            ModelType::Numeric(p, s) => Some((n.to_ascii_uppercase(), (p, s))),
                            _ => None,
                        })
                        .collect();
                    (name.clone(), numeric)
                })
                .collect();
            (m.first_seen, tables)
        })
        .collect();
    let keys = catalog
        .tables()
        .iter()
        .map(|t| (t.name.clone(), t.primary_key.clone()))
        .collect();
    let overrides = catalog
        .tables()
        .iter()
        .filter(|t| !t.overrides.is_empty())
        .map(|t| {
            let typed = t
                .overrides
                .iter()
                .filter_map(|o| Some((o.column.clone(), o.iceberg_type()?)))
                .collect();
            (t.name.clone(), typed)
        })
        .collect();
    Ok(Declarations {
        keys,
        overrides,
        releases,
    })
}

/// The histories the previous export published for `market`: every
/// `{market}/{TABLE}/metadata/v1.metadata.json`, by table
/// ([`utilake_iceberg::from_metadata`]). A directory without one
/// published no history. Metadata from before the format carried the
/// history (no `utilake.format`) is read from the `iceberg` block of the
/// `table.json` beside it, so one fold can cross the format change.
pub fn published(root: &Path, market: Market) -> Result<BTreeMap<String, History>, FoldError> {
    let mut out = BTreeMap::new();
    let nem = root.join(market.id());
    let dirs = entries(&nem).map_err(|e| FoldError::Published {
        path: nem.display().to_string(),
        message: e.to_string(),
    })?;
    for dir in dirs.iter().filter(|d| d.is_dir()) {
        let path = dir.join(crate::export::METADATA);
        let bad = |path: &Path, message: String| FoldError::Published {
            path: path.display().to_string(),
            message,
        };
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(bad(&path, e.to_string())),
        };
        let m: TableMetadata =
            serde_json::from_str(&text).map_err(|e| bad(&path, e.to_string()))?;
        if !m.properties.contains_key("utilake.format") {
            let legacy = dir.join("table.json");
            let doc: serde_json::Value = std::fs::read_to_string(&legacy)
                .map_err(|e| e.to_string())
                .and_then(|t| serde_json::from_str(&t).map_err(|e| e.to_string()))
                .map_err(|e| bad(&legacy, e))?;
            let Some(iceberg) = doc.get("iceberg") else {
                continue;
            };
            let h = History::deserialize(iceberg).map_err(|e| bad(&legacy, e.to_string()))?;
            out.insert(h.table.clone(), h);
            continue;
        }
        let p = from_metadata(&m).map_err(|e| bad(&path, e.to_string()))?;
        out.insert(p.history.table.clone(), p.history);
    }
    Ok(out)
}

/// A history as its file holds it: pretty JSON, keys sorted, a newline
/// at the end.
pub fn render(h: &History) -> String {
    let v = serde_json::to_value(h).expect("a history serialises");
    let mut text = serde_json::to_string_pretty(&v).expect("a JSON value prints");
    text.push('\n');
    text
}

fn write_error(path: &Path, source: std::io::Error) -> FoldError {
    FoldError::Write {
        path: path.display().to_string(),
        source,
    }
}

/// Folds every recorded census into each table's history and writes
/// `evidence/histories/`, or with `check` only counts the files that
/// would change. With `rebaseline`, the reason, no published history is
/// kept, and the baseline record is raised to the next data version.
pub fn fold(
    root: &Path,
    market: Market,
    catalog: &Catalog,
    check: bool,
    rebaseline: Option<&str>,
) -> Result<Folded, FoldError> {
    let obs = observe(root, market, catalog)?;
    let floor = match rebaseline {
        Some(_) => BTreeMap::new(),
        None => published(root, market)?,
    };
    let tables = obs.finish(&declarations(root, market, catalog)?, &floor)?;
    let mut folded = Folded {
        tables: tables.len(),
        ..Folded::default()
    };
    if let Some(reason) = rebaseline {
        let next = Baseline {
            version: Baseline::read(root, market)?.version + 1,
            reason: reason.to_owned(),
        };
        let path = baseline_path(root, market);
        folded.changed += 1;
        if !check {
            crate::files::write_atomic(&path, next.render().as_bytes())
                .map_err(|e| write_error(&path, e))?;
        }
    }
    let dir = histories_dir(root, market);
    for stale in entries(&dir).map_err(|e| write_error(&dir, e))? {
        let table = stale
            .file_name()
            .and_then(|n| n.to_str()?.strip_suffix(".json"));
        if table.is_some_and(|t| !tables.contains_key(t)) {
            folded.changed += 1;
            if !check {
                std::fs::remove_file(&stale).map_err(|e| write_error(&stale, e))?;
            }
        }
    }
    for (table, h) in &tables {
        folded.schemas += h.schemas.len();
        folded.runs += h.runs.len();
        folded.breaks += h.breaking.len();
        let path = history_path(root, market, table);
        let text = render(h);
        if std::fs::read(&path).ok().as_deref() == Some(text.as_bytes()) {
            continue;
        }
        folded.changed += 1;
        if !check {
            crate::files::write_atomic(&path, text.as_bytes())
                .map_err(|e| write_error(&path, e))?;
        }
    }
    Ok(folded)
}

#[cfg(test)]
mod tests;
