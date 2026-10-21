//! What a published history promises its readers, checked against the next
//! version of it.
//!
//! - A field id keeps its name and its type. Data its type cannot hold
//!   gets a new field; the old one is never retyped.
//! - A schema id keeps its fields, their order and nullability, and its
//!   key, so a published schema document never changes.
//! - Runs only grow: every run but the last stays as it was, and the last
//!   may only take in more files. The list of breaking columns only grows.
//!
//! A history may gain fields, schemas, runs and breaks freely.

use std::fmt;

use crate::history::{History, Run};

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
    /// A run is not what it was.
    RunChanged {
        /// Its position.
        index: usize,
    },
    /// Fewer runs than before.
    RunsGone {
        /// Runs before.
        was: usize,
        /// Runs now.
        now: usize,
    },
    /// A breaking column is not what it was, or is gone.
    BreakChanged {
        /// Its position.
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
            Self::RunChanged { index } => write!(f, "run {index} has changed"),
            Self::RunsGone { was, now } => write!(f, "{was} runs became {now}"),
            Self::BreakChanged { index } => write!(f, "breaking column {index} has changed"),
        }
    }
}

/// Whether `new` is `old` with more files at its end: the same run up to
/// its last file, which may be later.
fn grown(old: &Run, new: &Run) -> bool {
    old.schema_id == new.schema_id
        && old.report == new.report
        && old.version == new.version
        && old.from == new.from
        && old.changes == new.changes
        && old.updates == new.updates
        && old.overlaps == new.overlaps
        && new.files >= old.files
        && new.rows >= old.rows
        && old.kinds.iter().all(|k| new.kinds.contains(k))
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
    if new.runs.len() < old.runs.len() {
        out.push(Violation::RunsGone {
            was: old.runs.len(),
            now: new.runs.len(),
        });
    }
    let last = old.runs.len().saturating_sub(1);
    for (index, (o, n)) in old.runs.iter().zip(&new.runs).enumerate() {
        let kept = if index == last { grown(o, n) } else { o == n };
        if !kept {
            out.push(Violation::RunChanged { index });
        }
    }
    for (index, o) in old.breaking.iter().enumerate() {
        if new.breaking.get(index) != Some(o) {
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

    fn history() -> History {
        serde_json::from_value(sample()).unwrap()
    }

    #[test]
    fn a_history_may_grow() {
        let old = history();
        assert!(check(&old, &old).is_empty());
        let mut new = old.clone();
        let last = new.runs.last_mut().unwrap();
        last.files += 4;
        last.rows += 40;
        last.kinds.push("other".into());
        last.to.file = "D.zip".into();
        let mut extra = last.clone();
        extra.schema_id = 2;
        new.runs.push(extra);
        let mut s = new.schemas[1].clone();
        s.schema_id = 2;
        new.schemas.push(s);
        new.breaking.push(new.breaking[0].clone());
        assert!(check(&old, &new).is_empty());
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
        new.runs[0].files = 1;
        new.runs[1].kinds.clear();
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
                Violation::RunChanged { index: 1 },
                Violation::BreakChanged { index: 0 },
            ]
        );
        let mut gone = old.clone();
        gone.schemas.truncate(1);
        gone.runs.truncate(1);
        let v = check(&old, &gone);
        assert_eq!(
            v,
            [
                Violation::FieldGone { id: 3 },
                Violation::SchemaGone { id: 1 },
                Violation::RunsGone { was: 2, now: 1 }
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
            Violation::RunChanged { index: 1 },
            Violation::RunsGone { was: 2, now: 1 },
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
                "run 1 has changed",
                "2 runs became 1",
                "breaking column 0 has changed"
            ]
        );
    }
}
