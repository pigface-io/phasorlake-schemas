//! AEMO's tables as Apache Iceberg tables (format version 3).
//!
//! Every table is an Iceberg schema history: fields with ids that never
//! change meaning and types from the set in [`IcebergType`]. A table's key
//! and temporal roles are a reader's decisions and are not published here.
//! A column's type is chosen once from everything observed of
//! it and AEMO's declaration ([`choose`]); [`holds`] says whether a type
//! takes what a later file holds. A table's [`History`] is what the
//! export publishes; [`compat::check`] holds a new version of it to what the
//! old one promised. [`publish`] writes a history, with the table's
//! declaration and documentation, as Iceberg table metadata and reads it
//! back: the one document a published table is. [`plan`] reads any time interval of a
//! table's files across its schemas, and [`table`] writes table metadata
//! and manifests as bytes. [`provenance`] names the two columns every lake
//! file adds: each row's source and when it became public. Every error
//! here describes itself as a diagnostic with a stable code ([`diagnose`]).
//!
//! Std, serde and SHA-256 only; no I/O.

pub mod compat;
pub mod diagnose;
mod evidence;
pub mod history;
pub mod market;
mod plan;
pub mod provenance;
pub mod publish;
mod schema;
pub mod table;
mod types;

pub use evidence::{Choice, Declared, Evidence, Kind, choose, holds};
pub use history::History;
pub use market::Market;
pub use plan::{Column, DataFile, PlanError, Read, Target, casts_to, duckdb_sql, plan};
pub use publish::{
    Declaration, PublishError, Published, breaks_of_properties, declaration_of_properties,
    from_metadata, to_metadata,
};
pub use schema::{Field, Schema};
pub use types::IcebergType;
