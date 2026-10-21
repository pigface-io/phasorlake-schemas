//! One JSON Schema per Iceberg schema of a table.
//!
//! A schema's document is named by a hash of its fields and key, so its
//! name never changes as the history grows: `nem/{TABLE}/schema-{hash}.json`,
//! the hash the first 16 hex digits of the SHA-256 of
//! `{"fields": …, "identifier-field-ids": …}` as compact JSON with its keys
//! sorted, the name utilake gives the same schema. A row read under the
//! schema validates against the document; `x-fields` gives each field's id,
//! name and Iceberg type. Nothing in it says which files were written under
//! the schema, so a published document never changes.

use serde_json::{Map, Value, json};
use utilake_iceberg::{IcebergType, Schema};

use super::{FORMAT, text};

/// The JSON Schema of a value of an Iceberg type, as a row holds it.
fn value_schema(ty: IcebergType) -> Value {
    match ty {
        IcebergType::Boolean => json!({"type": "boolean"}),
        IcebergType::Long => json!({"type": "integer"}),
        IcebergType::Double | IcebergType::Decimal { .. } => json!({"type": "number"}),
        IcebergType::Date => json!({"type": "string", "format": "date"}),
        IcebergType::Timestamptz => json!({"type": "string", "format": "date-time"}),
        IcebergType::String => json!({"type": "string"}),
    }
}

/// The file name of a schema's document: `schema-{hash16}.json`.
pub fn file_name(schema: &Schema) -> String {
    let v = schema.to_json();
    let identity = json!({
        "fields": v["fields"],
        "identifier-field-ids": v["identifier-field-ids"],
    });
    let hash = crate::ctl::sha256(identity.to_string().as_bytes());
    format!("schema-{}.json", &hash[..16])
}

/// The document of `schema` of `table`, published at `{base}{path}`.
pub fn document(base: &str, table: &str, path: &str, schema: &Schema) -> String {
    let mut properties = Map::new();
    let mut required = Vec::new();
    for f in &schema.fields {
        let mut p = value_schema(f.ty);
        if f.required {
            required.push(json!(f.name));
        } else {
            p["type"] = json!([p["type"].take(), "null"]);
        }
        p["x-field-id"] = json!(f.id);
        p["x-iceberg-type"] = json!(f.ty.name());
        properties.insert(f.name.clone(), p);
    }
    text(&json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "$id": format!("{base}{path}"),
        "title": format!("{table}, Iceberg schema {}", schema.schema_id),
        "type": "object",
        "additionalProperties": false,
        "required": required,
        "properties": properties,
        "x-format": FORMAT,
        "x-table": {"name": table, "$ref": "table.json"},
        "x-schema-id": schema.schema_id,
        "x-identifier-field-ids": schema.identifier_field_ids,
        "x-fields": schema
            .fields
            .iter()
            .map(|f| json!([f.id, f.name, f.ty.name()]))
            .collect::<Vec<_>>(),
    }))
}
