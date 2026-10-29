//! A published table as Iceberg table metadata, and back.
//!
//! The one document of a published table is its `v1.metadata.json`
//! ([`TableMetadata`]). Everything else said of the table is derived from
//! it. Beyond what Iceberg holds of any table (schemas by id, the current
//! schema), the metadata carries:
//!
//! - **The history's runs as snapshots**, in run order: `sequence-number`
//!   1..n; `schema-id` the run's; the run's files, rows, report pair,
//!   AEMO version, kinds and first and last file in the snapshot's
//!   `summary` as `phasor.*` keys ([`summary`]); `operation` `append`.
//!   The snapshot's id is a hash of the run's first file, report pair,
//!   version and schema, so a run kept across exports keeps its id. Its
//!   time is the start of the month AEMO released the run's first file
//!   in (market time, +10:00) plus the run's index in milliseconds, so
//!   the times rise strictly in release order. Each snapshot's manifest
//!   list is an empty one at `metadata/snap-{id}-{seq}-{uuid}.avro`: the
//!   table holds no data, and a reader that opens it reads no rows. Row
//!   lineage counts nothing. What a run says of its start is not stored:
//!   a reader derives it ([`crate::history::derive`]).
//! - **The breaking columns** as the property `phasor.breaks`, a compact
//!   JSON array of [`Break`].
//! - **The declaration** (`tables/{TABLE}.toml`) as `phasor.*`
//!   properties ([`Declaration`]); `phasor.declared` says whether there
//!   is one. Every list is a compact JSON array.
//! - **The documentation**: the table's description as `comment`, each
//!   column's as its field's `doc` (on every schema's copy of the field),
//!   and each column's type in AEMO's data model as `phasor.model-types`,
//!   a compact JSON object by field id.
//! - `phasor.format`, the generation of this layout ([`FORMAT`]), and
//!   `phasor.table`, the table's name.
//! - **The market**, for a table that is not the NEM's: `phasor.market`
//!   (`wem`, `gas`) and `phasor.offset` (`+08:00`). Absent, the table is
//!   the NEM's at `+10:00`, as every table published before there were
//!   other markets is: a NEM table's metadata is the same bytes with or
//!   without this.
//! - **The headers** a file wrote for columns whose names were made from
//!   them (`Load Forecast (MW)` for `LOAD_FORECAST_MW`, [`History::headers`]):
//!   `phasor.headers`, a compact JSON object by field id; and **the model tables** that
//!   annotate a table whose own name is not in AEMO's data model:
//!   `phasor.model`, a compact JSON array. Both only when there are any.
//!
//! [`to_metadata`] writes a [`Published`] table this way and
//! [`from_metadata`] reads one back: the same table, with the fields'
//! docs taken off the history ([`Schema::without_docs`]), as the fold
//! compares and names schemas without them.

use std::collections::BTreeMap;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::history::{Break, History, Mark, Run, derive};
use crate::market::Market;
use crate::schema::Schema;
use crate::table::manifest::write_manifest_list;
use crate::table::metadata::{Snapshot, TableMetadata};

/// The generation of the published layout: the properties, the summary
/// keys and their meaning. Raised when a reader of this format would
/// misread a table: 4 since a table may take only the blocks of its pairs
/// that hold certain columns (`phasor.with-columns`), which a reader of
/// 3 would route to the wrong table; 5 since a declaration carries no key
/// and no temporal roles (`phasor.key`, `phasor.temporal.*`), which are a
/// reader's to decide: a reader of 4 would find them missing.
pub const FORMAT: u32 = 5;

/// A table's declaration, as `tables/{TABLE}.toml` holds it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Declaration {
    /// The package of AEMO's data model it is in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub package: Option<String>,
    /// The report pairs (type, subtype) its blocks are published under.
    pub report_pairs: Vec<[String; 2]>,
    /// The columns a block of those pairs must hold to be this table's;
    /// empty for every block of them (the table of one kind of row where
    /// a pair carries two, as an area's beside a region's).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub with_columns: Vec<String>,
    /// The column a block's time span is of.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time_column: Option<String>,
    /// How its files are loaded: `append` or `snapshot`.
    pub load_mode: String,
    /// Who may see it, as AEMO's control files say.
    pub visibility: String,
}

/// A table as published: its history, its declaration when it has one,
/// and its documentation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Published {
    /// The market the table is of.
    pub market: Market,
    /// The history, no field documented.
    pub history: History,
    /// The declaration.
    pub declaration: Option<Declaration>,
    /// What the table holds, in AEMO's words.
    pub description: Option<String>,
    /// What each column holds, in AEMO's words, by field id.
    pub docs: BTreeMap<u32, String>,
    /// Each column's type in AEMO's data model, by field id.
    pub model_types: BTreeMap<u32, String>,
    /// The tables of AEMO's data model whose columns annotate this table,
    /// when the table's own name is not one of them.
    pub model: Vec<String>,
}

