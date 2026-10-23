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
//! `{market}/folders.json` says which report folder of a feed carries each
//! block (report pair and version) and over which releases, from the folder
//! census: what a reader asks before fetching an interval of a table from the
//! report folders rather than the monthly archive.
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

use crate::catalog::{Catalog, CatalogError, TableDecl, tables_dir};
use crate::census::CensusError;
use crate::fold::{Baseline, FoldError, histories_dir, recorded_folders};
use crate::models::{Model, ModelError, ModelTable, by_first_seen, read_models};
use crate::reports::read_folder;
use utilake_iceberg::Market;
use utilake_plain_decode::feeds::{Feed, feeds_of};

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

/// The directories the export owns: one per market, `nem/`, `wem/`, `gas/`.
pub fn owned() -> impl Iterator<Item = &'static str> {
    Market::ALL.into_iter().map(Market::id)
}

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
    /// A folder census that cannot be read.
    #[error(transparent)]
    Census(#[from] CensusError),
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

/// Every history under `evidence/histories/{market}/`, by table. A file
/// must be named for its table; a file that is not `.json` is not a
/// history.
pub fn read_histories(
    root: &Path,
    market: Market,
) -> Result<BTreeMap<String, History>, ExportError> {
    let dir = histories_dir(root, market);
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

/// What the export reads of one market.
#[derive(Debug)]
struct MarketExport {
    histories: BTreeMap<String, History>,
    /// Each declared table's declaration in its published form.
    declarations: BTreeMap<String, Declaration>,
    /// The data-model tables each declared table names as its own words'
    /// source, when it names any.
    models_of: BTreeMap<String, Vec<String>>,
    /// The data-model releases, first shipped first.
    models: Vec<Model>,
    /// The data version of the histories.
    baseline: Baseline,
    /// Which report folder carries each block, from the folder census;
    /// empty when none is recorded.
    folders: Vec<Value>,
}

/// Everything the export reads, loaded: each market that has declarations
/// or histories.
#[derive(Debug)]
pub struct Export {
    markets: BTreeMap<Market, MarketExport>,
    base_url: String,
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

impl MarketExport {
    /// Reads one market's declarations, histories, models and baseline;
    /// none when the market has neither declarations nor histories.
    fn load(root: &Path, market: Market) -> Result<Option<Self>, ExportError> {
        let tables = tables_dir(root, market);
        let histories = histories_dir(root, market);
        if !tables.exists() && !histories.exists() {
            return Ok(None);
        }
        let (declarations, models_of) = if tables.exists() {
            let catalog = Catalog::read(&tables)?;
            let declarations = catalog
                .tables()
                .iter()
                .map(|d| Ok((d.name.clone(), declaration(d)?)))
                .collect::<Result<_, ExportError>>()?;
            let models_of = catalog
                .tables()
                .iter()
                .filter(|d| !d.model.is_empty())
                .map(|d| (d.name.clone(), d.model.clone()))
                .collect();
            (declarations, models_of)
        } else {
            (BTreeMap::new(), BTreeMap::new())
        };
        Ok(Some(Self {
            histories: read_histories(root, market)?,
            declarations,
            models_of,
            models: by_first_seen(&read_models(root, market)?)
                .into_iter()
                .cloned()
                .collect(),
            baseline: Baseline::read(root, market)?,
            folders: folders(root, market)?,
        }))
    }

    /// The data-model tables that say what `table`'s columns are: its
    /// own name, then the ones its declaration names.
    fn model_names<'a>(&'a self, table: &'a str) -> impl Iterator<Item = &'a str> {
        std::iter::once(table).chain(
            self.models_of
                .get(table)
                .into_iter()
                .flatten()
                .map(String::as_str),
        )
    }

    /// The newest release's declaration of `table`, under its own name or
    /// the first model table its declaration names.
    fn model(&self, table: &str) -> Option<&ModelTable> {
        self.model_names(table)
            .find_map(|name| self.models.iter().rev().find_map(|m| m.tables.get(name)))
    }

    /// The comment and type the newest release with the column gives it,
    /// in the table's own model table or the ones its declaration names.
    fn column(&self, table: &str, column: &str) -> (Option<&str>, Option<&str>) {
        self.model_names(table)
            .find_map(|name| {
                self.models
                    .iter()
                    .rev()
                    .filter_map(|m| m.tables.get(name))
                    .find_map(|t| t.columns.iter().find(|c| c.0.eq_ignore_ascii_case(column)))
            })
            .map_or((None, None), |(_, ty, _, comment)| {
                (non_empty(comment), Some(ty.as_str()))
            })
    }
}

impl Export {
    /// Reads the histories, the declarations and the data-model releases
    /// of every market under `root`; the documents will live under
    /// `base_url`.
    pub fn load(root: &Path, base_url: &str) -> Result<Self, ExportError> {
        let mut markets = BTreeMap::new();
        for market in Market::ALL {
            if let Some(m) = MarketExport::load(root, market)? {
                markets.insert(market, m);
            }
        }
        Ok(Self {
            markets,
            base_url: base_url.to_owned(),
        })
    }

    /// The tables with a history, by market then name.
    pub fn tables(&self) -> impl Iterator<Item = (Market, &str)> {
        self.markets
            .iter()
            .flat_map(|(m, e)| e.histories.keys().map(move |t| (*m, t.as_str())))
    }

    /// `table` of `market` as published: its history, its declaration, and
    /// what the data-model releases say of it and its columns.
    pub fn published(&self, market: Market, table: &str) -> Option<Published> {
        let e = self.markets.get(&market)?;
        let h = e.histories.get(table)?;
        let mut docs = BTreeMap::new();
        let mut model_types = BTreeMap::new();
        for f in h.fields() {
            let (comment, model_type) = e.column(table, &f.name);
            if let Some(c) = comment {
                docs.insert(f.id, c.to_owned());
            }
            if let Some(t) = model_type {
                model_types.insert(f.id, t.to_owned());
            }
        }
        Some(Published {
            market,
            history: h.clone(),
            declaration: e.declarations.get(table).cloned(),
            description: e
                .model(table)
                .and_then(|t| non_empty(&t.comment))
                .map(str::to_owned),
            docs,
            model_types,
            model: e.models_of.get(table).cloned().unwrap_or_default(),
        })
    }

    /// `table` of `market` as Iceberg table metadata at `location`, with
    /// its manifest lists; none when it has no history.
    pub fn table(
        &self,
        market: Market,
        table: &str,
        location: &str,
    ) -> Option<Result<Table, ExportError>> {
        let p = self.published(market, table)?;
        let uuid = crate::iceberg::table_uuid_in(market, table);
        Some(
            to_metadata(&p, location, &uuid).map_err(|source| ExportError::Publish {
                table: table.to_owned(),
                source,
            }),
        )
    }

    /// Renders every file.
    pub fn render(&self) -> Result<Rendered, ExportError> {
        let mut out = Rendered::default();
        let mut markets = serde_json::Map::new();
        for (market, e) in &self.markets {
            let mut listed = Vec::new();
            let mut histories = Vec::new();
            for table in e.histories.keys() {
                let dir = format!("{market}/{table}");
                let location = format!("{}{dir}", self.base_url);
                let t = self
                    .table(*market, table, &location)
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
                debug_assert_eq!(
                    p,
                    self.published(*market, table)
                        .expect("the table has a history")
                );
                for s in &p.history.schemas {
                    let path = format!("{dir}/{}", schema::file_name(s.schema_id));
                    let doc = schema::document(&self.base_url, table, &path, s, &p);
                    out.files.insert(path, doc.into());
                }
                out.schemas += p.history.schemas.len();
                listed.push(listing(&p));
                histories.push(p.history);
            }
            out.tables += histories.len();
            out.files.insert(
                format!("{market}/headers.json"),
                text(&headers(histories.iter())).into(),
            );
            if !e.folders.is_empty() {
                out.files.insert(
                    format!("{market}/folders.json"),
                    text(&json!({"x-format": FORMAT, "folders": e.folders})).into(),
                );
            }
            markets.insert(
                market.id().to_owned(),
                json!({
                    "baseline": {"version": e.baseline.version, "reason": e.baseline.reason},
                    "tables": histories.len(),
                    "schemas": histories.iter().map(|h| h.schemas.len()).sum::<usize>(),
                    "listed": listed,
                }),
            );
        }
        let manifest = manifest(&self.base_url, &markets, &out);
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

/// `{market}/folders.json`: each report folder the census recorded, under
/// its feed and root, with every block it carried: report pair, the
/// versions seen, the first and last release and how many months. A
/// reader fetching an interval of a table from the report folders asks it
/// which folders carry the table's report pairs.
fn folders(root: &Path, market: Market) -> Result<Vec<Value>, ExportError> {
    /// A block: report pair and version.
    type Key = ([String; 2], u16);
    let mut out = Vec::new();
    for feed in feeds_of(market.id()) {
        for feed_root in feed.roots {
            let label = Feed::label(feed_root);
            // The root's own census (`_root`, an empty name) first, then
            // its sub-folders by name.
            let mut rels = recorded_folders(root, market, (feed, label))?;
            rels.sort();
            for rel in rels {
                // Listed, so recorded: an empty census if it has gone since.
                let census = read_folder(root, market, (feed, label, &rel))?.unwrap_or_default();
                let mut blocks: BTreeMap<Key, (String, String, u64)> = BTreeMap::new();
                for t in &census.blocks {
                    let key = (
                        [t.block.report.0.clone(), t.block.report.1.clone()],
                        t.block.version,
                    );
                    let month = t.month.to_string();
                    let e = blocks
                        .entry(key)
                        .or_insert_with(|| (month.clone(), month.clone(), 0));
                    if month < e.0 {
                        e.0 = month.clone();
                    }
                    if month > e.1 {
                        e.1 = month;
                    }
                    e.2 += 1;
                }
                let folder = if rel.is_empty() {
                    feed_root.rsplit('/').next().unwrap_or(feed_root).to_owned()
                } else {
                    rel.clone()
                };
                out.push(json!({
                    "feed": feed.id,
                    "label": label,
                    "root": feed_root,
                    "folder": folder,
                    "blocks": blocks
                        .into_iter()
                        .map(|((report, version), (first, last, months))| {
                            json!({
                                "report": report,
                                "version": version,
                                "first-release": first,
                                "last-release": last,
                                "months": months,
                            })
                        })
                        .collect::<Vec<_>>(),
                }));
            }
        }
    }
    Ok(out)
}

fn non_empty(s: &str) -> Option<&str> {
    Some(s).filter(|s| !s.is_empty())
}

/// The manifest: what was written, each market's data version
/// (`baseline`: the version, and the reason it was begun), its counts and
/// its tables' lines ([`listing`]) under `markets`, and the SHA-256 of
/// every file under every market's directory.
fn manifest(base_url: &str, markets: &serde_json::Map<String, Value>, r: &Rendered) -> String {
    let hashes: BTreeMap<&str, String> = r
        .files
        .iter()
        .map(|(p, bytes)| (p.as_str(), crate::ctl::sha256(bytes)))
        .collect();
    text(&json!({
        "format": FORMAT,
        "base_url": base_url,
        "markets": markets,
        "tables": r.tables,
        "schemas": r.schemas,
        "sha256": hashes,
    }))
}

#[cfg(test)]
pub(crate) mod tests;
