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
//!   `summary` as `utilake.*` keys ([`summary`]); `operation` `append`.
//!   The snapshot's id is a hash of the run's first file, report pair,
//!   version and schema, so a run kept across exports keeps its id. Its
//!   time is the start of the month AEMO released the run's first file
//!   in (market time, +10:00) plus the run's index in milliseconds, so
//!   the times rise strictly in release order. Each snapshot's manifest
//!   list is an empty one at `metadata/snap-{id}-{seq}-{uuid}.avro`: the
//!   table holds no data, and a reader that opens it reads no rows. Row
//!   lineage counts nothing. What a run says of its start is not stored:
//!   a reader derives it ([`crate::history::derive`]).
//! - **The breaking columns** as the property `utilake.breaks`, a compact
//!   JSON array of [`Break`].
//! - **The declaration** (`tables/{TABLE}.toml`) as `utilake.*`
//!   properties ([`Declaration`]); `utilake.declared` says whether there
//!   is one. Every list is a compact JSON array.
//! - **The documentation**: the table's description as `comment`, each
//!   column's as its field's `doc` (on every schema's copy of the field),
//!   and each column's type in AEMO's data model as `utilake.model-types`,
//!   a compact JSON object by field id.
//! - `utilake.format`, the generation of this layout ([`FORMAT`]), and
//!   `utilake.table`, the table's name.
//!
//! [`to_metadata`] writes a [`Published`] table this way and
//! [`from_metadata`] reads one back: the same table, with the fields'
//! docs taken off the history ([`Schema::without_docs`]), as the fold
//! compares and names schemas without them.

use std::collections::BTreeMap;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::history::{Break, History, Mark, Run, derive};
use crate::schema::Schema;
use crate::table::manifest::write_manifest_list;
use crate::table::metadata::{Snapshot, TableMetadata};

/// The generation of the published layout: the properties, the summary
/// keys and their meaning. Raised when a reader of this format would
/// misread a table.
pub const FORMAT: u32 = 3;

/// A table's declaration, as `tables/{TABLE}.toml` holds it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Declaration {
    /// The package of AEMO's data model it is in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub package: Option<String>,
    /// The report pairs (type, subtype) its blocks are published under.
    pub report_pairs: Vec<[String; 2]>,
    /// Its key, in order; empty for none.
    pub primary_key: Vec<String>,
    /// The column a block's time span is of.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time_column: Option<String>,
    /// How its files are loaded: `append` or `snapshot`.
    pub load_mode: String,
    /// Who may see it, as AEMO's control files say.
    pub visibility: String,
    /// Its temporal roles.
    pub temporal: Temporal,
}

/// A table's temporal roles.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Temporal {
    /// When a row is valid.
    pub valid: Valid,
    /// The columns that name the thing a row is about.
    pub entity: Vec<String>,
    /// The columns that order restatements of it.
    pub order: Vec<String>,
    /// The column after which a row no longer changes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cutoff: Option<String>,
    /// Where the roles came from.
    pub origin: String,
}

/// When a row is valid.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Valid {
    /// `interval_end`, `instant`, `from`, `timeless`.
    pub kind: String,
    /// The column, when there is one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub column: Option<String>,
}

/// A table as published: its history, its declaration when it has one,
/// and its documentation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Published {
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
        "The run beginning at {file} was released in {release:?}, which is not a month (YYYY-MM)."
    )]
    Release {
        /// The file.
        file: String,
        /// Its release.
        release: String,
    },
    /// A key the metadata needs and lacks.
    #[error("The table metadata lacks {key} in {at}.")]
    Missing {
        /// Where: the properties, or a snapshot's summary.
        at: String,
        /// The key.
        key: String,
    },
    /// A key whose value is not what it must be.
    #[error("In {at}, {key} is {value:?}, which is not {wanted}.")]
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
    #[error("A snapshot was written under schema {0}, which the table lacks.")]
    UnknownSchema(u32),
}

const K_FORMAT: &str = "utilake.format";
const K_TABLE: &str = "utilake.table";
const K_DECLARED: &str = "utilake.declared";
const K_PACKAGE: &str = "utilake.package";
const K_REPORTS: &str = "utilake.reports";
const K_KEY: &str = "utilake.key";
const K_TIME: &str = "utilake.time";
const K_LOAD: &str = "utilake.load";
const K_VISIBILITY: &str = "utilake.visibility";
const K_VALID_KIND: &str = "utilake.temporal.valid.kind";
const K_VALID_COLUMN: &str = "utilake.temporal.valid.column";
const K_ENTITY: &str = "utilake.temporal.entity";
const K_ORDER: &str = "utilake.temporal.order";
const K_CUTOFF: &str = "utilake.temporal.cutoff";
const K_ORIGIN: &str = "utilake.temporal.origin";
const K_COMMENT: &str = "comment";
const K_MODEL_TYPES: &str = "utilake.model-types";
const K_BREAKS: &str = "utilake.breaks";