/// A manifest list to write beside the metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManifestList {
    /// Its path under the table's location: `metadata/snap-….avro`.
    pub path: String,
    /// Its bytes.
    pub bytes: Vec<u8>,
}

/// A published table as files: the metadata, and one manifest list per
/// snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Table {
    /// `metadata/v1.metadata.json`.
    pub metadata: TableMetadata,
    /// The manifest lists, in snapshot order.
    pub manifest_lists: Vec<ManifestList>,
}

/// Why a table cannot be written as metadata, or read from it.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PublishError {
    /// A run's first file has a release that is not a month.
    #[error(
        "the run beginning at {file} was released in {release:?}, which is not a month (YYYY-MM)"
    )]
    Release {
        /// The file.
        file: String,
        /// Its release.
        release: String,
    },
    /// A key the metadata needs and lacks.
    #[error("the table metadata lacks {key} in {at}")]
    Missing {
        /// Where: the properties, or a snapshot's summary.
        at: String,
        /// The key.
        key: String,
    },
    /// A key whose value is not what it must be.
    #[error("in {at}, {key} is {value:?}, which is not {wanted}")]
    Malformed {
        /// Where.
        at: String,
        /// The key.
        key: String,
        /// Its value.
        value: String,
        /// What it must be.
        wanted: String,
    },
    /// A snapshot written under a schema the table lacks.
    #[error("a snapshot was written under schema {0}, which the table lacks")]
    UnknownSchema(u32),
}

const K_FORMAT: &str = "phasor.format";
const K_TABLE: &str = "phasor.table";
const K_DECLARED: &str = "phasor.declared";
const K_PACKAGE: &str = "phasor.package";
const K_REPORTS: &str = "phasor.reports";
const K_WITH_COLUMNS: &str = "phasor.with-columns";
const K_TIME: &str = "phasor.time";
const K_LOAD: &str = "phasor.load";
const K_VISIBILITY: &str = "phasor.visibility";
const K_COMMENT: &str = "comment";
const K_MODEL_TYPES: &str = "phasor.model-types";
const K_BREAKS: &str = "phasor.breaks";
const K_MARKET: &str = "phasor.market";
const K_OFFSET: &str = "phasor.offset";
const K_HEADERS: &str = "phasor.headers";
const K_MODEL: &str = "phasor.model";

const S_ROWS: &str = "phasor.rows";
const S_TOTAL_ROWS: &str = "phasor.total-rows";
const S_REPORT_TYPE: &str = "phasor.report-type";
const S_REPORT_SUBTYPE: &str = "phasor.report-subtype";
const S_VERSION: &str = "phasor.version";
const S_KINDS: &str = "phasor.kinds";
const S_FILES: &str = "phasor.files";

fn json<T: Serialize>(v: &T) -> String {
    serde_json::to_string(v).expect("a list or map of strings serialises")
}

/// Days from 1970-01-01 to a civil date (proleptic Gregorian).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Midnight at +10:00 beginning the month `YYYY-MM`, in milliseconds
/// since 1970; none when it is not a month.
pub fn month_start_ms(month: &str) -> Option<i64> {
    month_start_ms_at(month, Market::Nem.offset_secs())
}

/// Midnight at `offset_secs` from UTC beginning the month `YYYY-MM`, in
/// milliseconds since 1970; none when it is not a month.
pub fn month_start_ms_at(month: &str, offset_secs: i32) -> Option<i64> {
    let (y, m) = month.split_once('-')?;
    if y.len() != 4 || m.len() != 2 || !y.bytes().chain(m.bytes()).all(|c| c.is_ascii_digit()) {
        return None;
    }
    let (y, m): (i64, i64) = (y.parse().ok()?, m.parse().ok()?);
    (1..=12)
        .contains(&m)
        .then(|| (days_from_civil(y, m, 1) * 86_400 - i64::from(offset_secs)) * 1000)
}

/// The id of the snapshot of `run` in `table`: 63 bits of the SHA-256 of
/// its first file, report pair, version and schema, never 0; `salt` is
/// raised when two runs of the table begin alike.
fn snapshot_id(table: &str, run: &Run, salt: u32) -> i64 {
    use sha2::Digest as _;
    let mut h = sha2::Sha256::new();
    let parts = [
        table,
        &run.from.file,
        &run.from.sha256,
        &run.report[0],
        &run.report[1],
        &run.version.to_string(),
        &run.schema_id.to_string(),
        &salt.to_string(),
    ];
    for part in parts {
        h.update(part.as_bytes());
        h.update(b"\n");
    }
    let digest = h.finalize();
    let mut b = [0u8; 8];
    b.copy_from_slice(&digest[..8]);
    (i64::from_be_bytes(b) & i64::MAX).max(1)
}

