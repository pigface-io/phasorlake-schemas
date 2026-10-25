//! The zones of a walk, for `zonetrace` (feature `zones`). The numbers
//! are fixed: a report from one revision diffs against the next.

use zonetrace::Zone;

/// The whole walk of one zip on disk.
pub static WALK: Zone = Zone::new(1, "walk");
/// The zip reader's reading of a central directory.
pub static ARCHIVE_OPEN: Zone = Zone::new(2, "archive.open");
/// Looking for entries whose local header disagrees with the directory.
pub static FIND_MISPLACED: Zone = Zone::new(3, "find_misplaced");
/// Reading one local header to compare with its central record.
pub static LOCAL_HEADER: Zone = Zone::new(4, "find_misplaced.local_header");
/// The zip reader reading the directory again, after the look.
pub static REOPEN: Zone = Zone::new(5, "find_misplaced.reopen");
/// Opening one entry for reading.
pub static ENTRY_OPEN: Zone = Zone::new(6, "entry.open");
/// The caller's callback for a wanted entry, its reads included.
pub static ON_ENTRY: Zone = Zone::new(7, "on_entry");
/// One read of a wanted entry: inflating and checking its CRC-32.
pub static ENTRY_READ: Zone = Zone::new(8, "entry.read");
/// A nested zip read whole into memory.
pub static NESTED_MEMORY: Zone = Zone::new(9, "nested.read_to_end");
/// A nested zip written to a temporary file.
pub static NESTED_SPOOL: Zone = Zone::new(10, "nested.spool");
/// Loading one chunk of a zip from its file.
pub static CHUNK_LOAD: Zone = Zone::new(11, "chunk.load");
/// Copying out of a loaded chunk to the reader's buffer.
pub static CHUNK_COPY: Zone = Zone::new(12, "chunk.copy_out");
/// One read of an entry whose local header disagrees.
pub static VERIFIED_READ: Zone = Zone::new(13, "verified.read");
/// The caller's callback for an entry it did not want.
pub static ON_OTHER: Zone = Zone::new(14, "on_other");
/// Reading a central directory from spans of a file.
pub static DIR_READ_FROM: Zone = Zone::new(20, "dir.read_from");
/// Reading one span for the directory reader.
pub static DIR_SPAN: Zone = Zone::new(21, "dir.span");
/// Finding the end record in the tail.
pub static DIR_LOCATE: Zone = Zone::new(22, "dir.locate");
/// Parsing the central directory records.
pub static DIR_PARSE: Zone = Zone::new(23, "dir.parse");
