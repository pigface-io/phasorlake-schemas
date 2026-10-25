//! The MMS C/I/D row format, as AEMO publishes the National Electricity
//! Market's data.
//!
//! ```text
//! C,SETP.WORLD,DVD_DISPATCH_UNIT_SCADA,AEMO,PUBLIC,2024/02/07,00:00:07,...   file header
//! I,DISPATCH,UNIT_SCADA,1,SETTLEMENTDATE,DUID,SCADAVALUE                    block header
//! D,DISPATCH,UNIT_SCADA,1,"2024/01/01 00:05:00",BUTLERSG,9.199999            data row
//! C,"END OF REPORT",4193482                                                  footer: total lines
//! ```
//!
//! Synchronous and allocation-free per row. [`scan()`] splits bytes into
//! rows and drives a [`Sink`]: each I row reaches it as the block's shape
//! (report pair, version, ordered column names), each D row as its fields,
//! untyped. What a sink refuses or fails with is the sink's own type
//! ([`Sink::Reject`], [`Sink::Fatal`]): this crate knows no registry, no
//! lake and no diagnostics.
//!
//! Beside the scanner: [`names`] normalises a column name and
//! [`fingerprint`] identifies a column list; [`shape`] gives a block's
//! report pair and columns a content-addressed [`shape::ShapeId`], the one
//! id every utilake repository gives it; [`filename`] reads a table and a
//! [`month::YearMonth`] out of an archive file name.

/// Opens a zone of `zonetrace` until the end of the block, with the
/// feature `zones`; nothing without it.
#[allow(unused_macros)]
macro_rules! zone {
    ($z:ident) => {
        #[cfg(feature = "zones")]
        let _zone = zonetrace::enter(&crate::zones::$z);
    };
}

#[cfg(feature = "zones")]
pub mod zones;

pub mod error;
pub mod fields;
pub mod filename;
pub mod fingerprint;
pub mod irow;
pub mod month;
pub mod names;
pub mod scan;
pub mod shape;

pub use error::{Never, Pos, RowError, RowFailure, ScanError};
pub use fields::{PREFIX_FIELDS, Row};
pub use fingerprint::Fingerprint;
pub use irow::{IRow, IRowError, IRowScratch, parse_version};
pub use month::YearMonth;
pub use names::normalize_name;
pub use scan::{Flow, ScanOptions, ScanStats, Sink, scan, scan_with};
pub use shape::{CanonicalShape, ShapeId, TypeClass};