fn mark_keys(side: &str, m: &Mark, into: &mut BTreeMap<String, String>) {
    into.insert(format!("phasor.{side}.file"), m.file.clone());
    into.insert(format!("phasor.{side}.sha256"), m.sha256.clone());
    into.insert(format!("phasor.{side}.release"), m.release.clone());
    if let Some(t) = &m.data_time {
        into.insert(format!("phasor.{side}.time"), t.clone());
    }
}

/// A run as a snapshot's summary: `operation` `append`, the run's rows
/// and the rows of every run so far (`phasor.rows`, `phasor.total-rows`),
/// its report pair (`phasor.report-type`, `phasor.report-subtype`), AEMO
/// version (`phasor.version`), kinds (`phasor.kinds`, a JSON array),
/// files (`phasor.files`), and its first and last file (`phasor.from.*`,
/// `phasor.to.*`: `file`, `sha256`, `release`, and `time` when the file's
/// data has one).
pub fn summary(run: &Run, total_rows: u64) -> BTreeMap<String, String> {
    let mut s = BTreeMap::new();
    s.insert("operation".to_owned(), "append".to_owned());
    s.insert(S_ROWS.to_owned(), run.rows.to_string());
    s.insert(S_TOTAL_ROWS.to_owned(), total_rows.to_string());
    s.insert(S_REPORT_TYPE.to_owned(), run.report[0].clone());
    s.insert(S_REPORT_SUBTYPE.to_owned(), run.report[1].clone());
    s.insert(S_VERSION.to_owned(), run.version.to_string());
    s.insert(S_KINDS.to_owned(), json(&run.kinds));
    s.insert(S_FILES.to_owned(), run.files.to_string());
    mark_keys("from", &run.from, &mut s);
    mark_keys("to", &run.to, &mut s);
    s
}

/// The table's properties.
fn properties(p: &Published) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let mut put = |k: &str, v: String| {
        out.insert(k.to_owned(), v);
    };
    put(K_FORMAT, FORMAT.to_string());
    put(K_TABLE, p.history.table.clone());
    if p.market != Market::Nem {
        put(K_MARKET, p.market.id().to_owned());
        put(K_OFFSET, p.market.offset_text().to_owned());
    }
    put(K_DECLARED, p.declaration.is_some().to_string());
    if let Some(d) = &p.declaration {
        if let Some(pkg) = &d.package {
            put(K_PACKAGE, pkg.clone());
        }
        put(K_REPORTS, json(&d.report_pairs));
        if !d.with_columns.is_empty() {
            put(K_WITH_COLUMNS, json(&d.with_columns));
        }
        if let Some(t) = &d.time_column {
            put(K_TIME, t.clone());
        }
        put(K_LOAD, d.load_mode.clone());
        put(K_VISIBILITY, d.visibility.clone());
    }
    if let Some(c) = &p.description {
        put(K_COMMENT, c.clone());
    }
    if !p.model_types.is_empty() {
        put(K_MODEL_TYPES, json(&p.model_types));
    }
    if !p.history.headers.is_empty() {
        put(K_HEADERS, json(&p.history.headers));
    }
    if !p.model.is_empty() {
        put(K_MODEL, json(&p.model));
    }
    if !p.history.breaking.is_empty() {
        put(K_BREAKS, json(&p.history.breaking));
    }
    out
}

/// The table `p` as metadata at `location` with UUID `uuid`, and its
/// manifest lists. The same table writes the same bytes.
pub fn to_metadata(p: &Published, location: &str, uuid: &str) -> Result<Table, PublishError> {
    let h = &p.history;
    let schemas: Vec<Schema> = h.schemas.iter().map(|s| s.with_docs(&p.docs)).collect();
    let first = schemas.first().cloned().unwrap_or_default();
    let mut m = TableMetadata::new(uuid, location, &first, 0);
    m.last_column_id = schemas
        .iter()
        .flat_map(|s| s.fields.iter().map(|f| f.id))
        .max()
        .unwrap_or(0);
    m.schemas = schemas;
    m.current_schema_id = h.current_schema_id;
    m.properties = properties(p);
    let mut manifest_lists = Vec::new();
    let mut total_rows = 0;
    for (i, run) in h.runs.iter().enumerate() {
        let sequence = i64::try_from(i).expect("fewer runs than i64 counts") + 1;
        let mut salt = 0;
        let mut id = snapshot_id(&h.table, run, salt);
        while m.snapshots.iter().any(|s| s.snapshot_id == id) {
            salt += 1;
            id = snapshot_id(&h.table, run, salt);
        }
        let parent = m.current_snapshot_id;
        let timestamp_ms = month_start_ms_at(&run.from.release, p.market.offset_secs())
            .ok_or_else(|| PublishError::Release {
                file: run.from.file.clone(),
                release: run.from.release.clone(),
            })?
            + sequence
            - 1;
        let path = format!("metadata/snap-{id}-{sequence}-{uuid}.avro");
        let bytes = write_manifest_list(id, parent, sequence, 0, &[])
            .expect("an empty manifest list encodes");
        total_rows += run.rows;
        m.add_snapshot(Snapshot {
            snapshot_id: id,
            parent_snapshot_id: parent,
            sequence_number: sequence,
            timestamp_ms,
            manifest_list: format!("{location}/{path}"),
            summary: summary(run, total_rows),
            schema_id: run.schema_id,
            first_row_id: 0,
            added_rows: 0,
        });
        m.last_updated_ms = timestamp_ms;
        manifest_lists.push(ManifestList { path, bytes });
    }
    Ok(Table {
        metadata: m,
        manifest_lists,
    })
}

