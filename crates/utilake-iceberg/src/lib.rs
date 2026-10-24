//! AEMO's tables as Apache Iceberg tables (format version 3).
//!
//! Every table is an Iceberg schema history: fields with ids that never
//! change meaning, types from the set in [`IcebergType`], identifier fields
//! for the key. A column's type is chosen once from everything observed of
//! it and AEMO's declaration ([`choose`]); [`holds`] says whether a type
//! takes what a later file holds. A table's [`History`] is what the
//! export publishes; [`compat::check`] holds a new version of it to what the
//! old one promised. [`publish`] writes a history, with the table's
//! declaration and documentation, as Iceberg table metadata and reads it
//! back: the one document a published table is. [`derive()`] gives a
//! table's temporal roles from its key and columns, the one rule every
//! reader and the publisher share. [`plan`] reads any time interval of a
//! table's files across its schemas, and [`table`] writes table metadata
//! and manifests as bytes.
//!
//! Std, serde and SHA-256 only; no I/O.

pub mod compat;
mod evidence;
pub mod history;
pub mod market;
mod plan;
pub mod publish;
mod schema;
pub mod table;
pub mod temporal;
mod types;

pub use evidence::{Choice, Declared, Evidence, Kind, choose, holds};
pub use history::History;
pub use market::Market;
pub use plan::{Column, DataFile, PlanError, Read, Target, casts_to, duckdb_sql, plan};
pub use publish::{
    Declaration, PublishError, Published, Temporal, Valid, from_metadata, to_metadata,
};
pub use schema::{Field, Schema};
pub use temporal::derive;
pub use types::IcebergType;
