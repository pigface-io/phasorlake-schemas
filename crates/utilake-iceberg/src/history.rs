//! A table's history as an Iceberg table: what the fold writes, what
//! [`crate::publish`] turns into table metadata and back, and what
//! [`crate::compat`] compares across commits.
//!
//! - **Schemas** are Iceberg schemas by id. A field's id never changes
//!   meaning: one name and one type for its life.
//! - **Runs** are AEMO's files in the order AEMO released them, grouped
//!   while the schema, the AEMO version and the report pair stay the same;
//!   each says what changed at its start and the schema updates from the
//!   run before.
//! - **Breaking** lists the columns whose values left their field's type,
//!   so they needed a new field.
//!
//! What a run says of its start (`changes`, `updates`, `overlaps`) is a
//! function of the runs before it ([`derive_run`]): the fold computes it
//! as it adds a run, and a reader of the table metadata computes it again
//! from the snapshots, the same way.

use serde::{Deserialize, Serialize};

use crate::schema::{Field, Schema};

/// One table's history.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct History {
    /// The table.
    pub table: String,
    /// The Iceberg format version: 3.
    pub format_version: u8,
    /// The schema of the last run: what a reader of the table today gets.
    /// A document from before the field was written has schema 0.
    #[serde(default)]
    pub current_schema_id: u32,
    /// Every schema, by id.
    #[serde(with = "crate::schema::serde_list")]
    pub schemas: Vec<Schema>,
    /// Every run, in release order.
    pub runs: Vec<Run>,
    /// The columns that needed a new field.
    pub breaking: Vec<Break>,
}

/// Where a run starts or ends: one AEMO file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct Mark {
    /// The file's name.
    pub file: String,
    /// Its SHA-256.
    pub sha256: String,
    /// The month AEMO released it in, `YYYY-MM`: the monthly archive it
    /// belongs to, or a report file's month. Runs are in release order.
    pub release: String,
    /// Where its data starts (at a run's start) or ends (at its end), in
    /// the market's time, RFC 3339; none when its rows carry no time.
    pub data_time: Option<String>,
}

/// What changed at the start of a run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Change {
    /// The table's first run.
    First,
    /// Another schema.
    Schema,
    /// Another AEMO version of the report.
    Version,
    /// Another report pair.
    Report,
    /// A run the published history began here, kept whole: the files
    /// before it, added later, do not join it.
    Published,
}

/// An Iceberg schema update from one run's schema to the next.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "kebab-case")]
pub enum Update {
    /// A field no earlier schema had.
    Add {
        /// The field.
        #[serde(rename = "field-id")]
        field_id: u32,
    },
    /// A field an earlier schema had, back.
    Restore {
        /// The field.
        #[serde(rename = "field-id")]
        field_id: u32,
    },
    /// A field the schema before had and this one lacks.
    Delete {
        /// The field.
        #[serde(rename = "field-id")]
        field_id: u32,
    },
    /// A field whose place among the others changed.
    Move {
        /// The field.
        #[serde(rename = "field-id")]
        field_id: u32,
    },
    /// A field that now takes nulls.
    MakeOptional {
        /// The field.
        #[serde(rename = "field-id")]
        field_id: u32,
    },
    /// The key's fields.
    SetIdentifierFields {
        /// The fields.
        #[serde(rename = "field-ids")]
        field_ids: Vec<u32>,
    },
}

/// Files in a row under one schema, AEMO version and report pair.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct Run {
    /// The schema.
    pub schema_id: u32,
    /// The report type and subtype.
    pub report: [String; 2],
    /// AEMO's version of the report.
    pub version: u16,
    /// The kinds of file it came in (`archive`, `current`, …), sorted.
    pub kinds: Vec<String>,
    /// Its first file.
    pub from: Mark,
    /// Its last.
    pub to: Mark,
    /// Files.
    pub files: u64,
    /// Rows.
    pub rows: u64,
    /// What changed at its start.
    pub changes: Vec<Change>,
    /// The updates from the run before.
    pub updates: Vec<Update>,
    /// Whether its data starts before the run before it ends.
    pub overlaps: bool,
}

