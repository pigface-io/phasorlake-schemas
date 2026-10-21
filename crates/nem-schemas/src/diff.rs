//! `diff --base REV`: what the working tree's `nem/` adds to what `REV`
//! published, per table, as Markdown for a pull request.
//!
//! Tables added and removed are listed by name. A table in both whose
//! history changed gets a row: the schemas it gained, the runs added, the
//! runs that took in more files, the fields added, and the columns that
//! broke.

use std::fmt::Write as _;

use nem_iceberg::History;

use crate::published::Published;

/// What changed in one table.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Changed {
    /// Schema ids the base lacks.
    pub schemas: Vec<u32>,
    /// Runs after the base's last.
    pub runs_added: usize,
    /// Runs the base has that are not as they were (they grew).
    pub runs_grown: usize,
    /// Fields the base lacks, as `id NAME`.
    pub fields: Vec<String>,
    /// Breaking columns after the base's.
    pub breaks: usize,
}

/// What changed from `old` to `new`.
pub fn changed(old: &History, new: &History) -> Changed {
    let had = old.fields();
    Changed {
        schemas: new
            .schemas
            .iter()
            .map(|s| s.schema_id)
            .filter(|id| old.schema(*id).is_none())
            .collect(),
        runs_added: new.runs.len().saturating_sub(old.runs.len()),
        runs_grown: old
            .runs
            .iter()
            .zip(&new.runs)
            .filter(|(a, b)| a != b)
            .count(),
        fields: new
            .fields()
            .iter()
            .filter(|f| !had.iter().any(|g| g.id == f.id))
            .map(|f| format!("{} {}", f.id, f.name))
            .collect(),
        breaks: new.breaking.len().saturating_sub(old.breaking.len()),
    }
}

fn names<'a>(names: impl Iterator<Item = &'a String>) -> String {
    names
        .map(|n| format!("`{n}`"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// The Markdown summary of `now` against `base`, published at `rev`.
pub fn diff(rev: &str, base: &Published, now: &Published) -> String {
    let mut out = format!("## nem-schemas against `{rev}`\n\n");
    let added: Vec<&String> = now
        .tables
        .keys()
        .filter(|t| !base.tables.contains_key(*t))
        .collect();
    let removed: Vec<&String> = base
        .tables
        .keys()
        .filter(|t| !now.tables.contains_key(*t))
        .collect();
    let rows: Vec<(&String, Changed)> = base
        .tables
        .iter()
        .filter_map(|(t, old)| {
            let new = now.tables.get(t).filter(|new| *new != old)?;
            Some((t, changed(old, new)))
        })
        .collect();
    if added.is_empty() && removed.is_empty() && rows.is_empty() {
        out.push_str("No table changed.\n");
        return out;
    }
    if !added.is_empty() {
        let _ = writeln!(
            out,
            "- Tables added ({}): {}",
            added.len(),
            names(added.into_iter())
        );
    }
    if !removed.is_empty() {
        let _ = writeln!(
            out,
            "- Tables removed ({}): {}",
            removed.len(),
            names(removed.into_iter())
        );
    }
    if !rows.is_empty() {
        out.push_str(
            "\n| Table | Schemas added | Runs added | Runs grown | Fields added | Breaks added |\n\
             |---|---|---|---|---|---|\n",
        );
        for (t, c) in rows {
            let schemas: Vec<String> = c.schemas.iter().map(u32::to_string).collect();
            let _ = writeln!(
                out,
                "| `{t}` | {} | {} | {} | {} | {} |",
                list(&schemas),
                c.runs_added,
                c.runs_grown,
                list(&c.fields),
                c.breaks
            );
        }
    }
    out
}

fn list(items: &[String]) -> String {
    if items.is_empty() {
        "–".to_owned()
    } else {
        items.join(", ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::export::tests::{exported, grow};

    #[test]
    fn says_what_each_table_gained() {
        let dir = exported();
        let root = dir.path();
        let base = Published::read(root).unwrap();
        assert_eq!(
            diff("main", &base, &base),
            "## nem-schemas against `main`\n\nNo table changed.\n"
        );
        grow(root);
        let mut now = Published::read(root).unwrap();
        let mut more = now.tables["DISPATCH_UNIT_SCADA"].clone();
        more.table = "NEW".into();
        now.tables.insert("NEW".into(), more);
        let mut old = base.clone();
        let mut gone = old.tables["DISPATCH_UNIT_SCADA"].clone();
        gone.table = "GONE".into();
        old.tables.insert("GONE".into(), gone);
        assert_eq!(
            diff("main", &old, &now),
            "## nem-schemas against `main`\n\n\
             - Tables added (1): `NEW`\n\
             - Tables removed (1): `GONE`\n\n\
             | Table | Schemas added | Runs added | Runs grown | Fields added | Breaks added |\n\
             |---|---|---|---|---|---|\n\
             | `DISPATCH_UNIT_SCADA` | 2 | 1 | 1 | 4 SCADAVALUE | 1 |\n"
        );
        // Nothing new but files at the end of a run.
        let mut grown = base.clone();
        grown.tables.get_mut("DISPATCH_UNIT_SCADA").unwrap().runs[1].files += 1;
        assert!(diff("main", &base, &grown).ends_with("| 0 | 1 | – | 0 |\n"));
        assert!(diff("main", &Published::default(), &base).contains("Tables added (1)"));
    }
}
