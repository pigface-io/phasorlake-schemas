//! `export`: every table's history as the documents under `nem/`.
//!
//! Per table with a history (`evidence/histories/{TABLE}.json`), the
//! table is published as Iceberg table metadata,
//! `nem/{TABLE}/metadata/v1.metadata.json` ([`utilake_iceberg::publish`]):
//! its schemas, its runs as snapshots, its declaration from `tables/`,
//! and its description, each column's comment and model type from the
//! latest data-model release under `evidence/models/` that has them.
//! Beside it, `version-hint.text` names version 1, and one empty manifest
//! list per snapshot lets any Iceberg reader open the table and read no
//! rows. Everything else is derived from the metadata, read back through
//! [`utilake_iceberg::from_metadata`], never from the history: per schema,
//! `nem/{TABLE}/schema-{id}.json` is a JSON Schema of a row ([`schema`]);
//! `nem/headers.json` maps every header AEMO's files carry (report pair,
//! version, columns) to its table and schemas; `manifest.json` names every
//! file under `nem/` with its SHA-256, and lists the tables.
//!
//! Everything is a function of the files read: no clock, no git. Every
//! document is pretty JSON with its keys sorted and a newline at the end,
//! and the manifest lists are bytes of the table's content, so the same
//! evidence writes the same bytes. The export owns `nem/` whole: a file
//! there it did not write is removed ([`write()`]), or reported by a check
//! ([`check`]).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use utilake_iceberg::publish::{Declaration, PublishError, Published, Table, to_metadata};
use utilake_iceberg::{History, from_metadata};

use crate::catalog::{Catalog, CatalogError, TableDecl};
use crate::fold::{Baseline, FoldError, histories_dir};
use crate::models::{Model, ModelError, ModelTable, by_first_seen, read_models};

pub mod schema;
mod write;

pub use write::{Stale, Written, check, write};

/// The generation of the documents' layout: the files, the `x-` keys and
/// their meaning. Raised when a reader of this format would misread a
/// document. It is the published layout's ([`utilake_iceberg::publish::FORMAT`]).
pub const FORMAT: u32 = utilake_iceberg::publish::FORMAT;

/// Where the documents are published: a document's `$id` is this and its
/// path.
pub const DEFAULT_BASE_URL: &str = "https://raw.githubusercontent.com/g0itb4/utilake-schemas/main/";

/// The directory the export owns.
pub const OWNED: &str = "nem";

/// The table's metadata file under its directory.
pub const METADATA: &str = "metadata/v1.metadata.json";

