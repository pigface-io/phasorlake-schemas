//! The primitives every decoder of AEMO's files shares, whatever the
//! format.
//!
//! The MMS decoder and the plain decoder read different files into the
//! same shape: blocks of rows whose fields are text. What that shape is
//! made of lives here, so neither decoder depends on the other.
//!
//! - [`fields`]: a line split into a [`Row`] of fields, quote-aware and
//!   without allocating per row.
//! - [`flow`]: where a row sits in its file ([`Pos`]), what a reader does
//!   after a bad row ([`Flow`]), and [`Never`], the error of what cannot
//!   fail.
//! - [`month`]: the calendar month AEMO cuts its archives in.
//! - [`names`]: a report or column name normalised.
//! - [`fingerprint`]: the identity of an ordered column list.
//! - [`diagnose`]: the diagnostic of each error here, with its stable code.
//!
//! Pure: no I/O, no format, no registry, no lake.

pub mod diagnose;
pub mod fields;
pub mod fingerprint;
pub mod flow;
pub mod month;
pub mod names;

pub use fields::{Row, Splitter};
pub use fingerprint::{Fingerprint, FingerprintError};
pub use flow::{Flow, Never, Pos};
pub use month::{YearMonth, YearMonthError};
pub use names::normalize_name;