/// A column whose values left its field's type.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct Break {
    /// The column.
    pub column: String,
    /// The field it had.
    pub old_field_id: u32,
    /// Its type.
    pub old_type: String,
    /// The field it has from here.
    pub new_field_id: u32,
    /// Its type.
    pub new_type: String,
    /// The file it happened in.
    pub at: Mark,
}

impl History {
    /// The schema with this id.
    pub fn schema(&self, id: u32) -> Option<&Schema> {
        self.schemas.iter().find(|s| s.schema_id == id)
    }

    /// Every field any schema has, by id, as the first schema with it has
    /// it: a field has one name and one type in every schema.
    pub fn fields(&self) -> Vec<&Field> {
        let mut out: Vec<&Field> = Vec::new();
        for f in self.schemas.iter().flat_map(|s| &s.fields) {
            if let Err(i) = out.binary_search_by_key(&f.id, |g| g.id) {
                out.insert(i, f);
            }
        }
        out
    }

    /// The schema a block of report pair `report`, AEMO version `version`
    /// and columns `columns` (in file order), released in `release`
    /// (`YYYY-MM`), is read with.
    ///
    /// A run fits the block when its report pair is the block's and its
    /// schema's fields are the block's columns, in order. Of the runs that
    /// fit, those of the block's version are preferred; of those, the last
    /// released by `release`, else the first. A block no run fits has no
    /// schema.
    pub fn resolve(
        &self,
        report: [&str; 2],
        version: u16,
        columns: &[&str],
        release: &str,
    ) -> Option<u32> {
        let fits = |r: &&Run| {
            r.report == report
                && self.schema(r.schema_id).is_some_and(|s| {
                    s.fields.len() == columns.len()
                        && s.fields.iter().zip(columns).all(|(f, c)| f.name == *c)
                })
        };
        let pick = |same: bool| {
            let fit: Vec<&Run> = self
                .runs
                .iter()
                .filter(fits)
                .filter(|r| !same || r.version == version)
                .collect();
            fit.iter()
                .rev()
                .find(|r| r.from.release.as_str() <= release)
                .or(fit.first())
                .copied()
        };
        pick(true).or_else(|| pick(false)).map(|r| r.schema_id)
    }
}

/// Fills in what `run` says of its start, given the `before` runs and the
/// table's `schemas`: its changes (the first run's `first`; else which of
/// schema, AEMO version and report pair differ from the run before, or
/// `published` when none does), the schema updates from the run before,
/// and whether its data starts before that run's ends. `Err` names a
/// schema id a run has and `schemas` lacks.
pub fn derive_run(schemas: &[Schema], before: &[Run], run: &mut Run) -> Result<(), u32> {
    let schema = |id: u32| schemas.iter().find(|s| s.schema_id == id).ok_or(id);
    let Some(prev) = before.last() else {
        run.changes = vec![Change::First];
        run.updates.clear();
        run.overlaps = false;
        return Ok(());
    };
    let mut changes = Vec::new();
    if prev.schema_id != run.schema_id {
        changes.push(Change::Schema);
    }
    if prev.version != run.version {
        changes.push(Change::Version);
    }
    if prev.report != run.report {
        changes.push(Change::Report);
    }
    if changes.is_empty() {
        changes.push(Change::Published);
    }
    let earlier = before
        .iter()
        .map(|r| schema(r.schema_id))
        .collect::<Result<Vec<_>, _>>()?;
    run.updates = updates(schema(prev.schema_id)?, schema(run.schema_id)?, &earlier);
    run.overlaps = match (&prev.to.data_time, &run.from.data_time) {
        (Some(end), Some(start)) => start < end,
        _ => false,
    };
    run.changes = changes;
    Ok(())
}