/// Why the export could not be done.
#[derive(Debug, thiserror::Error)]
pub enum ExportError {
    /// The declarations cannot be read.
    #[error(transparent)]
    Catalog(#[from] CatalogError),
    /// A data-model release cannot be read.
    #[error(transparent)]
    Models(#[from] ModelError),
    /// The baseline record that cannot be read.
    #[error(transparent)]
    Baseline(#[from] FoldError),
    /// A history file that cannot be read.
    #[error("The history {path} cannot be read: {message}")]
    History {
        /// The file.
        path: String,
        /// What is wrong.
        message: String,
    },
    /// A declaration the published form cannot hold.
    #[error("The declaration of {table} cannot be published: {message}")]
    Declaration {
        /// The table.
        table: String,
        /// What is wrong.
        message: String,
    },
    /// A history that cannot be written as table metadata.
    #[error("The history of {table} cannot be published: {source}")]
    Publish {
        /// The table.
        table: String,
        /// Cause.
        #[source]
        source: PublishError,
    },
    /// A file that cannot be read, written or removed.
    #[error("The export could not read or write {path}.")]
    Io {
        /// The file or directory.
        path: String,
        /// Cause.
        #[source]
        source: std::io::Error,
    },
}

impl ExportError {
    pub(crate) fn io(path: &Path, source: std::io::Error) -> Self {
        Self::Io {
            path: path.display().to_string(),
            source,
        }
    }
}

/// A document as the export writes it: pretty JSON, keys sorted, a newline
/// at the end.
pub fn text(v: &Value) -> String {
    let mut s = serde_json::to_string_pretty(v).expect("a JSON value prints");
    s.push('\n');
    s
}

/// Every history under `evidence/histories/`, by table. A file must be
/// named for its table; a file that is not `.json` is not a history.
pub fn read_histories(root: &Path) -> Result<BTreeMap<String, History>, ExportError> {
    let dir = histories_dir(root);
    let mut paths: Vec<PathBuf> = match std::fs::read_dir(&dir) {
        Ok(rd) => rd
            .map(|e| e.map(|e| e.path()))
            .collect::<std::io::Result<_>>()
            .map_err(|e| ExportError::io(&dir, e))?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(e) => return Err(ExportError::io(&dir, e)),
    };
    paths.retain(|p| p.extension().is_some_and(|e| e == "json"));
    paths.sort();
    let mut out = BTreeMap::new();
    for path in paths {
        let bad = |message: String| ExportError::History {
            path: path.display().to_string(),
            message,
        };
        let bytes = std::fs::read(&path).map_err(|e| bad(e.to_string()))?;
        let h: History = serde_json::from_slice(&bytes).map_err(|e| bad(e.to_string()))?;
        let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned());
        if stem.as_deref() != Some(h.table.as_str()) {
            return Err(bad(format!(
                "it is the history of {}, not of its file name",
                h.table
            )));
        }
        out.insert(h.table.clone(), h);
    }
    Ok(out)
}

/// A table's declaration in its published form; the temporal roles must
/// be the ones the form names.
fn declaration(d: &TableDecl) -> Result<Declaration, ExportError> {
    let bad = |message: String| ExportError::Declaration {
        table: d.name.clone(),
        message,
    };
    let temporal = serde_json::to_value(&d.temporal).map_err(|e| bad(e.to_string()))?;
    let load_mode = serde_json::to_value(d.load_mode).map_err(|e| bad(e.to_string()))?;
    Ok(Declaration {
        package: d.package.clone(),
        report_pairs: d.report_pairs.clone(),
        primary_key: d.primary_key.clone(),
        time_column: d.time_column.clone(),
        load_mode: load_mode
            .as_str()
            .ok_or_else(|| bad("the load mode is not a word".into()))?
            .to_owned(),
        visibility: d.visibility.clone(),
        temporal: serde_json::from_value(temporal).map_err(|e| {
            bad(format!(
                "its temporal roles are not the published ones: {e}"
            ))
        })?,
    })
}

/// Everything the export reads, loaded.
#[derive(Debug)]
pub struct Export {
    histories: BTreeMap<String, History>,
    /// Each declared table's declaration in its published form.
    declarations: BTreeMap<String, Declaration>,
    /// The data-model releases, first shipped first.
    models: Vec<Model>,
    base_url: String,
    /// The data version of the histories.
    baseline: Baseline,
}

/// Every file one export writes, by path relative to the root.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Rendered {
    /// Path to bytes: every file under `nem/` and `manifest.json`.
    pub files: BTreeMap<String, Vec<u8>>,
    /// Tables written.
    pub tables: usize,
    /// Schema documents written.
    pub schemas: usize,
}

impl Export {
    /// Reads the histories, the declarations and the data-model releases
    /// under `root`; the documents will live under `base_url`.
    pub fn load(root: &Path, base_url: &str) -> Result<Self, ExportError> {
        let catalog = Catalog::read(&root.join("tables"))?;
        let declarations = catalog
            .tables()
            .iter()
            .map(|d| Ok((d.name.clone(), declaration(d)?)))
            .collect::<Result<_, ExportError>>()?;
        Ok(Self {
            histories: read_histories(root)?,
            declarations,
            models: by_first_seen(&read_models(root)?)
                .into_iter()
                .cloned()
                .collect(),
            base_url: base_url.to_owned(),
            baseline: Baseline::read(root)?,
        })
    }

