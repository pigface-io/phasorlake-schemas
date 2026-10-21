//! Every table's history, folded from every census file.
//!
//! The fold reads every month's census (`evidence/census/{YYYY-MM}.toml`),
//! every report folder's (`evidence/census/reports/**`), the data-model
//! releases (`evidence/models/`), the declarations (`tables/`) and the
//! histories last published (the `iceberg` block of `nem/{TABLE}/table.json`),
//! and writes each table's history ([`crate::history`]) from scratch to
//! `evidence/histories/{TABLE}.json`: pretty JSON with its keys sorted, so
//! the same evidence writes the same bytes. Each file is written whole,
//! through a temporary file renamed into place, and a history of a table
//! no census holds any more is removed.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use nem_iceberg::History;
use serde::Deserialize as _;

use crate::catalog::Catalog;
use crate::census::{CensusError, FileRecord, census_dir, files_under, read_month};
use crate::history::{Declarations, HistoryError, Numeric, Observations};
use crate::models::{ModelError, ModelType, by_first_seen, read_models};
use crate::month::YearMonth;
use crate::reports::{ROOTS, kind, read_folder};

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

/// The directory of the histories under the repository root.
pub fn histories_dir(root: &Path) -> PathBuf {
    root.join("evidence").join("histories")
}

/// The history file of a table.
pub fn history_path(root: &Path, table: &str) -> PathBuf {
    histories_dir(root).join(format!("{table}.json"))
}

/// The table document of the previous export.
pub fn published_path(root: &Path, table: &str) -> PathBuf {
    root.join("nem").join(table).join("table.json")
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
pub fn recorded_months(root: &Path) -> Result<Vec<YearMonth>, CensusError> {
    let dir = census_dir(root);
    let months = entries(&dir)
        .map_err(|e| census_read(&dir, &e))?
        .iter()
        .filter_map(|p| p.file_name()?.to_str()?.strip_suffix(".toml")?.parse().ok())
        .collect();
    Ok(months)
}

/// The folders of report root `root` whose census was recorded, as
/// [`crate::reports::folder_path`] names them.
pub fn recorded_folders(repo: &Path, root: &str) -> Result<Vec<String>, CensusError> {
    let dir = census_dir(repo).join("reports").join(root);
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
pub fn observe(root: &Path, catalog: &Catalog) -> Result<Observations, CensusError> {
    let mut obs = Observations::default();
    for month in recorded_months(root)? {
        let files = read_month(root, month)?.unwrap_or_default();
        for (file, f) in &files {
            for (i, b) in f.block.iter().enumerate() {
                let table = catalog.table_of(&b.report.0, &b.report.1);
                let index = u32::try_from(i).unwrap_or(u32::MAX);
                obs.block(&table, month, (file, index), f, b);
            }
        }
    }
    for root_name in ROOTS {
        for rel in recorded_folders(root, root_name)? {
            let census = read_folder(root, root_name, &rel)?.unwrap_or_default();
            // A folder's CSVs are not hashed one by one: its runs' marks
            // carry no SHA-256.
            let file = FileRecord {
                source: format!("Reports/{root_name}/{rel}"),
                sha256: String::new(),
                size: 0,
                kind: kind(root_name),
                block: Vec::new(),
            };
            for t in &census.blocks {
                let b = &t.block;
                let table = catalog.table_of(&b.report.0, &b.report.1);
                let files = (t.first_csv.as_str(), t.last_csv.as_str(), t.csvs);
                obs.run(&table, t.month, files, &file, b);
            }
        }
    }
    Ok(obs)
}

/// What is declared beyond the control files: each declared table's key
/// and overrides, and the numeric columns of every data-model release
/// recorded.
pub fn declarations(root: &Path, catalog: &Catalog) -> Result<Declarations, ModelError> {
    let models = read_models(root)?;
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

/// The histories the previous export published: the `iceberg` block of
/// every `nem/{TABLE}/table.json`, by table. A document without one
/// published no history.
pub fn published(root: &Path) -> Result<BTreeMap<String, History>, FoldError> {
    let mut out = BTreeMap::new();
    let nem = root.join("nem");
    let dirs = entries(&nem).map_err(|e| FoldError::Published {
        path: nem.display().to_string(),
        message: e.to_string(),
    })?;
    for dir in dirs.iter().filter(|d| d.is_dir()) {
        let path = dir.join("table.json");
        let bad = |message: String| FoldError::Published {
            path: path.display().to_string(),
            message,
        };
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(bad(e.to_string())),
        };
        let doc: serde_json::Value = serde_json::from_str(&text).map_err(|e| bad(e.to_string()))?;
        let Some(iceberg) = doc.get("iceberg") else {
            continue;
        };
        let h = History::deserialize(iceberg).map_err(|e| bad(e.to_string()))?;
        out.insert(h.table.clone(), h);
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
/// would change.
pub fn fold(root: &Path, catalog: &Catalog, check: bool) -> Result<Folded, FoldError> {
    let obs = observe(root, catalog)?;
    let tables = obs.finish(&declarations(root, catalog)?, &published(root)?)?;
    let mut folded = Folded {
        tables: tables.len(),
        ..Folded::default()
    };
    let dir = histories_dir(root);
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
        let path = history_path(root, table);
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
