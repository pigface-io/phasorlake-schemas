//! The zones of a run of `phasor-schemas`, for `zonetrace` (feature
//! `zones`). The numbers are fixed, 1100–1199 for this crate, so a report
//! from one revision diffs against the next and merges with the readers'
//! (phasor-zip 1–99, phasor-mms-decode 400–499, phasor-plain-decode
//! 1200–1299). The tracer caches a zone by `id % 64`: a zone entered in an
//! inner loop takes an id whose remainder is 24–35 or 53–63, which the hot
//! zones of the readers leave free.

use zonetrace::Zone;

/// The whole `census` command.
pub static ROOT_CENSUS: Zone = Zone::new(1100, "schemas.census");
/// The whole `reports` command.
pub static ROOT_REPORTS: Zone = Zone::new(1101, "schemas.reports");
/// The whole `fold` command.
pub static ROOT_FOLD: Zone = Zone::new(1102, "schemas.fold");
/// The whole `export` command.
pub static ROOT_EXPORT: Zone = Zone::new(1103, "schemas.export");
/// The whole `sources` command.
pub static ROOT_SOURCES: Zone = Zone::new(1104, "schemas.sources");
/// The whole `models` command.
pub static ROOT_MODELS: Zone = Zone::new(1105, "schemas.models");

/// One report folder's census.
pub static CENSUS_FOLDER: Zone = Zone::new(1110, "census.folder");
/// One file of a folder, read.
pub static CENSUS_FILE: Zone = Zone::new(1111, "census.file");
/// One entry of a file (a loose file is its own one entry): id 1112 is slot 24.
pub static CENSUS_ENTRY: Zone = Zone::new(1112, "census.entry");
/// The reading of an entry's rows into blocks: the scan and the profiling.
pub static CENSUS_DECODE: Zone = Zone::new(1113, "census.decode");
/// A block's shape worked out from its rows and the control files.
pub static CENSUS_BLOCK_SHAPE: Zone = Zone::new(1114, "census.block_shape");
/// The hashing of a run's first and last files at the end of a folder.
pub static CENSUS_HASH: Zone = Zone::new(1115, "census.hash");
/// A folder's census written under evidence.
pub static CENSUS_RECORD: Zone = Zone::new(1116, "census.record");
/// One file of a month's directory, read (the `census` command).
pub static CENSUS_MONTH_FILE: Zone = Zone::new(1120, "census.month_file");

/// The fold's reading of every census.
pub static FOLD_OBSERVE: Zone = Zone::new(1130, "fold.observe");
/// The fold's reading of what was last published.
pub static FOLD_PUBLISHED: Zone = Zone::new(1131, "fold.published");
/// The fold itself: every census into each table's history.
pub static FOLD_FINISH: Zone = Zone::new(1132, "fold.finish");
/// The check that every rename was decided.
pub static FOLD_RENAMES: Zone = Zone::new(1133, "fold.renames");
/// The check that the data model's loads agree.
pub static FOLD_LOADS: Zone = Zone::new(1134, "fold.loads");
/// The histories written.
pub static FOLD_WRITE: Zone = Zone::new(1135, "fold.write");

/// The export's reading of every history and declaration.
pub static EXPORT_LOAD: Zone = Zone::new(1140, "export.load");
/// Every document rendered in memory.
pub static EXPORT_RENDER: Zone = Zone::new(1141, "export.render");
/// The documents written.
pub static EXPORT_WRITE: Zone = Zone::new(1142, "export.write");
/// The documents compared with what is on disk.
pub static EXPORT_CHECK: Zone = Zone::new(1143, "export.check");

/// One item of work on a pool thread.
pub static POOL_TASK: Zone = Zone::new(1150, "pool.task");