    /// The tables with a history, in order.
    pub fn tables(&self) -> impl Iterator<Item = &str> {
        self.histories.keys().map(String::as_str)
    }

    /// The newest release's declaration of `table`.
    fn model(&self, table: &str) -> Option<&ModelTable> {
        self.models.iter().rev().find_map(|m| m.tables.get(table))
    }

    /// The comment and type the newest release with the column gives it.
    fn column(&self, table: &str, column: &str) -> (Option<&str>, Option<&str>) {
        self.models
            .iter()
            .rev()
            .filter_map(|m| m.tables.get(table))
            .find_map(|t| t.columns.iter().find(|c| c.0.eq_ignore_ascii_case(column)))
            .map_or((None, None), |(_, ty, _, comment)| {
                (non_empty(comment), Some(ty.as_str()))
            })
    }

    /// `table` as published: its history, its declaration, and what the
    /// data-model releases say of it and its columns.
    pub fn published(&self, table: &str) -> Option<Published> {
        let h = self.histories.get(table)?;
        let mut docs = BTreeMap::new();
        let mut model_types = BTreeMap::new();
        for f in h.fields() {
            let (comment, model_type) = self.column(table, &f.name);
            if let Some(c) = comment {
                docs.insert(f.id, c.to_owned());
            }
            if let Some(t) = model_type {
                model_types.insert(f.id, t.to_owned());
            }
        }
        Some(Published {
            history: h.clone(),
            declaration: self.declarations.get(table).cloned(),
            description: self
                .model(table)
                .and_then(|t| non_empty(&t.comment))
                .map(str::to_owned),
            docs,
            model_types,
        })
    }

    /// `table` as Iceberg table metadata at `location`, with its manifest
    /// lists; none when it has no history.
    pub fn table(&self, table: &str, location: &str) -> Option<Result<Table, ExportError>> {
        let p = self.published(table)?;
        Some(
            to_metadata(&p, location, &crate::iceberg::table_uuid(table)).map_err(|source| {
                ExportError::Publish {
                    table: table.to_owned(),
                    source,
                }
            }),
        )
    }

