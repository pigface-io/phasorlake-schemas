//! The zones of a scan, for `zonetrace` (feature `zones`). The numbers
//! are fixed, so a report from one revision diffs against the next; the
//! scanner's are 400 to 499.

use zonetrace::Zone;

/// One stream scanned to its end, the sink's work included.
pub static SCAN: Zone = Zone::new(400, "scan");
/// One read from the stream: for a zip entry, inflating it.
pub static FILL: Zone = Zone::new(401, "scan.fill");
/// A D row split into its fields.
pub static SPLIT: Zone = Zone::new(402, "scan.split");
/// An I row read into the block's shape, the sink's work included.
pub static HEADER: Zone = Zone::new(403, "scan.header");
/// The partial last line moved to the start of the buffer.
pub static TAIL: Zone = Zone::new(404, "scan.tail");
