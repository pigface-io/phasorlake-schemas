//! A table's history as an Iceberg table: the document the export
//! publishes in each `table.json` (`iceberg`), and what [`crate::compat`]
//! compares across commits.
//!
//! - **Schemas** are Iceberg schemas by id. A field's id never changes
//!   meaning: one name and one type for its life.
//! - **Runs** are AEMO's files in the order AEMO released them, grouped
//!   while the schema, the AEMO version and the report pair stay the same;
//!   each says what changed at its start and the schema updates from the
//!   run before.
//! - **Breaking** lists the columns whose values left their field's type,
//!   so they needed a new field.

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
