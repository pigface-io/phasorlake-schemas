//! The two columns every lake file adds to its table's: where each row
//! came from and when that became public. They make a row self-describing
//! through any copy, concatenation or compaction, so "latest", "known at"
//! and point-in-time joins are plain SQL over the files.
//!
//! The names fall under the `_nl_` prefix no table may declare, and the
//! field ids sit far above any a census gives out and below the ones
//! Iceberg reserves for itself (2147483447 and up).

use crate::schema::{Field, Schema};
use crate::types::IcebergType;

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
    name: "_nl_source",
    id: 2_147_483_000,
    ty: IcebergType::String,
    required: true,
    doc: "The file the row was read from: its URL, and `!{entry}` for a file inside a zip.",
};

/// When the row's source became public, as its URL or the server says.
pub const PUBLISHED: Column = Column {
    name: "_nl_published",
    id: 2_147_483_001,
    ty: IcebergType::Timestamptz,
    required: false,
    doc: "When the source became public; null when nothing says.",
};

/// Both, in the order a lake file holds them, after the table's columns.
pub const COLUMNS: [Column; 2] = [SOURCE, PUBLISHED];

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
    s.last_column_id = s.last_column_id.max(PUBLISHED.id);
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
        assert_eq!(names, ["DUID", "_nl_source", "_nl_published"]);
        assert_eq!(lake.last_column_id, PUBLISHED.id);
        assert_eq!(lake.identifier_field_ids, [1]);
        // Twice is once.
        assert_eq!(lake_schema(&lake), lake);
        assert!(is_provenance("_nl_source"));
        assert!(!is_provenance("DUID"));
        // Clear of the ids Iceberg keeps for itself.
        assert!(COLUMNS.iter().all(|c| c.id < 2_147_483_447));
    }
}
