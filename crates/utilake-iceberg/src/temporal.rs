//! A table's temporal roles, derived from its key and columns.
//!
//! AEMO's data has two clocks: when a fact applies (valid time) and when
//! a row was recorded (knowledge). The roles say which columns are which:
//! the `entity` a fact is about, the `valid` time and its axis, the
//! `order` of its revisions, and the `cutoff` a read "as known at" is held
//! to. They are a function of the data model's primary key and the
//! table's columns ([`derive()`]), so no table keeps a list by hand unless a
//! person decides otherwise in its declaration.

use crate::publish::{Temporal, Valid};
use crate::schema::Field;
use crate::types::IcebergType;

/// Valid-time columns of a period that ends at the time.
pub const INTERVAL_END: &[&str] = &[
    "SETTLEMENTDATE",
    "INTERVAL_DATETIME",
    "DATETIME",
    "PERIOD_ENDING",
];

/// Valid-time columns from which a row is in force.
pub const IN_FORCE_FROM: &[&str] = &["EFFECTIVEDATE", "VALIDFROM", "START_DATE", "STARTDATE"];

/// Columns that end a version before the next one starts, on a table
/// whose rows are in force from a time.
pub const UNTIL: &[&str] = &["END_DATE", "ENDDATE", "VALIDTO", "VALID_TO"];

/// Columns that rank revisions. A time among them may also be a cutoff,
/// and is the valid time of a table that has no other.
pub const KNOWLEDGE: &[&str] = &[
    "RUN_DATETIME",
    "VERSION_DATETIME",
    "PUBLISH_DATETIME",
    "OFFERDATETIME",
    "OFFERDATE",
    "PREDISPATCHSEQNO",
    "VERSIONNO",
    "BILLRUNNO",
    "RUNNO",
    "RUN_NO",
];

/// A column that names a scenario, never a revision: AEMO publishes an
/// intervention run's rows beside the normal ones, and leaves it out of
/// some keys (the P5MIN and PREDISPATCH tables). Merging the two would
/// lose one, so it is always an entity where the table has it.
pub const SCENARIO: &str = "INTERVENTION";

/// The column every revision of most tables carries: when AEMO last
/// changed the row.
pub const LAST_CHANGED: &str = "LASTCHANGED";

/// The origin a derived role set carries.
pub const DERIVED: &str = "derived";

fn is_time(f: &Field) -> bool {
    matches!(f.ty, IcebergType::Timestamptz | IcebergType::Date)
}

