//! What a published history promises its readers, checked against the next
//! version of it.
//!
//! - A field id keeps its name and its type. Data its type cannot hold
//!   gets a new field; the old one is never retyped.
//! - A schema id keeps its fields, their order and nullability, and its
//!   key, so a published schema document never changes.
//! - A file keeps its schema: the file that began a published run still
//!   begins a run of the same schema, report pair and version, and the
//!   file that ended it is still under that schema. Runs may split, take
//!   in files, or have others before them, as evidence from before or
//!   between them is added.
//! - A breaking column stays broken: its new field is still what the
//!   column became.
//!
//! A history may gain fields, schemas, runs and breaks freely.

use std::fmt;

use crate::history::History;

/// A promise the new history breaks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Violation {
    /// A field the old history has is gone.
    FieldGone {
        /// The field.
        id: u32,
    },
    /// A field has another name or type.
    FieldChanged {
        /// The field.
        id: u32,
        /// Its name and type before.
        was: String,
        /// And now.
        now: String,
    },
    /// A schema the old history has is gone.
    SchemaGone {
        /// The schema.
        id: u32,
    },
    /// A schema has other fields, order, nullability or key.
    SchemaChanged {
        /// The schema.
        id: u32,
    },
    /// The file that began a run begins none now.
    RunGone {
        /// The run's position in the old history.
        index: usize,
    },
    /// The file that began a run begins one of another schema, report
    /// pair or version.
    RunChanged {
        /// The run's position in the old history.
        index: usize,
    },
    /// The file that ended a run is no longer under its schema: no run of
    /// that schema, report pair and version reaches its data.
    RunCut {
        /// The run's position in the old history.
        index: usize,
    },
    /// A breaking column no longer breaks into the field it did.
    BreakChanged {
        /// Its position in the old history.
        index: usize,
    },
}

impl fmt::Display for Violation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::FieldGone { id } => write!(f, "field {id} is gone"),
            Self::FieldChanged { id, was, now } => {
                write!(f, "field {id} was {was} and is now {now}")
            }
            Self::SchemaGone { id } => write!(f, "schema {id} is gone"),
            Self::SchemaChanged { id } => write!(f, "schema {id} has changed"),
            Self::RunGone { index } => write!(f, "run {index}'s first file begins no run"),
            Self::RunChanged { index } => write!(f, "run {index} has changed"),
            Self::RunCut { index } => {
                write!(f, "run {index}'s last file is no longer under its schema")
            }
            Self::BreakChanged { index } => write!(f, "breaking column {index} has changed"),
        }
    }
}

