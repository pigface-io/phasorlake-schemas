//! AEMO's NEM tables as Apache Iceberg tables (format version 3).
//!
//! Every table is an Iceberg schema history: fields with ids that never
//! change meaning, types from the set in [`IcebergType`], identifier fields
//! for the key. A column's type is chosen once from everything observed of
//! it and AEMO's declaration ([`choose`]); [`holds`] says whether a type
//! takes what a later file holds. A table's [`History`] is what the
//! export publishes; [`compat::check`] holds a new version of it to what the
//! old one promised. [`plan`] reads any time interval of a table's files
//! across its schemas, and [`table`] writes the table's metadata and
//! manifests.
//!
//! Std, serde and SHA-256 only; no I/O.

pub mod compat;
mod evidence;
pub mod history;
mod plan;
mod schema;
pub mod table;
mod types;

pub use evidence::{Choice, Declared, Evidence, Kind, choose, holds};
pub use history::History;
pub use plan::{Column, DataFile, PlanError, Read, Target, casts_to, duckdb_sql, plan};
pub use schema::{Field, Schema};
pub use types::IcebergType;
