//! Splitting one line into fields: the splitter of [`phasor_decode`],
//! and the prefix every MMS row starts with.

pub use phasor_decode::fields::{Row, Span, Splitter};

/// Fields AEMO always writes before the data: `D`, report type, subtype,
/// version.
pub const PREFIX_FIELDS: usize = 4;
