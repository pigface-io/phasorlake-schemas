//! `iceberg --out DIR`: every table as the export publishes it, at file
//! locations, so any Iceberg reader can be pointed at it.
//!
//! `DIR/{TABLE}/metadata/v1.metadata.json` is the same table metadata the
//! export writes under `nem/` ([`utilake_iceberg::publish`]), with `DIR`
//! as its location instead of the published URL, and the same manifest
//! lists beside it. `version-hint.text` names version 1: the digit alone,
//! which readers that do not trim it need. The table's UUID is derived
//! from its name, so the same history writes the same bytes.

use std::path::Path;

use crate::export::{DEFAULT_BASE_URL, Export, ExportError, text};

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

/// Writes every table under `root` as a table under `out`; returns the
/// tables written.
pub fn write_tables(root: &Path, out: &Path) -> Result<usize, ExportError> {
    let export = Export::load(root, DEFAULT_BASE_URL)?;
    let mut n = 0;
    for table in export.tables() {
        let dir = out.join(table);
        let t = export
            .table(table, &dir.display().to_string())
            .expect("the table has a history")?;
        let meta = dir.join("metadata");
        let mut files = vec![
            (
                meta.join("v1.metadata.json"),
                text(&serde_json::to_value(&t.metadata).expect("metadata serialises")).into_bytes(),
            ),
            (meta.join("version-hint.text"), b"1".to_vec()),
        ];
        files.extend(
            t.manifest_lists
                .into_iter()
                .map(|ml| (dir.join(ml.path), ml.bytes)),
        );
        for (path, body) in files {
            crate::files::write_atomic(&path, &body).map_err(|e| ExportError::io(&path, e))?;
        }
        n += 1;
    }
    Ok(n)
}

#[cfg(test)]
mod tests {
    use utilake_iceberg::table::TableMetadata;

    use super::*;
    use crate::export::tests::exported;

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
        assert_eq!(m.snapshots.len(), 2);
        assert_eq!(
            m.location,
            out.join("DISPATCH_UNIT_SCADA").display().to_string()
        );
        assert_eq!(m.table_uuid, table_uuid("DISPATCH_UNIT_SCADA"));
        // Each snapshot's manifest list is where the metadata says.
        for s in &m.snapshots {
            let path = Path::new(&s.manifest_list);
            assert!(path.starts_with(&meta), "{}", s.manifest_list);
            let bytes = std::fs::read(path).unwrap();
            assert_eq!(
                utilake_iceberg::table::read_manifest_list(&bytes).unwrap(),
                []
            );
        }
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
    }
}
