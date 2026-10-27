//! Every history this repository holds survives being published as
//! Iceberg table metadata: what the fold wrote comes back out of the
//! metadata, field for field, run for run, and the snapshots' times rise
//! strictly in release order.

use std::collections::BTreeMap;
use std::path::Path;

use phasor_iceberg::Market;
use phasor_iceberg::{Published, from_metadata, to_metadata};
use phasor_schemas::export::read_histories;

#[test]
fn every_history_is_its_metadata_and_back() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let histories = read_histories(&root, Market::Nem).unwrap();
    assert!(
        !histories.is_empty(),
        "no histories under {}",
        root.display()
    );
    for (table, h) in &histories {
        assert!(
            table.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'),
            "{table}: a table is named in letters, digits and underscores"
        );
        let p = Published {
            market: Market::Nem,
            history: h.clone(),
            declaration: None,
            description: None,
            docs: BTreeMap::new(),
            model_types: BTreeMap::new(),
            model: Vec::new(),
        };
        let t = to_metadata(&p, "file:///t", "u").unwrap_or_else(|e| panic!("{table}: {e}"));
        let back = from_metadata(&t.metadata).unwrap_or_else(|e| panic!("{table}: {e}"));
        assert_eq!(&back.history, h, "{table}");
        let times: Vec<i64> = t
            .metadata
            .snapshots
            .iter()
            .map(|s| s.timestamp_ms)
            .collect();
        assert!(times.windows(2).all(|w| w[0] < w[1]), "{table}: {times:?}");
        assert_eq!(t.metadata.snapshots.len(), h.runs.len(), "{table}");
    }
}
