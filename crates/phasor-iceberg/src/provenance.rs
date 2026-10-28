//! The columns every lake file adds to its table's: where each row came
//! from and every time its source was stamped with. They make a row
//! self-describing through any copy, concatenation or compaction, so
//! "latest", "known at" and point-in-time joins are plain SQL over the
//! files.
//!
//! Each time column transcribes one fact from where it was found, and is
//! named for that place: the web server's `Last-Modified`, the zip entry's
//! modified time in its zip's central directory, the timestamp written in
//! the file's name. Nothing is combined or judged at write time; a reader
//! chooses which to trust, or takes the earliest.
//!
//! The names fall under the `_phasor_` prefix no table may declare, and the
//! field ids sit far above any a census gives out and below the ones
//! Iceberg reserves for itself (2147483447 and up).

use crate::schema::{Field, Schema};
use crate::types::IcebergType;

/// What every provenance column's name begins with, and no table's may.
pub const PREFIX: &str = "_phasor_";

/// One provenance column.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Column {
    /// Its name in a lake file.
    pub name: &'static str,
    /// Its field id.
    pub id: u32,
    /// Its type.
    pub ty: IcebergType,
    /// Whether every row has a value.
    pub required: bool,
    /// What it holds.
    pub doc: &'static str,
}

/// The row's source: the source file's URL, then `!` and the entry for a
/// row read from inside a zip (nested zips joined by `!` too).
pub const SOURCE: Column = Column {
    name: "_phasor_source",
    id: 2_147_483_000,
    ty: IcebergType::String,
    required: true,
    doc: "The file the row was read from: its URL, and `!{entry}` for a file inside a zip.",
};

/// The HTTP `Last-Modified` the web server gave for the row's source: of
/// the entry's own bytes where they were also served on their own, else
/// of the file they came in. The server's word for when the bytes were
/// public: exact for a file fetched as it came out; late, never early, for
/// one the server touched again (AEMO touched most of its archive again in
/// April 2026).
pub const SERVER_LAST_MODIFIED: Column = Column {
    name: "_phasor_server_last_modified",
    id: 2_147_483_001,
    ty: IcebergType::Timestamptz,
    required: false,
    doc: "The HTTP Last-Modified the web server gave for the source's bytes; null when it gave none.",
};

/// The modified time of the row's source as an entry of its zip, from the
/// zip's central directory (the DOS date and time, to two seconds), read
/// in the clock of the machine that packed the zip: AEST for AEMO's. When
/// the file was written, so at or before it was public. None for a file
/// not read from a zip, or a zip that records no time.
pub const ZIP_ENTRY_MODIFIED: Column = Column {
    name: "_phasor_zip_entry_modified",
    id: 2_147_483_002,
    ty: IcebergType::Timestamptz,
    required: false,
    doc: "The entry's modified time in its zip's central directory, in the packer's clock; null for a file not read from a zip.",
};

/// The timestamp written in the source's own file name: the first run of
/// exactly fourteen digits, `YYYYMMDDHHMMSS`, read in the host's clock.
/// AEMO stamps a report with its creation time
/// (`PUBLIC_YESTBID_201612220000_20161223040519.zip` was created at
/// 2016-12-23 04:05:19) and a gas restatement with its time
/// (`int651_…~20261023113222.csv`). The twelve-digit stamp a report also
/// carries is the interval it reports, not a time it was made, and is not
/// read here. None when the name carries no fourteen-digit run.
pub const FILENAME_TIMESTAMP: Column = Column {
    name: "_phasor_filename_timestamp",
    id: 2_147_483_003,
    ty: IcebergType::Timestamptz,
    required: false,
    doc: "The fourteen-digit YYYYMMDDHHMMSS stamp in the source's file name, in the host's clock; null when it has none.",
};

/// All four, in the order a lake file holds them, after the table's columns.
pub const COLUMNS: [Column; 4] = [
    SOURCE,
    SERVER_LAST_MODIFIED,
    ZIP_ENTRY_MODIFIED,
    FILENAME_TIMESTAMP,
];

impl Column {
    /// As a schema field.
    pub fn field(&self) -> Field {
        Field {
            id: self.id,
            name: self.name.to_owned(),
            ty: self.ty,
            required: self.required,
            doc: Some(self.doc.to_owned()),
        }
    }
}

/// Whether `name` is one of the provenance columns.
pub fn is_provenance(name: &str) -> bool {
    COLUMNS.iter().any(|c| c.name == name)
}

/// A table's schema as its lake files hold it: its columns, then the
/// provenance columns. The largest id given out grows to theirs, as
/// Iceberg asks of every id a schema holds.
#[must_use]
pub fn lake_schema(table: &Schema) -> Schema {
    let mut s = table.clone();
    s.fields.retain(|f| !is_provenance(&f.name));
    s.fields.extend(COLUMNS.iter().map(Column::field));
    s.last_column_id = s.last_column_id.max(FILENAME_TIMESTAMP.id);
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_lake_schema_is_the_tables_then_the_provenance() {
        let table = Schema {
            schema_id: 3,
            fields: vec![Field {
                id: 1,
                name: "DUID".into(),
                ty: IcebergType::String,
                required: true,
                doc: None,
            }],
            identifier_field_ids: vec![1],
            last_column_id: 7,
        };
        let lake = lake_schema(&table);
        let names: Vec<&str> = lake.fields.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "DUID",
                "_phasor_source",
                "_phasor_server_last_modified",
                "_phasor_zip_entry_modified",
                "_phasor_filename_timestamp",
            ]
        );
        assert_eq!(lake.last_column_id, FILENAME_TIMESTAMP.id);
        assert_eq!(lake.identifier_field_ids, [1]);
        // Twice is once.
        assert_eq!(lake_schema(&lake), lake);
        assert!(is_provenance("_phasor_zip_entry_modified"));
        assert!(!is_provenance("DUID"));
        // Ids in order, clear of the ones Iceberg keeps for itself.
        let ids: Vec<u32> = COLUMNS.iter().map(|c| c.id).collect();
        assert_eq!(
            ids,
            [2_147_483_000, 2_147_483_001, 2_147_483_002, 2_147_483_003]
        );
        assert!(COLUMNS.iter().all(|c| c.id < 2_147_483_447));
        assert!(COLUMNS.iter().all(|c| c.name.starts_with(PREFIX)));
    }
}