/// [`derive_run`] over every run in order.
pub fn derive(schemas: &[Schema], runs: &mut [Run]) -> Result<(), u32> {
    for i in 0..runs.len() {
        let (before, rest) = runs.split_at_mut(i);
        derive_run(schemas, before, &mut rest[0])?;
    }
    Ok(())
}

/// The positions, in `seq`, outside one longest increasing subsequence:
/// the fewest elements that moved.
fn outside_lis(seq: &[usize]) -> Vec<bool> {
    let mut tails: Vec<usize> = Vec::new();
    let mut back = vec![usize::MAX; seq.len()];
    for (i, v) in seq.iter().enumerate() {
        let k = tails.partition_point(|&t| seq[t] < *v);
        if k > 0 {
            back[i] = tails[k - 1];
        }
        if k == tails.len() {
            tails.push(i);
        } else {
            tails[k] = i;
        }
    }
    let mut outside = vec![true; seq.len()];
    let mut i = tails.last().copied().unwrap_or(usize::MAX);
    while i != usize::MAX {
        outside[i] = false;
        i = back[i];
    }
    outside
}

/// The updates that take `a` to `b`; `earlier` holds the schemas of every
/// run so far, so a field back from one of them is a restore, not an add.
pub fn updates(a: &Schema, b: &Schema, earlier: &[&Schema]) -> Vec<Update> {
    let mut out = Vec::new();
    for f in &a.fields {
        if b.field(f.id).is_none() {
            out.push(Update::Delete { field_id: f.id });
        }
    }
    for f in &b.fields {
        match a.field(f.id) {
            None if earlier.iter().any(|s| s.field(f.id).is_some()) => {
                out.push(Update::Restore { field_id: f.id });
            }
            None => out.push(Update::Add { field_id: f.id }),
            Some(old) => {
                if old.required && !f.required {
                    out.push(Update::MakeOptional { field_id: f.id });
                }
            }
        }
    }
    let shared: Vec<u32> = b
        .fields
        .iter()
        .filter(|f| a.field(f.id).is_some())
        .map(|f| f.id)
        .collect();
    let positions: Vec<usize> = shared
        .iter()
        .filter_map(|id| a.fields.iter().position(|f| f.id == *id))
        .collect();
    for (id, moved) in shared.iter().zip(outside_lis(&positions)) {
        if moved {
            out.push(Update::Move { field_id: *id });
        }
    }
    let (mut ka, mut kb) = (
        a.identifier_field_ids.clone(),
        b.identifier_field_ids.clone(),
    );
    ka.sort_unstable();
    kb.sort_unstable();
    if ka != kb {
        out.push(Update::SetIdentifierFields {
            field_ids: b.identifier_field_ids.clone(),
        });
    }
    out
}

#[cfg(test)]
pub(crate) mod tests {
    use serde_json::json;

    use super::*;

    /// A history of two schemas and two runs, as the export writes it.
    pub fn sample() -> serde_json::Value {
        let mark = |file: &str, t: Option<&str>| json!({"file": file, "sha256": "ab", "release": "2024-08", "data-time": t});
        json!({
            "table": "UNIT",
            "format-version": 3,
            "current-schema-id": 1,
            "schemas": [
                {"type": "struct", "schema-id": 0, "identifier-field-ids": [1], "fields": [
                    {"id": 1, "name": "SETTLEMENTDATE", "type": "timestamptz", "required": true},
                    {"id": 2, "name": "SCADAVALUE", "type": "decimal(16, 6)", "required": false},
                ]},
                {"type": "struct", "schema-id": 1, "identifier-field-ids": [1], "fields": [
                    {"id": 1, "name": "SETTLEMENTDATE", "type": "timestamptz", "required": true},
                    {"id": 3, "name": "FLAG", "type": "boolean", "required": false},
                    {"id": 2, "name": "SCADAVALUE", "type": "decimal(16, 6)", "required": false},
                ]},
            ],
            "runs": [
                {"schema-id": 0, "report": ["DISPATCH", "UNIT_SCADA"], "version": 1, "kinds": ["archive"],
                 "from": mark("A.zip", Some("2024-08-01T00:05:00+10:00")), "to": mark("B.zip", None),
                 "files": 2, "rows": 9, "changes": ["first"], "updates": [], "overlaps": false},
                {"schema-id": 1, "report": ["DISPATCH", "UNIT_SCADA"], "version": 2, "kinds": ["archive", "current"],
                 "from": mark("C.zip", None), "to": mark("C.zip", None), "files": 1, "rows": 3,
                 "changes": ["schema", "version"],
                 "updates": [{"action": "add", "field-id": 3}, {"action": "move", "field-id": 2},
                             {"action": "restore", "field-id": 9}, {"action": "delete", "field-id": 8},
                             {"action": "make-optional", "field-id": 7},
                             {"action": "set-identifier-fields", "field-ids": [1]}],
                 "overlaps": true},
            ],
            "breaking": [
                {"column": "X", "old-field-id": 4, "old-type": "long", "new-field-id": 5,
                 "new-type": "string", "at": mark("C.zip", None)},
            ],
        })
    }