    /// Renders every file.
    pub fn render(&self) -> Result<Rendered, ExportError> {
        let mut out = Rendered::default();
        let mut listed = Vec::new();
        let mut histories = Vec::new();
        for table in self.tables() {
            let dir = format!("{OWNED}/{table}");
            let location = format!("{}{dir}", self.base_url);
            let t = self
                .table(table, &location)
                .expect("the table has a history")?;
            out.files.insert(
                format!("{dir}/{METADATA}"),
                text(&serde_json::to_value(&t.metadata).expect("metadata serialises")).into(),
            );
            out.files
                .insert(format!("{dir}/metadata/version-hint.text"), b"1".to_vec());
            for ml in t.manifest_lists {
                out.files.insert(format!("{dir}/{}", ml.path), ml.bytes);
            }
            // Everything else is read back out of the metadata.
            let p = from_metadata(&t.metadata).expect("what was published reads back");
            debug_assert_eq!(p, self.published(table).expect("the table has a history"));
            for s in &p.history.schemas {
                let path = format!("{dir}/{}", schema::file_name(s.schema_id));
                let doc = schema::document(&self.base_url, table, &path, s, &p);
                out.files.insert(path, doc.into());
            }
            out.schemas += p.history.schemas.len();
            listed.push(listing(&p));
            histories.push(p.history);
        }
        out.tables = histories.len();
        out.files.insert(
            format!("{OWNED}/headers.json"),
            text(&headers(histories.iter())).into(),
        );
        let manifest = manifest(&self.base_url, &self.baseline, &listed, &out);
        out.files
            .insert("manifest.json".to_owned(), manifest.into());
        Ok(out)
    }
}

/// The first and last release month of the runs whose schema `has`;
/// none when no run's does.
fn span_of(h: &History, has: impl Fn(&utilake_iceberg::Schema) -> bool) -> (Value, Value) {
    let runs = h
        .runs
        .iter()
        .filter(|r| h.schema(r.schema_id).is_some_and(&has));
    let first = runs.clone().map(|r| r.from.release.as_str()).min();
    let last = runs.map(|r| r.to.release.as_str()).max();
    (json!(first), json!(last))
}

/// The table's line in the manifest: its name, package, whether it is
/// declared, the releases its runs span, and its counts.
fn listing(p: &Published) -> Value {
    let h = &p.history;
    let (first, last) = span_of(h, |_| true);
    json!({
        "name": h.table,
        "package": p.declaration.as_ref().and_then(|d| d.package.as_deref()),
        "declared": p.declaration.is_some(),
        "first_release": first,
        "last_release": last,
        "schemas": h.schemas.len(),
        "runs": h.runs.len(),
        "breaks": h.breaking.len(),
    })
}

/// `nem/headers.json`: every header AEMO's files carry, in the order
/// of report pair, version and columns, with its table and the schemas it
/// was read with, each with the releases of its runs. A reader of a file
/// resolves its header as [`History::resolve`] does: the last schema whose
/// first release is at or before the file's, else the first.
fn headers<'a>(histories: impl Iterator<Item = &'a History>) -> Value {
    /// A header: report pair, version, columns.
    type Header = ([String; 2], u16, Vec<String>);
    let mut out: BTreeMap<Header, (String, Vec<Value>)> = BTreeMap::new();
    for h in histories {
        for r in &h.runs {
            let Some(s) = h.schema(r.schema_id) else {
                continue;
            };
            let columns: Vec<String> = s.fields.iter().map(|f| f.name.clone()).collect();
            let key = (r.report.clone(), r.version, columns);
            let (_, schemas) = out
                .entry(key)
                .or_insert_with(|| (h.table.clone(), Vec::new()));
            let last = schemas
                .iter_mut()
                .find(|v| v["schema-id"] == json!(r.schema_id));
            match last {
                Some(v) => {
                    v["runs"] = json!(v["runs"].as_u64().unwrap_or(0) + 1);
                    let later = v["last-release"]
                        .as_str()
                        .is_none_or(|l| r.to.release.as_str() > l);
                    if later {
                        v["last-release"] = json!(r.to.release);
                    }
                }
                None => schemas.push(json!({
                    "schema-id": r.schema_id,
                    "first-release": r.from.release,
                    "last-release": r.to.release,
                    "runs": 1,
                })),
            }
        }
    }
    let headers: Vec<Value> = out
        .into_iter()
        .map(|((report, version, columns), (table, schemas))| {
            json!({
                "report": report,
                "version": version,
                "columns": columns,
                "table": table,
                "schemas": schemas,
            })
        })
        .collect();
    json!({"x-format": FORMAT, "headers": headers})
}

fn non_empty(s: &str) -> Option<&str> {
    Some(s).filter(|s| !s.is_empty())
}

/// The manifest: what was written, the data version (`baseline`: the
/// version, and the reason it was begun), each table's line
/// ([`listing`]) and the SHA-256 of every file under `nem/`.
fn manifest(base_url: &str, baseline: &Baseline, listed: &[Value], r: &Rendered) -> String {
    let hashes: BTreeMap<&str, String> = r
        .files
        .iter()
        .map(|(p, bytes)| (p.as_str(), crate::ctl::sha256(bytes)))
        .collect();
    text(&json!({
        "format": FORMAT,
        "market": "nem",
        "base_url": base_url,
        "baseline": {"version": baseline.version, "reason": baseline.reason},
        "tables": r.tables,
        "schemas": r.schemas,
        "listed": listed,
        "sha256": hashes,
    }))
}

#[cfg(test)]
pub(crate) mod tests;
