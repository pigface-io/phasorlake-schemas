//! AEMO's files that are not MMS rows, read as blocks of rows.
//!
//! The gas markets' reports (GBB, STTM, DWGM) are plain CSV files with one
//! header row; the Western Australian market's datafiles are the same; its
//! WEMDE market data is JSON. This crate reads them all into the one shape
//! the MMS scanner produces: a block header of normalised column names,
//! then rows whose fields are text, with every date written the way MMS
//! files write it (`YYYY/MM/DD` or `YYYY/MM/DD HH:MM:SS`). What reads MMS
//! rows then reads these rows too, unchanged.
//!
//! - [`dates`]: every date form AEMO writes, rewritten into the MMS form.
//! - [`csv`]: a header-row CSV as one block of rows.
//! - [`json`]: a WEMDE document as one block per array.
//! - [`names`]: the block a file is (family, name, version), the column a
//!   header is, the stamps and months a file name carries.
//! - [`feeds`]: every feed that is not MMSDM: its host, roots, decoder and
//!   the rules its files follow.
//! - [`diagnose`]: the diagnostic of each error here, with its stable
//!   code.
//! - [`diff`]: the rows a version of a file adds to its previous version.
//! - [`mod@sniff`]: what a file's first bytes say it is, when a folder holds
//!   the odd file of another kind.
//!
//! Pure: no I/O beyond the reader handed in, no registry, no lake.

pub mod csv;
pub mod dates;
pub mod diagnose;
pub mod diff;
pub mod feeds;
pub mod json;
pub mod names;
pub mod sniff;

pub use csv::{Header, PlainError, PlainOptions, PlainRowError, PlainSink, PlainStats, Rows};
pub use diff::RowSet;
pub use feeds::{Decoder, Dedupe, FEEDS, Feed, Release};
pub use json::{JsonError, JsonStats};
pub use names::{BlockName, column_name};
pub use sniff::{Format, peek, sniff};
pub use utilake_decode::{Flow, Pos, Row};
