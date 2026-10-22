//! One JSON Schema per Iceberg schema of a table, derived from the
//! table's metadata.
//!
//! A schema's document is named by its id, `nem/{TABLE}/schema-{id}.json`:
//! a schema id never changes meaning within a data version. A row read
//! under the schema validates against the document; each property carries
//! its field's id and Iceberg type, and, when the data model documents the
//! column, its words (`description`) and its type there (`x-model-type`);
//! `x-fields` gives each field's id, name and Iceberg type in order.
//! Nothing in it says which files were written under the schema.

use serde_json::{Map, Value, json};
use utilake_iceberg::publish::Published;
use utilake_iceberg::{IcebergType, Schema};

use super::{FORMAT, METADATA, text};

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

/// The file name of schema `id`'s document.
pub fn file_name(id: u32) -> String {
    format!("schema-{id}.json")
}

/// The document of `schema` of `table`, published at `{base}{path}`,
/// with what `published` documents of its columns.
pub fn document(
    base: &str,
    table: &str,
    path: &str,
    schema: &Schema,
    published: &Published,
) -> String {
    let mut properties = Map::new();
    let mut required = Vec::new();
    for f in &schema.fields {
        let mut p = value_schema(f.ty);
        if f.required {
            required.push(json!(f.name));
        } else {
            p["type"] = json!([p["type"].take(), "null"]);
        }
        if let Some(d) = published.docs.get(&f.id) {
            p["description"] = json!(d);
        }
        p["x-field-id"] = json!(f.id);
        p["x-iceberg-type"] = json!(f.ty.name());
        if let Some(t) = published.model_types.get(&f.id) {
            p["x-model-type"] = json!(t);
        }
        if let Some(h) = published.history.headers.get(&f.id) {
            p["x-header"] = json!(h);
        }
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
        "x-table": {"name": table, "$ref": METADATA},
        "x-schema-id": schema.schema_id,
        "x-identifier-field-ids": schema.identifier_field_ids,
        "x-fields": schema
            .fields
            .iter()
            .map(|f| json!([f.id, f.name, f.ty.name()]))
            .collect::<Vec<_>>(),
    }))
}
