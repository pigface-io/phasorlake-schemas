//! The zones of a plain read, for `zonetrace` (feature `zones`). The
//! numbers are fixed, 1200–1299 for this crate, so a report from one
//! revision diffs against the next. The tracer caches a zone by `id % 64`,
//! and 1240–1245 fall in slots 24–29, which the readers' hot zones leave
//! free.

use zonetrace::Zone;

/// One header-row CSV scanned into rows.
pub static PLAIN_SCAN: Zone = Zone::new(1240, "plain.scan");
/// One WEMDE document scanned into blocks.
pub static WEMDE_SCAN: Zone = Zone::new(1243, "wemde.scan");
/// The document parsed as JSON.
pub static WEMDE_PARSE: Zone = Zone::new(1244, "wemde.parse");
/// The first bytes of a file read ahead to tell its kind.
pub static SNIFF_PEEK: Zone = Zone::new(1245, "sniff.peek");