const S_ROWS: &str = "utilake.rows";
const S_TOTAL_ROWS: &str = "utilake.total-rows";
const S_REPORT_TYPE: &str = "utilake.report-type";
const S_REPORT_SUBTYPE: &str = "utilake.report-subtype";
const S_VERSION: &str = "utilake.version";
const S_KINDS: &str = "utilake.kinds";
const S_FILES: &str = "utilake.files";

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
    let (y, m) = month.split_once('-')?;
    if y.len() != 4 || m.len() != 2 || !y.bytes().chain(m.bytes()).all(|c| c.is_ascii_digit()) {
        return None;
    }
    let (y, m): (i64, i64) = (y.parse().ok()?, m.parse().ok()?);
    (1..=12)
        .contains(&m)
        .then(|| (days_from_civil(y, m, 1) * 86_400 - 36_000) * 1000)
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
    into.insert(format!("utilake.{side}.file"), m.file.clone());
    into.insert(format!("utilake.{side}.sha256"), m.sha256.clone());
    into.insert(format!("utilake.{side}.release"), m.release.clone());
    if let Some(t) = &m.data_time {
        into.insert(format!("utilake.{side}.time"), t.clone());
    }
}

/// A run as a snapshot's summary: `operation` `append`, the run's rows
/// and the rows of every run so far (`utilake.rows`, `utilake.total-rows`),
/// its report pair (`utilake.report-type`, `utilake.report-subtype`), AEMO
/// version (`utilake.version`), kinds (`utilake.kinds`, a JSON array),
/// files (`utilake.files`), and its first and last file (`utilake.from.*`,
/// `utilake.to.*`: `file`, `sha256`, `release`, and `time` when the file's
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
    put(K_DECLARED, p.declaration.is_some().to_string());
    if let Some(d) = &p.declaration {
        if let Some(pkg) = &d.package {
            put(K_PACKAGE, pkg.clone());
        }
        put(K_REPORTS, json(&d.report_pairs));
        put(K_KEY, json(&d.primary_key));
        if let Some(t) = &d.time_column {
            put(K_TIME, t.clone());
        }
        put(K_LOAD, d.load_mode.clone());
        put(K_VISIBILITY, d.visibility.clone());
        put(K_VALID_KIND, d.temporal.valid.kind.clone());
        if let Some(c) = &d.temporal.valid.column {
            put(K_VALID_COLUMN, c.clone());
        }
        put(K_ENTITY, json(&d.temporal.entity));
        put(K_ORDER, json(&d.temporal.order));
        if let Some(c) = &d.temporal.cutoff {
            put(K_CUTOFF, c.clone());
        }
        put(K_ORIGIN, d.temporal.origin.clone());
    }
    if let Some(c) = &p.description {
        put(K_COMMENT, c.clone());
    }
    if !p.model_types.is_empty() {
        put(K_MODEL_TYPES, json(&p.model_types));
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
        let timestamp_ms =
            month_start_ms(&run.from.release).ok_or_else(|| PublishError::Release {
                file: run.from.file.clone(),
                release: run.from.release.clone(),
            })? + sequence
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
        file: p.need(&format!("utilake.{side}.file"))?.to_owned(),
        sha256: p.need(&format!("utilake.{side}.sha256"))?.to_owned(),
        release: p.need(&format!("utilake.{side}.release"))?.to_owned(),
        data_time: p.owned(&format!("utilake.{side}.time")),
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
        primary_key: p.need_json(K_KEY)?,
        time_column: p.owned(K_TIME),
        load_mode: p.need(K_LOAD)?.to_owned(),
        visibility: p.need(K_VISIBILITY)?.to_owned(),
        temporal: Temporal {
            valid: Valid {
                kind: p.need(K_VALID_KIND)?.to_owned(),
                column: p.owned(K_VALID_COLUMN),
            },
            entity: p.need_json(K_ENTITY)?,
            order: p.need_json(K_ORDER)?,
            cutoff: p.owned(K_CUTOFF),
            origin: p.need(K_ORIGIN)?.to_owned(),
        },
    })
}