    #[test]
    fn the_document_round_trips() {
        let v = sample();
        let h: History = serde_json::from_value(v.clone()).unwrap();
        assert_eq!(serde_json::to_value(&h).unwrap(), v);
        assert_eq!(h.runs[1].changes, [Change::Schema, Change::Version]);
        assert_eq!(h.current_schema_id, 1);
        let mut old = v.clone();
        old.as_object_mut().unwrap().remove("current-schema-id");
        let h: History = serde_json::from_value(old).unwrap();
        assert_eq!(h.current_schema_id, 0);
        assert_eq!(
            serde_json::to_value(Change::Published).unwrap(),
            json!("published")
        );
        assert_eq!(h.runs[1].updates[0], Update::Add { field_id: 3 });
        assert_eq!(h.schema(1).map(|s| s.fields.len()), Some(3));
        assert!(h.schema(7).is_none());
        let ids: Vec<u32> = h.fields().iter().map(|f| f.id).collect();
        assert_eq!(ids, [1, 2, 3]);
    }

    #[test]
    fn what_a_run_says_of_its_start_is_derived_from_the_runs_before() {
        let mut h: History = serde_json::from_value(sample()).unwrap();
        // The hand-written sample names updates of fields no schema has;
        // derived, the second run's is the add and nothing else: a field
        // put between two others moves neither.
        derive(&h.schemas, &mut h.runs).unwrap();
        assert_eq!(h.runs[0].changes, [Change::First]);
        assert!(h.runs[0].updates.is_empty() && !h.runs[0].overlaps);
        assert_eq!(h.runs[1].changes, [Change::Schema, Change::Version]);
        assert_eq!(h.runs[1].updates, [Update::Add { field_id: 3 }]);
        assert!(!h.runs[1].overlaps, "the second run's data has no time");
        // A third run back on schema 0 restores nothing (field 2 never
        // left), deletes 3, and overlaps when its data starts before the
        // second run's ends; the same schema, version and pair as the run
        // before is a published boundary.
        let mut third = h.runs[1].clone();
        third.schema_id = 0;
        third.from.data_time = Some("2024-08-01T00:00:00+10:00".into());
        h.runs[1].to.data_time = Some("2024-08-02T00:00:00+10:00".into());
        h.runs.push(third);
        let mut fourth = h.runs[1].clone();
        fourth.changes = vec![Change::First];
        h.runs.push(fourth);
        derive(&h.schemas, &mut h.runs).unwrap();
        assert_eq!(h.runs[2].changes, [Change::Schema]);
        assert_eq!(h.runs[2].updates, [Update::Delete { field_id: 3 }]);
        assert!(h.runs[2].overlaps);
        assert_eq!(h.runs[3].changes, [Change::Schema]);
        assert_eq!(h.runs[3].updates, [Update::Restore { field_id: 3 }]);
        let mut fifth = h.runs[3].clone();
        fifth.from.data_time = None;
        h.runs.push(fifth);
        derive(&h.schemas, &mut h.runs).unwrap();
        assert_eq!(h.runs[4].changes, [Change::Published]);
        assert!(h.runs[4].updates.is_empty() && !h.runs[4].overlaps);
        // A run of a schema the table lacks.
        h.runs[4].schema_id = 9;
        assert_eq!(derive(&h.schemas, &mut h.runs), Err(9));
        h.runs[4].schema_id = 1;
        h.runs[0].schema_id = 9;
        assert_eq!(derive(&h.schemas, &mut h.runs), Err(9));
    }