/// A string map read for what it must hold.
struct Props<'a> {
    map: &'a BTreeMap<String, String>,
    at: String,
}

impl Props<'_> {
    fn get(&self, key: &str) -> Option<&str> {
        self.map.get(key).map(String::as_str)
    }

    fn need(&self, key: &str) -> Result<&str, PublishError> {
        self.get(key).ok_or_else(|| PublishError::Missing {
            at: self.at.clone(),
            key: key.to_owned(),
        })
    }

    fn malformed(&self, key: &str, value: &str, wanted: &str) -> PublishError {
        PublishError::Malformed {
            at: self.at.clone(),
            key: key.to_owned(),
            value: value.to_owned(),
            wanted: wanted.to_owned(),
        }
    }

    fn num<T: FromStr>(&self, key: &str) -> Result<T, PublishError> {
        let v = self.need(key)?;
        v.parse().map_err(|_| self.malformed(key, v, "a number"))
    }

    /// The key's value as JSON; `None` when the key is absent.
    fn json<T: serde::de::DeserializeOwned>(&self, key: &str) -> Result<Option<T>, PublishError> {
        let Some(v) = self.get(key) else {
            return Ok(None);
        };
        serde_json::from_str(v)
            .map(Some)
            .map_err(|_| self.malformed(key, v, "JSON of the shape this key takes"))
    }

    fn need_json<T: serde::de::DeserializeOwned>(&self, key: &str) -> Result<T, PublishError> {
        self.need(key)?;
        Ok(self.json(key)?.expect("the key is there"))
    }

    fn owned(&self, key: &str) -> Option<String> {
        self.get(key).map(str::to_owned)
    }
}

fn mark_of(p: &Props<'_>, side: &str) -> Result<Mark, PublishError> {
    Ok(Mark {
        file: p.need(&format!("phasor.{side}.file"))?.to_owned(),
        sha256: p.need(&format!("phasor.{side}.sha256"))?.to_owned(),
        release: p.need(&format!("phasor.{side}.release"))?.to_owned(),
        data_time: p.owned(&format!("phasor.{side}.time")),
    })
}

fn run_of(s: &Snapshot) -> Result<Run, PublishError> {
    let p = Props {
        map: &s.summary,
        at: format!("the summary of snapshot {}", s.snapshot_id),
    };
    Ok(Run {
        schema_id: s.schema_id,
        report: [
            p.need(S_REPORT_TYPE)?.to_owned(),
            p.need(S_REPORT_SUBTYPE)?.to_owned(),
        ],
        version: p.num(S_VERSION)?,
        kinds: p.need_json(S_KINDS)?,
        from: mark_of(&p, "from")?,
        to: mark_of(&p, "to")?,
        files: p.num(S_FILES)?,
        rows: p.num(S_ROWS)?,
        changes: Vec::new(),
        updates: Vec::new(),
        overlaps: false,
    })
}

fn declaration_of(p: &Props<'_>) -> Result<Declaration, PublishError> {
    Ok(Declaration {
        package: p.owned(K_PACKAGE),
        report_pairs: p.need_json(K_REPORTS)?,
        with_columns: p.json(K_WITH_COLUMNS)?.unwrap_or_default(),
        time_column: p.owned(K_TIME),
        load_mode: p.need(K_LOAD)?.to_owned(),
        visibility: p.need(K_VISIBILITY)?.to_owned(),
    })
}

/// A table's declaration from its properties alone: what a reader of a
/// table built from the published one (the lake's, whose snapshots are its
/// own) needs of it. `None` for a table not declared.
pub fn declaration_of_properties(
    properties: &BTreeMap<String, String>,
) -> Result<Option<Declaration>, PublishError> {
    let p = Props {
        map: properties,
        at: "the table's properties".to_owned(),
    };
    match p.need(K_DECLARED)? {
        "true" => Ok(Some(declaration_of(&p)?)),
        "false" => Ok(None),
        other => Err(p.malformed(K_DECLARED, other, "true or false")),
    }
}