/// The table `m` holds, as [`to_metadata`] wrote it: every `utilake.*`
/// key it needs must be there and well formed. Snapshots are taken in
/// sequence order.
pub fn from_metadata(m: &TableMetadata) -> Result<Published, PublishError> {
    let p = Props {
        map: &m.properties,
        at: "the table's properties".to_owned(),
    };
    let format = p.need(K_FORMAT)?;
    if format != FORMAT.to_string() {
        return Err(p.malformed(K_FORMAT, format, &format!("format {FORMAT}")));
    }
    let table = p.need(K_TABLE)?.to_owned();
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
    Ok(Published {
        history: History {
            table,
            format_version: 3,
            current_schema_id: m.current_schema_id,
            schemas,
            runs,
            breaking,
        },
        declaration,
        description: p.owned(K_COMMENT),
        docs,
        model_types,
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
            history: h,
            declaration: Some(Declaration {
                package: Some("DISPATCH".into()),
                report_pairs: vec![["DISPATCH".into(), "UNIT_SCADA".into()]],
                primary_key: vec!["SETTLEMENTDATE".into()],
                time_column: Some("SETTLEMENTDATE".into()),
                load_mode: "append".into(),
                visibility: "PUBLIC".into(),
                temporal: Temporal {
                    valid: Valid {
                        kind: "interval_end".into(),
                        column: Some("SETTLEMENTDATE".into()),
                    },
                    entity: vec![],
                    order: vec!["LASTCHANGED".into()],
                    cutoff: Some("LASTCHANGED".into()),
                    origin: "derived".into(),
                },
            }),
            description: Some("Unit SCADA".into()),
            docs: BTreeMap::from([(1, "Interval end".to_owned()), (3, "A flag".to_owned())]),
            model_types: BTreeMap::from([(1, "TIMESTAMP(0)".to_owned())]),
        }
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
        assert_eq!(s["utilake.from.time"], "2024-08-01T00:05:00+10:00");
        assert!(!s.contains_key("utilake.to.time"));
        // The declaration and documentation are properties; the docs are
        // on every schema's copy of a field.
        assert_eq!(m.properties[K_FORMAT], "3");
        assert_eq!(m.properties[K_DECLARED], "true");
        assert_eq!(m.properties[K_KEY], r#"["SETTLEMENTDATE"]"#);
        assert_eq!(m.properties[K_REPORTS], r#"[["DISPATCH","UNIT_SCADA"]]"#);
        assert_eq!(m.properties[K_ENTITY], "[]");
        assert_eq!(m.properties[K_COMMENT], "Unit SCADA");
        assert_eq!(m.properties[K_MODEL_TYPES], r#"{"1":"TIMESTAMP(0)"}"#);
        assert!(m.properties[K_BREAKS].starts_with(r#"[{"column":"X""#));
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
            history: History {
                table: "T".into(),
                format_version: 3,
                current_schema_id: 0,
                schemas: vec![],
                runs: vec![],
                breaking: vec![],
            },
            declaration: None,
            description: None,
            docs: BTreeMap::new(),
            model_types: BTreeMap::new(),
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
        d.temporal.valid.column = None;
        d.temporal.cutoff = None;
        let q = Published {
            declaration: Some(d),
            ..p
        };
        let t = to_metadata(&q, "file:///t", "u").unwrap();
        assert!(!t.metadata.properties.contains_key(K_PACKAGE));
        assert_eq!(from_metadata(&t.metadata).unwrap(), q);
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
            "The table metadata lacks utilake.table in the table's properties."
        );
        assert_eq!(
            err(&with(K_FORMAT, "2")),
            "In the table's properties, utilake.format is \"2\", which is not format 3."
        );
        assert_eq!(
            err(&with(K_DECLARED, "maybe")),
            "In the table's properties, utilake.declared is \"maybe\", which is not true or false."
        );
        assert!(err(&without(K_LOAD)).contains("lacks utilake.load"));
        assert!(err(&without(K_KEY)).contains("lacks utilake.key"));
        assert!(err(&with(K_KEY, "[1]")).contains("JSON of the shape this key takes"));
        assert!(err(&with(K_BREAKS, "{}")).contains("utilake.breaks"));
        assert!(err(&with(K_MODEL_TYPES, "[]")).contains("utilake.model-types"));
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
            format!("The table metadata lacks utilake.version in the summary of snapshot {id}.")
        );
        assert_eq!(
            err(&summary(S_VERSION, Some("x"))),
            format!(
                "In the summary of snapshot {id}, utilake.version is \"x\", which is not a number."
            )
        );
        assert!(err(&summary(S_KINDS, Some("archive"))).contains("utilake.kinds"));
        assert!(err(&summary("utilake.to.file", None)).contains("utilake.to.file"));
        let mut unknown = m.clone();
        unknown.snapshots[0].schema_id = 9;
        assert_eq!(
            err(&unknown),
            "A snapshot was written under schema 9, which the table lacks."
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
            "The run beginning at C.zip was released in \"2024\", which is not a month (YYYY-MM)."
        );
    }
}
