//! Iceberg tables written by hand: table metadata as JSON, manifests and
//! manifest lists as Avro, every byte a function of the caller's input.
//!
//! Format version 3, unpartitioned, one writer. Nothing here does I/O:
//! whoever writes the table puts these bytes on disk.

pub mod avro;
pub mod manifest;
pub mod metadata;

pub use manifest::{
    DataFile, Entry, ManifestFile, read_manifest, read_manifest_list, write_manifest,
    write_manifest_list,
};
pub use metadata::{MetadataLog, Ref, Snapshot, SnapshotLog, SortOrder, Spec, TableMetadata};