/// A table's breaking columns from its properties alone; none when it has
/// none.
pub fn breaks_of_properties(
    properties: &BTreeMap<String, String>,
) -> Result<Vec<Break>, PublishError> {
    let p = Props {
        map: properties,
        at: "the table's properties".to_owned(),
    };
    Ok(p.json(K_BREAKS)?.unwrap_or_default())
}

/// The table `m` holds, as [`to_metadata`] wrote it: every `phasor.*`
/// key it needs must be there and well formed. Snapshots are taken in
/// sequence order.
pub fn from_metadata(m: &TableMetadata) -> Result<Published, PublishError> {
    let p = Props {
        map: &m.properties,
        at: "the table's properties".to_owned(),
    };
    // Format 3 is format 4 with no table taking only some of a pair's
    // blocks: read as such.
    let format = p.need(K_FORMAT)?;
    if format != FORMAT.to_string() && format != "3" {
        return Err(p.malformed(K_FORMAT, format, &format!("format {FORMAT}")));
    }
    let table = p.need(K_TABLE)?.to_owned();
    let market = match p.get(K_MARKET) {
        None => Market::Nem,
        Some(id) => Market::parse(id).ok_or_else(|| p.malformed(K_MARKET, id, "a market"))?,
    };
    if let Some(offset) = p.get(K_OFFSET)
        && offset != market.offset_text()
    {
        return Err(p.malformed(K_OFFSET, offset, &format!("{market}'s offset")));
    }
    let declaration = match p.need(K_DECLARED)? {
        "true" => Some(declaration_of(&p)?),
        "false" => None,
        other => return Err(p.malformed(K_DECLARED, other, "true or false")),
    };
    let mut docs = BTreeMap::new();
    let schemas: Vec<Schema> = m
        .schemas
        .iter()
        .map(|s| {
            for f in &s.fields {
                if let Some(d) = &f.doc {
                    docs.entry(f.id).or_insert_with(|| d.clone());
                }
            }
            s.without_docs()
        })
        .collect();
    let mut snapshots: Vec<&Snapshot> = m.snapshots.iter().collect();
    snapshots.sort_by_key(|s| s.sequence_number);
    let mut runs = snapshots
        .iter()
        .map(|s| run_of(s))
        .collect::<Result<Vec<_>, _>>()?;
    derive(&schemas, &mut runs).map_err(PublishError::UnknownSchema)?;
    let breaking: Vec<Break> = p.json(K_BREAKS)?.unwrap_or_default();
    let model_types: BTreeMap<u32, String> = p.json(K_MODEL_TYPES)?.unwrap_or_default();
    let headers: BTreeMap<u32, String> = p.json(K_HEADERS)?.unwrap_or_default();
    let model: Vec<String> = p.json(K_MODEL)?.unwrap_or_default();
    Ok(Published {
        market,
        history: History {
            table,
            format_version: 3,
            current_schema_id: m.current_schema_id,
            schemas,
            runs,
            breaking,
            headers,
        },
        declaration,
        description: p.owned(K_COMMENT),
        docs,
        model_types,
        model,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::{Change, Update, tests::sample};
    use crate::table::manifest::read_manifest_list;

    /// The sample history, derived, declared and documented.
    fn published() -> Published {
        let mut h: History = serde_json::from_value(sample()).unwrap();
        // A third run: published boundary, no data time at its start.
        let mut third = h.runs[1].clone();
        third.from = Mark {
            file: "D.zip".into(),
            sha256: "cd".into(),
            release: "2024-09".into(),
            data_time: None,
        };
        third.to = third.from.clone();
        h.runs.push(third);
        derive(&h.schemas, &mut h.runs).unwrap();
        h.current_schema_id = 1;
        Published {
            market: Market::Nem,
            history: h,
            declaration: Some(Declaration {
                package: Some("DISPATCH".into()),
                report_pairs: vec![["DISPATCH".into(), "UNIT_SCADA".into()]],
                with_columns: vec![],
                time_column: Some("SETTLEMENTDATE".into()),
                load_mode: "append".into(),
                visibility: "PUBLIC".into(),
            }),
            description: Some("Unit SCADA".into()),
            docs: BTreeMap::from([(1, "Interval end".to_owned()), (3, "A flag".to_owned())]),
            model_types: BTreeMap::from([(1, "TIMESTAMP(0)".to_owned())]),
            model: Vec::new(),
        }
    }

    #[test]
    fn a_tables_kind_of_rows_is_a_property_and_reads_back() {
        // Without one, the key is absent, as every table before had it.
        let plain = to_metadata(&published(), "file:///t", "u").unwrap();
        assert!(!plain.metadata.properties.contains_key(K_WITH_COLUMNS));
        // A table of one kind of a pair's rows says which, and reads back.
        let mut area = published();
        area.declaration.as_mut().unwrap().with_columns = vec!["AREAID".into()];
        let t = to_metadata(&area, "file:///t", "u").unwrap();
        assert_eq!(t.metadata.properties[K_WITH_COLUMNS], r#"["AREAID"]"#);
        assert_eq!(from_metadata(&t.metadata).unwrap(), area);
        // Its declaration and breaks read from its properties alone, as a
        // reader of a table built from this one does.
        let d = declaration_of_properties(&t.metadata.properties).unwrap();
        assert_eq!(d, area.declaration);
        assert_eq!(
            breaks_of_properties(&t.metadata.properties).unwrap(),
            area.history.breaking
        );
        let mut undeclared = t.metadata.properties.clone();
        undeclared.insert(K_DECLARED.into(), "false".into());
        assert_eq!(declaration_of_properties(&undeclared).unwrap(), None);
        undeclared.insert(K_DECLARED.into(), "maybe".into());
        assert!(declaration_of_properties(&undeclared).is_err());
        undeclared.insert(K_BREAKS.into(), "not json".into());
        assert!(breaks_of_properties(&undeclared).is_err());
        // Format 3 is read as 4 with no such tables.
        let mut old = plain.metadata.clone();
        old.properties.insert(K_FORMAT.into(), "3".into());
        assert_eq!(from_metadata(&old).unwrap(), published());
    }

    #[test]
    fn a_published_table_is_its_metadata_and_back() {
        let p = published();
        let t = to_metadata(&p, "file:///t", "u").unwrap();
        let m = &t.metadata;
        assert_eq!(from_metadata(m).unwrap(), p);
        // The runs are the snapshots, in order, each with its summary and
        // an empty manifest list of its own; the times rise.
        assert_eq!(m.snapshots.len(), 3);
        assert_eq!(t.manifest_lists.len(), 3);
        let ids: Vec<i64> = m.snapshots.iter().map(|s| s.snapshot_id).collect();
        assert!(ids.iter().all(|i| *i > 0));
        for (i, s) in m.snapshots.iter().enumerate() {
            assert_eq!(s.sequence_number, i64::try_from(i).unwrap() + 1);
            assert_eq!(s.parent_snapshot_id, i.checked_sub(1).map(|j| ids[j]));
            assert_eq!(
                s.manifest_list,
                format!("file:///t/{}", t.manifest_lists[i].path)
            );
            assert_eq!((s.first_row_id, s.added_rows), (0, 0));
            assert_eq!(s.summary["operation"], "append");
            assert_eq!(read_manifest_list(&t.manifest_lists[i].bytes).unwrap(), []);
        }
        assert!(
            m.snapshots
                .windows(2)
                .all(|w| w[0].timestamp_ms < w[1].timestamp_ms)
        );
        assert_eq!(
            m.snapshots[0].timestamp_ms,
            month_start_ms("2024-08").unwrap()
        );
        assert_eq!(
            m.snapshots[1].timestamp_ms,
            month_start_ms("2024-08").unwrap() + 1
        );
        assert_eq!(
            m.snapshots[2].timestamp_ms,
            month_start_ms("2024-09").unwrap() + 2
        );
        assert_eq!(m.last_updated_ms, m.snapshots[2].timestamp_ms);
        assert_eq!(m.current_snapshot_id, Some(ids[2]));
        assert_eq!(m.refs["main"].snapshot_id, ids[2]);
        assert_eq!((m.next_row_id, m.last_sequence_number), (0, 3));
        let s = &m.snapshots[0].summary;
        assert_eq!(s[S_ROWS], "9");
        assert_eq!(s[S_TOTAL_ROWS], "9");
        assert_eq!(m.snapshots[2].summary[S_TOTAL_ROWS], "15");
        assert_eq!(s[S_KINDS], r#"["archive"]"#);
        assert_eq!(s["phasor.from.time"], "2024-08-01T00:05:00+10:00");
        assert!(!s.contains_key("phasor.to.time"));
        // The declaration and documentation are properties; the docs are
        // on every schema's copy of a field.
        assert_eq!(m.properties[K_FORMAT], "5");
        assert_eq!(m.properties[K_DECLARED], "true");
        assert_eq!(m.properties[K_REPORTS], r#"[["DISPATCH","UNIT_SCADA"]]"#);
        // A table's key and roles are a reader's: none is published.
        assert!(
            !m.properties
                .keys()
                .any(|k| k == "phasor.key" || k.starts_with("phasor.temporal")),
            "{:?}",
            m.properties
        );
        assert_eq!(m.properties[K_COMMENT], "Unit SCADA");
        assert_eq!(m.properties[K_MODEL_TYPES], r#"{"1":"TIMESTAMP(0)"}"#);
        assert!(m.properties[K_BREAKS].starts_with(r#"[{"column":"X""#));
        // A NEM table says nothing of its market: its bytes are those of
        // every table published before there were other markets.
        for key in [K_MARKET, K_OFFSET, K_HEADERS, K_MODEL] {
            assert!(!m.properties.contains_key(key), "{key}");
        }
        for s in &m.schemas {
            assert_eq!(s.field(1).unwrap().doc.as_deref(), Some("Interval end"));
            assert_eq!(s.field(2).unwrap().doc, None);
        }
        assert_eq!(m.last_column_id, 3);
        // The same table, the same bytes; another table, other ids.
        assert_eq!(to_metadata(&p, "file:///t", "u").unwrap(), t);
        let mut other = p.clone();
        other.history.table = "OTHER".into();
        let o = to_metadata(&other, "file:///t", "u").unwrap();
        assert_ne!(o.metadata.snapshots[0].snapshot_id, ids[0]);
        // The derived changes are what the fold wrote.
        let h = &from_metadata(m).unwrap().history;
        assert_eq!(h.runs[2].changes, [Change::Published]);
        assert_eq!(h.runs[1].updates[0], Update::Add { field_id: 3 });
        let text = serde_json::to_string(m).unwrap();
        let back: TableMetadata = serde_json::from_str(&text).unwrap();
        assert_eq!(from_metadata(&back).unwrap(), p);
    }

    #[test]
    fn an_undeclared_table_with_no_runs_is_still_a_table() {
        let p = Published {
            market: Market::Nem,
            history: History {
                table: "T".into(),
                format_version: 3,
                current_schema_id: 0,
                schemas: vec![],
                runs: vec![],
                breaking: vec![],
                headers: BTreeMap::new(),
            },
            declaration: None,
            description: None,
            docs: BTreeMap::new(),
            model_types: BTreeMap::new(),
            model: Vec::new(),
        };
        let t = to_metadata(&p, "file:///t", "u").unwrap();
        let m = &t.metadata;
        assert!(m.snapshots.is_empty() && t.manifest_lists.is_empty());
        assert_eq!((m.current_snapshot_id, m.last_updated_ms), (None, 0));
        assert_eq!(m.properties[K_DECLARED], "false");
        assert!(!m.properties.contains_key(K_COMMENT));
        assert!(!m.properties.contains_key(K_BREAKS));
        assert!(!m.properties.contains_key(K_MODEL_TYPES));
        assert_eq!(from_metadata(m).unwrap(), p);
        // A declaration with nothing optional.
        let mut d = published().declaration.unwrap();
        d.package = None;
        d.time_column = None;
        let q = Published {
            declaration: Some(d),
            ..p
        };
        let t = to_metadata(&q, "file:///t", "u").unwrap();
        assert!(!t.metadata.properties.contains_key(K_PACKAGE));
        assert_eq!(from_metadata(&t.metadata).unwrap(), q);
    }

    #[test]
    fn a_table_of_another_market_says_so_and_keeps_its_clock() {
        let mut p = published();
        p.market = Market::Wem;
        p.history.headers = BTreeMap::from([(1, "Trading Interval".to_owned())]);
        p.model = vec!["GAS_DAILY_FLOW_ACTUAL".to_owned()];
        let t = to_metadata(&p, "file:///t", "u").unwrap();
        let m = &t.metadata;
        assert_eq!(m.properties[K_MARKET], "wem");
        assert_eq!(m.properties[K_OFFSET], "+08:00");
        assert_eq!(m.properties[K_HEADERS], r#"{"1":"Trading Interval"}"#);
        assert_eq!(m.properties[K_MODEL], r#"["GAS_DAILY_FLOW_ACTUAL"]"#);
        // Midnight in Perth, not Sydney: two hours later in UTC.
        assert_eq!(
            m.snapshots[0].timestamp_ms,
            month_start_ms("2024-08").unwrap() + 2 * 3600 * 1000
        );
        assert_eq!(
            month_start_ms_at("2024-01", 8 * 3600),
            Some(1_704_038_400_000)
        );
        assert_eq!(from_metadata(m).unwrap(), p);
        let nem = to_metadata(&published(), "file:///t", "u").unwrap();
        assert_ne!(
            nem.metadata.snapshots[0].timestamp_ms,
            m.snapshots[0].timestamp_ms
        );
        // A gas table keeps the NEM's clock.
        p.market = Market::Gas;
        let g = to_metadata(&p, "file:///t", "u").unwrap();
        assert_eq!(g.metadata.properties[K_MARKET], "gas");
        assert_eq!(g.metadata.properties[K_OFFSET], "+10:00");
        assert_eq!(
            g.metadata.snapshots[0].timestamp_ms,
            nem.metadata.snapshots[0].timestamp_ms
        );
        assert_eq!(from_metadata(&g.metadata).unwrap().market, Market::Gas);
    }

    #[test]
    fn two_runs_that_begin_alike_get_their_own_snapshots() {
        let mut p = published();
        let twin = p.history.runs[0].clone();
        p.history.runs.insert(1, twin);
        derive(&p.history.schemas, &mut p.history.runs).unwrap();
        let t = to_metadata(&p, "file:///t", "u").unwrap();
        let mut ids: Vec<i64> = t.metadata.snapshots.iter().map(|s| s.snapshot_id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), 4);
        assert_eq!(from_metadata(&t.metadata).unwrap(), p);
    }

    #[test]
    fn a_manifest_list_is_the_same_bytes_every_time() {
        use sha2::Digest as _;
        let bytes = write_manifest_list(7, Some(3), 2, 0, &[]).unwrap();
        let hash: String = sha2::Sha256::digest(&bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        assert_eq!(
            hash,
            "bcbe192a3df8ad87da1e3b8fa8eda749036ed4aa985744605a9ff63bc619ebc8"
        );
    }

    #[test]
    fn months_begin_at_market_midnight() {
        assert_eq!(month_start_ms("1970-01"), Some(-36_000_000));
        assert_eq!(month_start_ms("2024-08"), Some(1_722_434_400_000));
        assert_eq!(month_start_ms("2000-03"), Some(951_832_800_000));
        for bad in [
            "2024",
            "2024-13",
            "2024-00",
            "24-08",
            "2024-8",
            "2024-0a",
            "2024-08-01",
        ] {
            assert_eq!(month_start_ms(bad), None, "{bad}");
        }
    }

    #[test]
    fn what_cannot_be_read_is_named() {
        let p = published();
        let t = to_metadata(&p, "file:///t", "u").unwrap();
        let m = t.metadata;
        let err = |m: &TableMetadata| from_metadata(m).unwrap_err().to_string();
        let without = |key: &str| {
            let mut m = m.clone();
            m.properties.remove(key);
            m
        };
        let with = |key: &str, value: &str| {
            let mut m = m.clone();
            m.properties.insert(key.to_owned(), value.to_owned());
            m
        };
        assert_eq!(
            err(&without(K_TABLE)),
            "the table metadata lacks phasor.table in the table's properties"
        );
        assert_eq!(
            err(&with(K_FORMAT, "2")),
            "in the table's properties, phasor.format is \"2\", which is not format 5"
        );
        assert_eq!(
            err(&with(K_DECLARED, "maybe")),
            "in the table's properties, phasor.declared is \"maybe\", which is not true or false"
        );
        assert!(err(&without(K_LOAD)).contains("lacks phasor.load"));
        assert!(err(&without(K_REPORTS)).contains("lacks phasor.reports"));
        assert!(err(&with(K_REPORTS, "[1]")).contains("JSON of the shape this key takes"));
        assert!(err(&with(K_BREAKS, "{}")).contains("phasor.breaks"));
        assert!(err(&with(K_MODEL_TYPES, "[]")).contains("phasor.model-types"));
        assert!(err(&with(K_HEADERS, "[]")).contains("phasor.headers"));
        assert!(err(&with(K_MODEL, "{}")).contains("phasor.model"));
        assert_eq!(
            err(&with(K_MARKET, "mars")),
            "in the table's properties, phasor.market is \"mars\", which is not a market"
        );
        assert_eq!(
            err(&with(K_OFFSET, "+08:00")),
            "in the table's properties, phasor.offset is \"+08:00\", which is not nem's offset"
        );
        let summary = |key: &str, value: Option<&str>| {
            let mut m = m.clone();
            let s = &mut m.snapshots[1].summary;
            match value {
                Some(v) => s.insert(key.to_owned(), v.to_owned()),
                None => s.remove(key),
            };
            m
        };
        let id = m.snapshots[1].snapshot_id;
        assert_eq!(
            err(&summary(S_VERSION, None)),
            format!("the table metadata lacks phasor.version in the summary of snapshot {id}")
        );
        assert_eq!(
            err(&summary(S_VERSION, Some("x"))),
            format!(
                "in the summary of snapshot {id}, phasor.version is \"x\", which is not a number"
            )
        );
        assert!(err(&summary(S_KINDS, Some("archive"))).contains("phasor.kinds"));
        assert!(err(&summary("phasor.to.file", None)).contains("phasor.to.file"));
        let mut unknown = m.clone();
        unknown.snapshots[0].schema_id = 9;
        assert_eq!(
            err(&unknown),
            "a snapshot was written under schema 9, which the table lacks"
        );
        // Snapshots are taken in sequence order, however they are listed.
        let mut shuffled = m.clone();
        shuffled.snapshots.reverse();
        assert_eq!(from_metadata(&shuffled).unwrap(), p);
        // A run released in something that is not a month cannot be a
        // snapshot.
        let mut bad = p.clone();
        bad.history.runs[1].from.release = "2024".into();
        assert_eq!(
            to_metadata(&bad, "file:///t", "u").unwrap_err().to_string(),
            "the run beginning at C.zip was released in \"2024\", which is not a month (YYYY-MM)"
        );
    }
}
