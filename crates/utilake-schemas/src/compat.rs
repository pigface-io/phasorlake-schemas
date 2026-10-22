//! `compat --base REV`: holds what is under `nem/` now to what `REV`
//! published.
//!
//! Every table published at the base must still be published, its history
//! kept to what it promised ([`utilake_iceberg::compat::check`]), and every
//! schema document published at the base must be there with the same
//! bytes: a document's `$id` is a promise that it never changes.
//!
//! A data version is held only to the same version: a working tree
//! re-based to a later one ([`crate::fold::Baseline`]) holds nothing of
//! the base, and says so.

use crate::published::Published;

/// What holding the working tree to the base found.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Compat {
    /// One sentence per broken promise, by table, then schema document.
    pub violations: Vec<String>,
    /// Tables the base published.
    pub tables: usize,
    /// Schema documents the base published.
    pub schemas: usize,
    /// The data versions, when the working tree's is not the base's:
    /// nothing was held.
    pub rebased: Option<(u32, u32)>,
}

/// Every promise `now` breaks of `base`, published at `rev`.
pub fn compat(rev: &str, base: &Published, now: &Published) -> Compat {
    let mut violations = Vec::new();
    let (was, is) = (base.baseline.version, now.baseline.version);
    if was != is {
        return Compat {
            violations,
            tables: base.tables.len(),
            schemas: base.schemas.len(),
            rebased: Some((was, is)),
        };
    }
    for (table, old) in &base.tables {
        match now.tables.get(table) {
            None => violations.push(format!(
                "nem/{table}/table.json: published at {rev} and missing now"
            )),
            Some(new) => violations.extend(
                utilake_iceberg::compat::check(old, new)
                    .iter()
                    .map(|v| format!("nem/{table}/table.json: {v}")),
            ),
        }
    }
    for (path, bytes) in &base.schemas {
        match now.schemas.get(path) {
            None => violations.push(format!("{path}: published at {rev} and missing now")),
            Some(b) if b != bytes => violations.push(format!("{path}: differs from {rev}")),
            Some(_) => {}
        }
    }
    Compat {
        violations,
        tables: base.tables.len(),
        schemas: base.schemas.len(),
        rebased: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::export::tests::{exported, grow};

    #[test]
    fn a_grown_export_holds_and_a_changed_one_does_not() {
        let dir = exported();
        let root = dir.path();
        let base = Published::read(root).unwrap();
        let held = compat("main", &base, &base);
        assert_eq!(
            (
                held.violations.len(),
                held.tables,
                held.schemas,
                held.rebased
            ),
            (0, 1, 2, None)
        );
        assert!(
            compat("main", &Published::default(), &base)
                .violations
                .is_empty()
        );

        // More runs, another schema: still holds.
        grow(root);
        let grown = Published::read(root).unwrap();
        assert!(compat("main", &base, &grown).violations.is_empty());

        // A run rewritten, a schema document edited, one gone, a table gone.
        let mut broken = grown.clone();
        broken.tables.get_mut("DISPATCH_UNIT_SCADA").unwrap().runs[0].schema_id = 1;
        let mut docs = broken.schemas.keys().cloned();
        let (first, second) = (docs.next().unwrap(), docs.next().unwrap());
        broken.schemas.insert(first.clone(), b"{}".to_vec());
        broken.schemas.remove(&second);
        let v = compat("main", &base, &broken).violations;
        assert_eq!(
            v,
            [
                "nem/DISPATCH_UNIT_SCADA/table.json: run 0 has changed".to_owned(),
                format!("{first}: differs from main"),
                format!("{second}: published at main and missing now"),
            ]
        );
        broken.tables.clear();
        assert_eq!(
            compat("main", &base, &broken).violations[0],
            "nem/DISPATCH_UNIT_SCADA/table.json: published at main and missing now"
        );
        // Re-based: nothing is held, and the versions are named.
        broken.baseline.version = 2;
        let c = compat("main", &base, &broken);
        assert!(c.violations.is_empty());
        assert_eq!((c.rebased, c.tables), (Some((1, 2)), 1));
    }
}
