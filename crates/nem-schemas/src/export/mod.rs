//! `export`: every table's history as the documents under `nem/`.
//!
//! Per table with a history (`evidence/histories/{TABLE}.json`),
//! `nem/{TABLE}/table.json` describes the table: its declaration from
//! `tables/`, its description and each column's comment and model type
//! from the latest data-model release under `evidence/models/` that has
//! them, and its history as an Iceberg table (`iceberg`), each schema's
//! document named in `schema-files`. Per schema, `nem/{TABLE}/schema-{hash}.json`
//! is a JSON Schema of a row ([`schema`]). Beside them, `manifest.json`
//! names every file under `nem/` with its SHA-256.
//!
//! Everything is a function of the files read: no clock, no git. Every
//! document is pretty JSON with its keys sorted and a newline at the end,
//! so the same evidence writes the same bytes. The export owns `nem/`
//! whole: a file there it did not write is removed ([`write()`]), or
//! reported by a check ([`check`]).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use nem_iceberg::History;
use serde_json::{Value, json};

use crate::catalog::{Catalog, CatalogError, TableDecl};
use crate::fold::histories_dir;
use crate::models::{Model, ModelError, ModelTable, by_first_seen, read_models};

pub mod schema;
mod write;

pub use write::{Stale, Written, check, write};

/// The generation of the documents' layout: the files, the `x-` keys and
/// their meaning. Raised when a reader of this format would misread a
/// document.
pub const FORMAT: u32 = 2;

/// Where the documents are published: a document's `$id` is this and its
/// path.
pub const DEFAULT_BASE_URL: &str = "https://raw.githubusercontent.com/pigface-io/nem-schemas/main/";

/// The directory the export owns.
pub const OWNED: &str = "nem";

/// Why the export could not be done.
#[derive(Debug, thiserror::Error)]
pub enum ExportError {
    /// The declarations cannot be read.
    #[error(transparent)]
    Catalog(#[from] CatalogError),
    /// A data-model release cannot be read.
    #[error(transparent)]
    Models(#[from] ModelError),
    /// A history file that cannot be read.
    #[error("The history {path} cannot be read: {message}")]
    History {
        /// The file.
        path: String,
        /// What is wrong.
        message: String,
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

/// Everything the export reads, loaded.
#[derive(Debug)]
pub struct Export {
    histories: BTreeMap<String, History>,
    catalog: Catalog,
    /// The data-model releases, first shipped first.
    models: Vec<Model>,
    base_url: String,
}

/// Every file one export writes, by path relative to the root.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Rendered {
    /// Path to text: every `nem/` document and `manifest.json`.
    pub files: BTreeMap<String, String>,
    /// Tables written.
    pub tables: usize,
    /// Schema documents written.
    pub schemas: usize,
}

impl Export {
    /// Reads the histories, the declarations and the data-model releases
    /// under `root`; the documents will live under `base_url`.
    pub fn load(root: &Path, base_url: &str) -> Result<Self, ExportError> {
        Ok(Self {
            histories: read_histories(root)?,
            catalog: Catalog::read(&root.join("tables"))?,
            models: by_first_seen(&read_models(root)?)
                .into_iter()
                .cloned()
                .collect(),
            base_url: base_url.to_owned(),
        })
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

    /// Renders every file.
    pub fn render(&self) -> Rendered {
        let mut out = Rendered::default();
        for (table, h) in &self.histories {
            let mut files = BTreeMap::new();
            for s in &h.schemas {
                let name = schema::file_name(s);
                let path = format!("{OWNED}/{table}/{name}");
                let doc = schema::document(&self.base_url, table, &path, s);
                out.files.insert(path, doc);
                files.insert(s.schema_id.to_string(), json!(name));
            }
            out.schemas += h.schemas.len();
            let doc = self.table(h, self.catalog.table(table), &files);
            out.files
                .insert(format!("{OWNED}/{table}/table.json"), text(&doc));
        }
        out.tables = self.histories.len();
        let manifest = manifest(&self.base_url, &out);
        out.files.insert("manifest.json".to_owned(), manifest);
        out
    }

    /// The table's document.
    fn table(
        &self,
        h: &History,
        decl: Option<&TableDecl>,
        files: &BTreeMap<String, Value>,
    ) -> Value {
        let name = h.table.as_str();
        let columns: Vec<Value> = h
            .fields()
            .iter()
            .map(|f| {
                let (comment, model_type) = self.column(name, &f.name);
                json!({"id": f.id, "name": f.name, "comment": comment, "model_type": model_type})
            })
            .collect();
        let mut iceberg = serde_json::to_value(h).expect("a history serialises");
        iceberg["schema-files"] = json!(files);
        let description = self.model(name).and_then(|t| non_empty(&t.comment));
        json!({
            "name": name,
            "x-format": FORMAT,
            "declared": decl.is_some(),
            "description": description,
            "package": decl.and_then(|d| d.package.as_deref()),
            "primary_key": decl.map(|d| &d.primary_key),
            "time_column": decl.and_then(|d| d.time_column.as_deref()),
            "temporal": decl.map(|d| serde_json::to_value(&d.temporal).unwrap_or(Value::Null)),
            "load_mode": decl.map(|d| d.load_mode),
            "visibility": decl.map(|d| &d.visibility),
            "columns": columns,
            "iceberg": iceberg,
        })
    }
}

fn non_empty(s: &str) -> Option<&str> {
    Some(s).filter(|s| !s.is_empty())
}

/// The manifest: what was written, and the SHA-256 of every file under
/// `nem/`.
fn manifest(base_url: &str, r: &Rendered) -> String {
    let hashes: BTreeMap<&str, String> = r
        .files
        .iter()
        .map(|(p, t)| (p.as_str(), crate::ctl::sha256(t.as_bytes())))
        .collect();
    text(&json!({
        "format": FORMAT,
        "market": "nem",
        "base_url": base_url,
        "tables": r.tables,
        "schemas": r.schemas,
        "sha256": hashes,
    }))
}

#[cfg(test)]
pub(crate) mod tests;
