//! `iceberg --out DIR`: every table's history as an Iceberg table with no
//! data, so any Iceberg reader can be asked what its schemas are.
//!
//! `DIR/{TABLE}/metadata/v1.metadata.json` is the table's first metadata
//! ([`utilake_iceberg::table::TableMetadata`]): every schema of the history,
//! the current one the history's, and no snapshot. Beside it,
//! `version-hint.text` names version 1: the digit alone, which readers
//! that do not trim it need. The table's UUID is derived from its name and
//! its times are zero, so the same history writes the same bytes.

use std::path::{Path, PathBuf};

use utilake_iceberg::History;
use utilake_iceberg::table::TableMetadata;

use crate::export::{ExportError, read_histories, text};

/// A UUID (version 8, RFC 9562) from the SHA-256 of the table's name.
pub fn table_uuid(table: &str) -> String {
    use sha2::Digest as _;
    let hash = sha2::Sha256::digest(format!("utilake-schemas/{table}").as_bytes());
    let mut b = [0u8; 16];
    b.copy_from_slice(&hash[..16]);
    // The version in the high nibble of byte 6, the variant `10` in the
    // high bits of byte 8.
    b[6] = (b[6] & 0x0f) | 0x80;
    b[8] = (b[8] & 0x3f) | 0x80;
    let h = crate::files::hex(&b);
    format!(
        "{}-{}-{}-{}-{}",
        &h[0..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..32]
    )
}

/// The table's metadata at `location`.
pub fn metadata(h: &History, location: &str) -> TableMetadata {
    let first = h.schemas.first().cloned().unwrap_or_default();
    let mut m = TableMetadata::new(&table_uuid(&h.table), location, &first, 0);
    m.schemas.clone_from(&h.schemas);
    m.last_column_id = h
        .schemas
        .iter()
        .flat_map(|s| s.fields.iter().map(|f| f.id))
        .max()
        .unwrap_or(0);
    m.current_schema_id = h.current_schema_id;
    m
}

/// Writes every history under `root` as a table under `out`; returns the
/// tables written.
pub fn write_tables(root: &Path, out: &Path) -> Result<usize, ExportError> {
    let histories = read_histories(root)?;
    for (table, h) in &histories {
        let dir = out.join(table);
        let m = metadata(h, &dir.display().to_string());
        let meta = dir.join("metadata");
        let files: [(PathBuf, String); 2] = [
            (
                meta.join("v1.metadata.json"),
                text(&serde_json::to_value(&m).expect("metadata serialises")),
            ),
            (meta.join("version-hint.text"), "1".to_owned()),
        ];
        for (path, body) in files {
            crate::files::write_atomic(&path, body.as_bytes())
                .map_err(|e| ExportError::io(&path, e))?;
        }
    }
    Ok(histories.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::export::tests::{exported, sample};

    #[test]
    fn one_data_free_table_per_history() {
        let dir = exported();
        let out = dir.path().join("out");
        assert_eq!(write_tables(dir.path(), &out).unwrap(), 1);
        let meta = out.join("DISPATCH_UNIT_SCADA/metadata");
        assert_eq!(
            std::fs::read_to_string(meta.join("version-hint.text")).unwrap(),
            "1"
        );
        let text = std::fs::read_to_string(meta.join("v1.metadata.json")).unwrap();
        let m: TableMetadata = serde_json::from_str(&text).unwrap();
        assert_eq!(m.schemas.len(), 2);
        assert_eq!((m.current_schema_id, m.last_column_id), (1, 3));
        assert!(m.snapshots.is_empty() && m.current_snapshot_id.is_none());
        assert_eq!(
            m.location,
            out.join("DISPATCH_UNIT_SCADA").display().to_string()
        );
        assert_eq!(m.table_uuid, table_uuid("DISPATCH_UNIT_SCADA"));
        // The same history, the same bytes.
        write_tables(dir.path(), &out).unwrap();
        assert_eq!(
            std::fs::read_to_string(meta.join("v1.metadata.json")).unwrap(),
            text
        );
        // Nowhere to write.
        std::fs::write(dir.path().join("file"), "x").unwrap();
        let e = write_tables(dir.path(), &dir.path().join("file")).unwrap_err();
        assert!(e.to_string().contains("v1.metadata.json"), "{e}");
        std::fs::write(dir.path().join("evidence/histories/X.json"), "[").unwrap();
        assert!(write_tables(dir.path(), &out).is_err());
    }

    #[test]
    fn uuids_are_version_8_and_of_the_name() {
        let u = table_uuid("A");
        assert_eq!(u.len(), 36);
        assert_eq!(&u[14..15], "8");
        assert!("89ab".contains(&u[19..20]), "{u}");
        assert_ne!(u, table_uuid("B"));
        let mut h = sample();
        h.current_schema_id = 0;
        assert_eq!(metadata(&h, "x").current_schema_id, 0);
        h.schemas.clear();
        assert_eq!(metadata(&h, "x").last_column_id, 0);
    }
}