/// The roles of a table, from its primary key and columns.
///
/// - The valid time is the first key column in [`IN_FORCE_FROM`] (`from`,
///   with the first time column in [`UNTIL`] as its end), else in
///   [`INTERVAL_END`] (`interval_end`), else the first other time in the
///   key that is not a knowledge column (`instant`), else the first time
///   among the key's knowledge columns (`instant`: a run's own result is a
///   fact of that run), else none (`timeless`).
/// - The order is the key's [`KNOWLEDGE`] columns in key order, then the
///   knowledge columns outside the key, then [`LAST_CHANGED`].
/// - The cutoff is the first time among the key's knowledge columns, else
///   [`LAST_CHANGED`], else none.
/// - Every other key column is the entity, and [`SCENARIO`] wherever the
///   table has it.
///
/// A column named like a time but not typed as one is none of these.
pub fn derive(primary_key: &[String], fields: &[Field]) -> Temporal {
    let col = |n: &str| fields.iter().find(|f| f.name == n);
    let timed = |n: &&String| col(n).is_some_and(is_time);
    let knowledge = |n: &str| KNOWLEDGE.contains(&n);
    let in_key = |list: &[&str]| {
        primary_key
            .iter()
            .find(|k| list.contains(&k.as_str()))
            .cloned()
    };
    let key_knowledge_time = primary_key
        .iter()
        .filter(timed)
        .find(|k| knowledge(k))
        .cloned();
    let axis = |kind: &str, column: String| Valid {
        kind: kind.to_owned(),
        column: Some(column),
        until: None,
    };
    let valid = if let Some(c) = in_key(IN_FORCE_FROM).filter(|c| timed(&c)) {
        Valid {
            until: UNTIL
                .iter()
                .find_map(|n| col(n).filter(|f| is_time(f)))
                .map(|f| f.name.clone()),
            ..axis("from", c)
        }
    } else if let Some(c) = in_key(INTERVAL_END).filter(|c| timed(&c)) {
        axis("interval_end", c)
    } else if let Some(c) = primary_key
        .iter()
        .filter(timed)
        .find(|k| !knowledge(k))
        .cloned()
    {
        axis("instant", c)
    } else if let Some(c) = key_knowledge_time.clone() {
        axis("instant", c)
    } else {
        Valid {
            kind: "timeless".to_owned(),
            column: None,
            until: None,
        }
    };
    let is_valid = |n: &str| valid.column.as_deref() == Some(n);
    let mut order: Vec<String> = primary_key
        .iter()
        .filter(|k| knowledge(k) && !is_valid(k))
        .cloned()
        .collect();
    for k in KNOWLEDGE.iter().chain(&[LAST_CHANGED]) {
        if col(k).is_some() && !order.iter().any(|o| o == k) && !is_valid(k) {
            order.push((*k).to_owned());
        }
    }
    let cutoff = key_knowledge_time.filter(|c| !is_valid(c)).or_else(|| {
        col(LAST_CHANGED)
            .filter(|f| is_time(f))
            .map(|f| f.name.clone())
    });
    let mut entity: Vec<String> = primary_key
        .iter()
        .filter(|k| !is_valid(k) && !order.contains(k))
        .cloned()
        .collect();
    if col(SCENARIO).is_some() && !entity.iter().any(|e| e == SCENARIO) {
        entity.push(SCENARIO.to_owned());
    }
    Temporal {
        valid,
        entity,
        order,
        cutoff,
        origin: DERIVED.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fields(spec: &[(&str, IcebergType)]) -> Vec<Field> {
        spec.iter()
            .enumerate()
            .map(|(i, (n, ty))| Field {
                id: i as u32 + 1,
                name: (*n).to_owned(),
                ty: *ty,
                required: false,
                doc: None,
            })
            .collect()
    }

    fn names(n: &[&str]) -> Vec<String> {
        n.iter().map(|s| (*s).to_owned()).collect()
    }

    fn valid(kind: &str, column: &str) -> Valid {
        Valid {
            kind: kind.into(),
            column: Some(column.into()),
            until: None,
        }
    }

    const TS: IcebergType = IcebergType::Timestamptz;
    const S: IcebergType = IcebergType::String;
    const D: IcebergType = IcebergType::Double;

    #[test]
    fn effective_dated() {
        let c = fields(&[
            ("DUID", S),
            ("EFFECTIVEDATE", TS),
            ("VERSIONNO", D),
            ("LASTCHANGED", TS),
        ]);
        let t = derive(&names(&["DUID", "EFFECTIVEDATE", "VERSIONNO"]), &c);
        assert_eq!(t.entity, names(&["DUID"]));
        assert_eq!(t.valid, valid("from", "EFFECTIVEDATE"));
        assert_eq!(t.order, names(&["VERSIONNO", "LASTCHANGED"]));
        assert_eq!(t.cutoff.as_deref(), Some("LASTCHANGED"));
        assert_eq!(t.origin, DERIVED);
    }

    #[test]
    fn a_version_ends_at_its_end_column_when_the_table_has_one() {
        let c = fields(&[
            ("DUID", S),
            ("START_DATE", IcebergType::Date),
            ("END_DATE", IcebergType::Date),
            ("LASTCHANGED", TS),
        ]);
        let t = derive(&names(&["DUID", "START_DATE"]), &c);
        assert_eq!(
            t.valid,
            Valid {
                until: Some("END_DATE".into()),
                ..valid("from", "START_DATE")
            }
        );
        // An end column typed as text is not one; an end column on a
        // table that is not in force from a time is not one either.
        let text_end = fields(&[("DUID", S), ("START_DATE", TS), ("END_DATE", S)]);
        assert_eq!(
            derive(&names(&["DUID", "START_DATE"]), &text_end).valid,
            valid("from", "START_DATE")
        );
        let interval = fields(&[("SETTLEMENTDATE", TS), ("END_DATE", TS)]);
        assert_eq!(
            derive(&names(&["SETTLEMENTDATE"]), &interval).valid,
            valid("interval_end", "SETTLEMENTDATE")
        );
    }

    #[test]
    fn interval_with_runs() {
        let c = fields(&[
            ("SETTLEMENTDATE", TS),
            ("RUNNO", D),
            ("REGIONID", S),
            ("INTERVENTION", D),
            ("LASTCHANGED", TS),
        ]);
        let t = derive(
            &names(&["SETTLEMENTDATE", "RUNNO", "REGIONID", "INTERVENTION"]),
            &c,
        );
        assert_eq!(t.valid, valid("interval_end", "SETTLEMENTDATE"));
        assert_eq!(t.entity, names(&["REGIONID", "INTERVENTION"]));
        assert_eq!(t.order, names(&["RUNNO", "LASTCHANGED"]));
    }

    #[test]
    fn forecast_runs_are_knowledge() {
        let c = fields(&[
            ("RUN_DATETIME", TS),
            ("INTERVAL_DATETIME", TS),
            ("REGIONID", S),
            ("LASTCHANGED", TS),
        ]);
        let t = derive(
            &names(&["RUN_DATETIME", "INTERVAL_DATETIME", "REGIONID"]),
            &c,
        );
        assert_eq!(t.valid, valid("interval_end", "INTERVAL_DATETIME"));
        assert_eq!(t.order, names(&["RUN_DATETIME", "LASTCHANGED"]));
        assert_eq!(t.cutoff.as_deref(), Some("RUN_DATETIME"));
        assert_eq!(t.entity, names(&["REGIONID"]));
    }

    #[test]
    fn a_runs_own_result_is_valid_at_the_run() {
        let c = fields(&[("RUN_DATETIME", TS), ("LASTCHANGED", TS)]);
        let t = derive(&names(&["RUN_DATETIME"]), &c);
        assert_eq!(t.valid, valid("instant", "RUN_DATETIME"));
        assert_eq!(t.order, names(&["LASTCHANGED"]));
        assert_eq!(t.cutoff.as_deref(), Some("LASTCHANGED"));
        assert!(t.entity.is_empty());
    }

    #[test]
    fn a_day_label_with_a_run() {
        let c = fields(&[("RUN_DATETIME", TS), ("DAY", TS), ("REGIONID", S)]);
        let t = derive(&names(&["RUN_DATETIME", "DAY", "REGIONID"]), &c);
        assert_eq!(t.valid, valid("instant", "DAY"));
        assert_eq!(t.order, names(&["RUN_DATETIME"]));
        assert_eq!(t.cutoff.as_deref(), Some("RUN_DATETIME"));
    }

    #[test]
    fn a_knowledge_column_outside_the_key_ranks_revisions() {
        let c = fields(&[
            ("PREDISPATCHSEQNO", S),
            ("REGIONID", S),
            ("DATETIME", TS),
            ("LASTCHANGED", TS),
        ]);
        let t = derive(&names(&["REGIONID", "DATETIME"]), &c);
        assert_eq!(t.order, names(&["PREDISPATCHSEQNO", "LASTCHANGED"]));
        let mut with_iv = c.clone();
        with_iv.push(fields(&[("INTERVENTION", D)]).remove(0));
        let t = derive(&names(&["REGIONID", "DATETIME"]), &with_iv);
        assert_eq!(t.entity, names(&["REGIONID", "INTERVENTION"]));
        assert_eq!(t.cutoff.as_deref(), Some("LASTCHANGED"));
    }

    #[test]
    fn no_key_no_time() {
        let c = fields(&[
            ("CONTRACTYEAR", D),
            ("WEEKNO", D),
            ("BILLRUNNO", D),
            ("X", D),
        ]);
        let t = derive(&names(&["CONTRACTYEAR", "WEEKNO", "BILLRUNNO"]), &c);
        assert_eq!(
            t.valid,
            Valid {
                kind: "timeless".into(),
                column: None,
                until: None
            }
        );
        assert_eq!(t.order, names(&["BILLRUNNO"]));
        assert_eq!(t.entity, names(&["CONTRACTYEAR", "WEEKNO"]));
        assert_eq!(t.cutoff, None);
        let empty = derive(&[], &c);
        assert_eq!(empty.valid.kind, "timeless");
        assert!(empty.entity.is_empty());
    }

    #[test]
    fn a_time_named_like_a_valid_column_but_typed_as_text_is_not_one() {
        let c = fields(&[("EFFECTIVEDATE", S), ("DUID", S)]);
        let t = derive(&names(&["EFFECTIVEDATE", "DUID"]), &c);
        assert_eq!(t.valid.kind, "timeless");
        assert_eq!(t.entity, names(&["EFFECTIVEDATE", "DUID"]));
    }
}
