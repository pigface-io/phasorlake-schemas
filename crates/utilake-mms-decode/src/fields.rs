//! Splitting one line into fields: the splitter of [`utilake_decode`],
//! and the prefix every MMS row starts with.

pub use utilake_decode::fields::{Row, Span, Splitter};

/// Fields AEMO always writes before the data: `D`, report type, subtype,
/// version.
pub const PREFIX_FIELDS: usize = 4;
