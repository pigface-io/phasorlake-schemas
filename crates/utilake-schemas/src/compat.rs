//! `compat --base REV`: holds what is under the markets' directories now to
//! what `REV` published.
//!
//! Every table published at the base must still be published, its history
//! kept to what it promised ([`utilake_iceberg::compat::check`]), and every
//! schema document published at the base must be there saying the same of
//! its fields: a document's `$id` is a promise that what a row validates
//! against never changes. The words the data model gives a column
//! (`description`, `x-model-type`) may change with a release.
//!
//! A data version is held only to the same version: a market re-based to
//! a later one ([`crate::fold::Baseline`]) holds nothing of the base's
//! tables of that market, and says so. A base from before the metadata carried the
//! history named its schema documents by hash; its documents are not
//! held, its histories are.

use std::collections::BTreeMap;

use utilake_iceberg::Market;

use crate::export::METADATA;
use crate::published::Published;

/// A schema document without the words a data-model release may change.
fn identity(bytes: &[u8]) -> Vec<u8> {
    let Ok(mut doc) = serde_json::from_slice::<serde_json::Value>(bytes) else {
        return bytes.to_vec();
    };
    if let Some(props) = doc.get_mut("properties").and_then(|p| p.as_object_mut()) {
        for p in props.values_mut().filter_map(|p| p.as_object_mut()) {
            p.remove("description");
            p.remove("x-model-type");
        }
    }
    doc.to_string().into_bytes()
}

/// What holding the working tree to the base found.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Compat {
    /// One sentence per broken promise, by table, then schema document.
    pub violations: Vec<String>,
    /// Tables the base published.
    pub tables: usize,
    /// Schema documents the base published.
    pub schemas: usize,
    /// The markets whose data version is not the base's, with both
    /// versions: nothing of theirs was held.
    pub rebased: BTreeMap<Market, (u32, u32)>,
}

/// Every promise `now` breaks of `base`, published at `rev`.
pub fn compat(rev: &str, base: &Published, now: &Published) -> Compat {
    let mut violations = Vec::new();
    let rebased: BTreeMap<Market, (u32, u32)> = base
        .markets()
        .filter_map(|m| {
            let (was, is) = (base.baseline(m).version, now.baseline(m).version);
            (was != is).then_some((m, (was, is)))
        })
        .collect();
    let held = |key: &str| !rebased.keys().any(|m| key.starts_with(&format!("{m}/")));
    for (table, old) in base.tables.iter().filter(|(k, _)| held(k)) {
        match now.tables.get(table) {
            None => violations.push(format!(
                "{table}/{METADATA}: published at {rev} and missing now"
            )),
            Some(new) => violations.extend(
                utilake_iceberg::compat::check(old, new)
                    .iter()
                    .map(|v| format!("{table}/{METADATA}: {v}")),
            ),
        }
    }
    for (path, bytes) in base.schemas.iter().filter(|(p, _)| !base.legacy && held(p)) {
        match now.schemas.get(path) {
            None => violations.push(format!("{path}: published at {rev} and missing now")),
            Some(b) if identity(b) != identity(bytes) => {
                violations.push(format!("{path}: differs from {rev}"));
            }
            Some(_) => {}
        }
    }
    Compat {
        violations,
        tables: base.tables.len(),
        schemas: base.schemas.len(),
        rebased,
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
            (0, 1, 2, BTreeMap::new())
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
        broken
            .tables
            .get_mut("nem/DISPATCH_UNIT_SCADA")
            .unwrap()
            .runs[0]
            .schema_id = 1;
        let mut docs = broken.schemas.keys().cloned();
        let (first, second) = (docs.next().unwrap(), docs.next().unwrap());
        broken.schemas.insert(first.clone(), b"{}".to_vec());
        broken.schemas.remove(&second);
        let v = compat("main", &base, &broken).violations;
        assert_eq!(
            v,
            [
                "nem/DISPATCH_UNIT_SCADA/metadata/v1.metadata.json: run 0 has changed".to_owned(),
                format!("{first}: differs from main"),
                format!("{second}: published at main and missing now"),
            ]
        );
        broken.tables.clear();
        assert_eq!(
            compat("main", &base, &broken).violations[0],
            "nem/DISPATCH_UNIT_SCADA/metadata/v1.metadata.json: published at main and missing now"
        );
        // The data model's words may change; the fields may not. A base
        // whose documents were named by hash holds none of them.
        let mut reworded = grown.clone();
        let doc = reworded.schemas.get_mut(&first).unwrap();
        let mut d: serde_json::Value = serde_json::from_slice(doc).unwrap();
        d["properties"]["SETTLEMENTDATE"]["description"] = serde_json::json!("New words");
        d["properties"]["DUID"]["x-model-type"] = serde_json::json!("VARCHAR(30)");
        *doc = d.to_string().into_bytes();
        assert!(compat("main", &base, &reworded).violations.is_empty());
        d["properties"]["DUID"]["type"] = serde_json::json!("integer");
        reworded
            .schemas
            .insert(first.clone(), d.to_string().into_bytes());
        assert_eq!(
            compat("main", &base, &reworded).violations,
            [format!("{first}: differs from main")]
        );
        reworded.schemas.insert(first.clone(), b"not JSON".to_vec());
        assert_eq!(
            compat("main", &base, &reworded).violations,
            [format!("{first}: differs from main")]
        );
        let mut legacy = base.clone();
        legacy.legacy = true;
        legacy.schemas.insert(
            "nem/DISPATCH_UNIT_SCADA/schema-2e75294ff617bace.json".into(),
            b"{}".to_vec(),
        );
        let c = compat("main", &legacy, &grown);
        assert!(c.violations.is_empty());
        assert_eq!(c.schemas, 3);
        // Re-based: nothing is held, and the versions are named.
        broken.baselines.insert(
            Market::Nem,
            crate::fold::Baseline {
                version: 2,
                reason: "again".into(),
            },
        );
        let c = compat("main", &base, &broken);
        assert!(c.violations.is_empty());
        assert_eq!(
            (c.rebased, c.tables),
            (BTreeMap::from([(Market::Nem, (1, 2))]), 1)
        );
    }
}