    #[test]
    fn updates_are_the_fewest_moves_and_name_the_key() {
        let field = |id, required| Field {
            id,
            name: format!("F{id}"),
            ty: crate::IcebergType::Long,
            required,
            doc: None,
        };
        let schema = |ids: &[u32], key: &[u32]| Schema {
            schema_id: 0,
            fields: ids.iter().map(|id| field(*id, true)).collect(),
            identifier_field_ids: key.to_vec(),
            last_column_id: 0,
        };
        let a = schema(&[1, 2, 3, 4], &[1]);
        // Field 4 moved to the front: one move, not three.
        let b = schema(&[4, 1, 2, 3], &[1]);
        assert_eq!(updates(&a, &b, &[]), [Update::Move { field_id: 4 }]);
        // The key in another order is the same key.
        let c = schema(&[1, 2, 3, 4], &[2, 1]);
        assert_eq!(updates(&c, &schema(&[1, 2, 3, 4], &[1, 2]), &[]), []);
        assert_eq!(
            updates(&a, &c, &[]),
            [Update::SetIdentifierFields {
                field_ids: vec![2, 1]
            }]
        );
        // A required field made optional.
        let mut d = a.clone();
        d.fields[1] = field(2, false);
        assert_eq!(updates(&a, &d, &[]), [Update::MakeOptional { field_id: 2 }]);
        assert_eq!(outside_lis(&[]), Vec::<bool>::new());
    }

    #[test]
    fn a_block_resolves_to_the_run_its_columns_and_release_fit() {
        let mut h: History = serde_json::from_value(sample()).unwrap();
        let pair = ["DISPATCH", "UNIT_SCADA"];
        let old = ["SETTLEMENTDATE", "SCADAVALUE"];
        let new = ["SETTLEMENTDATE", "FLAG", "SCADAVALUE"];
        assert_eq!(h.resolve(pair, 1, &old, "2024-08"), Some(0));
        assert_eq!(h.resolve(pair, 2, &new, "2024-08"), Some(1));
        // An unseen version reads with the columns' schema.
        assert_eq!(h.resolve(pair, 9, &new, "2030-01"), Some(1));
        // Before every fitting run, the first.
        assert_eq!(h.resolve(pair, 1, &old, "2001-01"), Some(0));
        // No run fits: another pair, other columns, other order.
        assert_eq!(h.resolve(["DISPATCH", "PRICE"], 1, &old, "2024-08"), None);
        assert_eq!(h.resolve(pair, 1, &old[..1], "2024-08"), None);
        assert_eq!(
            h.resolve(pair, 1, &["SCADAVALUE", "SETTLEMENTDATE"], "2024-08"),
            None
        );
        // A run whose schema is missing fits nothing.
        h.runs[0].schema_id = 7;
        assert_eq!(h.resolve(pair, 1, &old, "2024-08"), None);
        // Two runs of the same columns: the last released by the month.
        let mut h: History = serde_json::from_value(sample()).unwrap();
        let mut later = h.runs[0].clone();
        later.schema_id = 1;
        later.from.release = "2025-01".into();
        h.schemas[1].fields = h.schemas[0].fields.clone();
        h.runs.push(later);
        assert_eq!(h.resolve(pair, 1, &old, "2024-12"), Some(0));
        assert_eq!(h.resolve(pair, 1, &old, "2025-01"), Some(1));
    }
}