/// Every promise `new` breaks of `old`, the history of the same table.
pub fn check(old: &History, new: &History) -> Vec<Violation> {
    let mut out = Vec::new();
    let now = new.fields();
    for f in old.fields() {
        let label = |f: &crate::Field| format!("{} {}", f.name, f.ty);
        match now.iter().find(|g| g.id == f.id) {
            None => out.push(Violation::FieldGone { id: f.id }),
            Some(g) if g.name != f.name || g.ty != f.ty => out.push(Violation::FieldChanged {
                id: f.id,
                was: label(f),
                now: label(g),
            }),
            Some(_) => {}
        }
    }
    for s in &old.schemas {
        match new.schema(s.schema_id) {
            None => out.push(Violation::SchemaGone { id: s.schema_id }),
            Some(n) if !n.same_as(s) => out.push(Violation::SchemaChanged { id: s.schema_id }),
            Some(_) => {}
        }
    }
    for (index, o) in old.runs.iter().enumerate() {
        let same = |n: &&crate::history::Run| {
            n.schema_id == o.schema_id && n.report == o.report && n.version == o.version
        };
        // Several runs may begin at one file (a report folder's zip carries
        // more than one version of a report), so the run held is the one
        // that begins there with the old run's schema, report and version.
        let mut begun = new.runs.iter().filter(|n| n.from == o.from);
        match begun.clone().find(same) {
            None if begun.next().is_none() => out.push(Violation::RunGone { index }),
            None => out.push(Violation::RunChanged { index }),
            Some(_) => {
                let reached = new
                    .runs
                    .iter()
                    .filter(same)
                    .any(|n| n.to.data_time >= o.to.data_time);
                if !reached {
                    out.push(Violation::RunCut { index });
                }
            }
        }
    }
    for (index, b) in old.breaking.iter().enumerate() {
        let kept = new
            .breaking
            .iter()
            .any(|n| n.column == b.column && n.new_field_id == b.new_field_id);
        if !kept {
            out.push(Violation::BreakChanged { index });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::IcebergType;
    use crate::history::tests::sample;
    use crate::history::{Change, Mark};

    fn history() -> History {
        serde_json::from_value(sample()).unwrap()
    }

    fn mark(file: &str, t: Option<&str>) -> Mark {
        Mark {
            file: file.into(),
            sha256: "ab".into(),
            release: "2024-07".into(),
            data_time: t.map(str::to_owned),
        }
    }

    /// Evidence from before, between and after the published runs: a run
    /// before the first, the first split around a run of another schema,
    /// files at the end of the last, a new schema, run and break.
    #[test]
    fn a_history_may_grow_at_either_end_and_between() {
        let old = history();
        assert!(check(&old, &old).is_empty());
        let mut new = old.clone();
        let mut before = old.runs[0].clone();
        before.from = mark("0.zip", Some("2024-07-01T00:05:00+10:00"));
        before.to = mark("0.zip", Some("2024-07-31T00:00:00+10:00"));
        let mut head = old.runs[0].clone();
        head.to = head.from.clone();
        head.files = 1;
        head.changes = vec![Change::Schema, Change::Published];
        let mut between = old.runs[1].clone();
        between.from = mark("A2.zip", None);
        between.to = between.from.clone();
        let mut tail = old.runs[0].clone();
        tail.from = tail.to.clone();
        tail.files = 1;
        new.runs = vec![before, head, between, tail];
        let mut last = old.runs[1].clone();
        last.files += 4;
        last.rows += 40;
        last.kinds.push("other".into());
        last.to = mark("D.zip", Some("2024-09-01T00:00:00+10:00"));
        new.runs.push(last);
        let mut extra = old.runs[1].clone();
        extra.schema_id = 2;
        extra.from = mark("E.zip", None);
        new.runs.push(extra);
        // Two runs begin at one file, under one schema, as versions 1 and
        // 2 of the report: each is held to its own.
        let mut twin = old.runs[1].clone();
        twin.version += 1;
        new.runs.push(twin);
        let mut s = new.schemas[1].clone();
        s.schema_id = 2;
        new.schemas.push(s);
        new.breaking.push(new.breaking[0].clone());
        assert!(check(&old, &new).is_empty());
        // The twin at another version only: the run begins there, changed.
        let mut twins = new.clone();
        let i = twins.runs.len() - 1;
        twins.runs[i].version += 1;
        assert!(check(&twins, &twins).is_empty());
        assert_eq!(check(&new, &twins), [Violation::RunChanged { index: i }]);
    }

    #[test]
    fn every_broken_promise_is_named() {
        let old = history();
        let mut new = old.clone();
        // Field 2 retyped in every schema, field 3 renamed in schema 1.
        for s in &mut new.schemas {
            for f in &mut s.fields {
                if f.id == 2 {
                    f.ty = IcebergType::String;
                }
            }
        }
        new.schemas[1].fields[1].name = "FLAG2".into();
        // Run 0 begins another schema; run 1's first file begins nothing.
        new.runs[0].schema_id = 1;
        new.runs[1].from.file = "Z.zip".into();
        new.breaking.clear();
        let v = check(&old, &new);
        assert_eq!(
            v,
            [
                Violation::FieldChanged {
                    id: 2,
                    was: "SCADAVALUE decimal(16, 6)".into(),
                    now: "SCADAVALUE string".into()
                },
                Violation::FieldChanged {
                    id: 3,
                    was: "FLAG boolean".into(),
                    now: "FLAG2 boolean".into()
                },
                Violation::SchemaChanged { id: 0 },
                Violation::SchemaChanged { id: 1 },
                Violation::RunChanged { index: 0 },
                Violation::RunGone { index: 1 },
                Violation::BreakChanged { index: 0 },
            ]
        );
        // A break into another field is a change; the same column into the
        // same field, from elsewhere, is not.
        let mut moved = old.clone();
        moved.breaking[0].old_field_id = 7;
        moved.breaking[0].at = mark("Q.zip", None);
        assert!(check(&old, &moved).is_empty());
        moved.breaking[0].new_field_id = 9;
        assert_eq!(check(&old, &moved), [Violation::BreakChanged { index: 0 }]);
        // The last file of run 0 no longer reached under its schema.
        let mut old2 = old.clone();
        old2.runs[0].to.data_time = Some("2024-08-02T00:00:00+10:00".into());
        let mut cut = old2.clone();
        cut.runs[0].to.data_time = Some("2024-08-01T12:00:00+10:00".into());
        assert_eq!(check(&old2, &cut), [Violation::RunCut { index: 0 }]);
        // Files that went: a schema and a run gone.
        let mut gone = old.clone();
        gone.schemas.truncate(1);
        gone.runs.truncate(1);
        let v = check(&old, &gone);
        assert_eq!(
            v,
            [
                Violation::FieldGone { id: 3 },
                Violation::SchemaGone { id: 1 },
                Violation::RunGone { index: 1 }
            ]
        );
        let text: Vec<String> = [
            Violation::FieldGone { id: 3 },
            Violation::FieldChanged {
                id: 2,
                was: "A long".into(),
                now: "A string".into(),
            },
            Violation::SchemaGone { id: 1 },
            Violation::SchemaChanged { id: 1 },
            Violation::RunGone { index: 0 },
            Violation::RunChanged { index: 1 },
            Violation::RunCut { index: 2 },
            Violation::BreakChanged { index: 0 },
        ]
        .iter()
        .map(ToString::to_string)
        .collect();
        assert_eq!(
            text,
            [
                "field 3 is gone",
                "field 2 was A long and is now A string",
                "schema 1 is gone",
                "schema 1 has changed",
                "run 0's first file begins no run",
                "run 1 has changed",
                "run 2's last file is no longer under its schema",
                "breaking column 0 has changed"
            ]
        );
    }
}
