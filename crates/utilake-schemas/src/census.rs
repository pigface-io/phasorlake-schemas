//! The census: which shape every block of every file of a month had.
//!
//! The files of a month are downloaded once (`utilake-schemas fetch`) into a
//! directory as nemweb lays the month out: `DATA/` (and the big-table
//! directories) of per-table zips, `CTL/` of control files, or one zip of
//! the whole month. The census then reads each file of the directory in
//! full, offline, nested zips included, and records every I row it holds:
//! a file of many tables has a block per table, anywhere in the file, and
//! all are seen. Each block's columns are joined with the types the
//! month's control files load them as, and every column's values are
//! profiled ([`crate::profile`]).
//!
//! It is written to `{root}/evidence/census/{YYYY-MM}.toml`: every file of
//! the month with its sha256, size and kind, and each of its blocks in file
//! order, in an order that does not depend on the order of work. When a
//! file was published is not the census's: `fetch` records it in
//! `sources/{YYYY-MM}.tsv`.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::catalog::Catalog;
use crate::ctl::CtlFile;
use crate::mms::scan::{Flow, Pos, ScanOptions, Sink, scan_with};
use crate::mms::{IRow, Row, RowError, RowFailure, ScanError, SinkError};
use crate::month::YearMonth;
use crate::profile::ColumnProfile;
use crate::shape::{CanonicalShape, ShapeId, TypeClass};
use crate::zip::{ArchiveError, WalkError, walk_csv_entries_in};

/// Why the census of a month could not be taken or recorded.
#[derive(Debug, thiserror::Error)]
pub enum CensusError {
    /// A census file or directory that cannot be read.
    #[error("The census file {path} cannot be read: {message}")]
    Read {
        /// The file.
        path: String,
        /// What is wrong.
        message: String,
    },
    /// A census file that cannot be written.
    #[error("The census file {path} could not be written.")]
    Write {
        /// The file.
        path: String,
        /// Cause.
        #[source]
        source: std::io::Error,
    },
    /// No file of the month is in the directory.
    #[error(
        "No file is in the directory {path}; download the month first with `utilake-schemas fetch`."
    )]
    Empty {
        /// The directory.
        path: String,
    },
}

/// A file, or an entry of one, that could not be read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Unread {
    /// The file (and entry).
    pub file: String,
    /// Why.
    pub error: String,
}

/// One block of one file, as the census saw it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockShape {
    /// The block's shape: its report pair and column names.
    pub shape: CanonicalShape,
    /// The version AEMO stamped on it.
    pub version: u16,
    /// Its D rows.
    pub rows: u64,
    /// Where its rows lie in time, when a date-like column was there.
    pub span: Option<Span>,
    /// What the month's control file declares each column as, in column
    /// order; `unknown` where none does.
    pub ctl: Vec<TypeClass>,
    /// What each column's values were, in column order.
    pub profile: Vec<ColumnProfile>,
}

/// One file of a month and where it came from.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FileCensus {
    /// The file it was read out of: itself, or a whole-month zip.
    pub source: String,
    /// The sha256 of that file.
    pub sha256: String,
    /// Its size in bytes.
    pub size: u64,
    /// The kind of file by its name (see [`file_kind`]).
    pub kind: String,
    /// Its blocks, in file order.
    pub blocks: Vec<BlockShape>,
}

/// The census of one month: per file name, its blocks in file order. A
/// file that could not be read has none, and `problems` says why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MonthCensus {
    /// The month.
    pub month: YearMonth,
    /// File name to what was read of it.
    pub files: BTreeMap<String, FileCensus>,
    /// Files that could not be read.
    pub problems: Vec<Unread>,
}

/// The table a control file loads, from its name in either archive naming
/// scheme, upper-case. A split digit of the old scheme stays on the name.
#[must_use]
pub fn ctl_table(file_name: &str) -> Option<String> {
    if !file_name.to_ascii_lowercase().ends_with(".ctl") {
        return None;
    }
    crate::names::parse(file_name).map(|n| n.table)
}

/// The names under which a control file or a data stem may be looked up:
/// as written, and without a split digit.
pub(crate) fn table_keys(table: &str) -> Vec<String> {
    let upper = table.to_ascii_uppercase();
    let trimmed = upper.trim_end_matches(|c: char| c.is_ascii_digit());
    if trimmed.is_empty() || trimmed == upper {
        vec![upper]
    } else {
        vec![trimmed.to_owned(), upper]
    }
}

/// Whether the control file for `ctl_table` describes the data file for
/// `data_table`: the same name, a split of it, or with `siblings` the
/// daily `_D` file of the same table.
fn ctl_serves(ctl_table: &str, data_table: &str, siblings: bool) -> bool {
    let wanted = table_keys(data_table);
    let mut have = table_keys(ctl_table);
    if siblings && let Some(base) = ctl_table.to_ascii_uppercase().strip_suffix("_D") {
        have.extend(table_keys(base));
    }
    have.iter().any(|k| wanted.contains(k))
}

/// The control files of a month, with the table each one's file name says.
#[derive(Debug, Default)]
pub struct Ctls(Vec<(String, CtlFile)>);

impl Ctls {
    /// Control files parsed from `(file name, text)` pairs; ones that do
    /// not parse are left out.
    pub fn parse<'a>(files: impl IntoIterator<Item = (&'a str, &'a str)>) -> Self {
        let mut out: Vec<(String, CtlFile)> = files
            .into_iter()
            .filter_map(|(name, text)| {
                let table = ctl_table(name)?;
                let ctl = crate::ctl::parse(name, text).ok()?;
                Some((table, ctl))
            })
            .collect();
        out.sort_by(|a, b| a.0.cmp(&b.0));
        Self(out)
    }

    /// The control file that describes `table`'s file: one of the same
    /// table, else of a split of it, else the daily `_D` file's.
    pub fn for_table(&self, tables: &[&str]) -> Option<&CtlFile> {
        [false, true].into_iter().find_map(|siblings| {
            tables.iter().find_map(|t| {
                self.0
                    .iter()
                    .find(|(ct, _)| ctl_serves(ct, t, siblings))
                    .map(|(_, c)| c)
            })
        })
    }
}

/// A block as read, before it is typed: report pair, version, columns and
/// rows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawBlock {
    /// Report type.
    pub report_type: String,
    /// Report subtype.
    pub report_subtype: String,
    /// Version.
    pub version: u16,
    /// Column names, normalised.
    pub columns: Vec<String>,
    /// D rows.
    pub rows: u64,
    /// For every column whose first value is written as a date
    /// (`YYYY/MM/DD…`): its index, and the least and greatest value over
    /// every D row, as written.
    pub bounds: Vec<(usize, Vec<u8>, Vec<u8>)>,
    /// Every column's profile, in column order, over every D row.
    pub profile: Vec<ColumnProfile>,
}

/// Where a block's rows lie in time: the least and greatest value of its
/// time column over every D row, as written (AEMO's `YYYY/MM/DD HH:MM:SS`
/// orders as text).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Span {
    /// The least value.
    pub first: String,
    /// The greatest value.
    pub last: String,
    /// The column the values are of.
    pub column: String,
}

/// Columns that name the time an observation is for, most specific first:
/// the span of a block whose table declares no time column is of the first
/// of these it has.
const TARGET_TIME: [&str; 9] = [
    "INTERVAL_DATETIME",
    "DATETIME",
    "SETTLEMENTDATE",
    "TRADINGDATE",
    "DAY",
    "EFFECTIVEDATE",
    "START_DATE",
    "STARTDATE",
    "RUN_DATETIME",
];

/// Whether a field is written as a date: `YYYY/MM/DD` at least.
fn date_like(v: &[u8]) -> bool {
    v.len() >= 10
        && v[4] == b'/'
        && v[7] == b'/'
        && [0, 1, 2, 3, 5, 6, 8, 9]
            .iter()
            .all(|&i| v[i].is_ascii_digit())
}

/// A sink that keeps the I rows, counts the D rows of each block and
/// tracks the least and greatest value of every date-like column.
#[derive(Default)]
struct Blocks {
    blocks: Vec<RawBlock>,
    /// Whether the current block's first D row has been seen.
    probed: bool,
}

impl Sink for Blocks {
    fn comment(&mut self, _: &Row<'_>, _: Pos) -> Result<(), SinkError> {
        Ok(())
    }
    fn header(&mut self, irow: &IRow<'_>, _: Pos) -> Result<(), SinkError> {
        self.blocks.push(RawBlock {
            report_type: irow.report_type.to_owned(),
            report_subtype: irow.report_subtype.to_owned(),
            version: irow.version,
            columns: irow.columns.iter().map(|c| (*c).to_owned()).collect(),
            rows: 0,
            bounds: Vec::new(),
            profile: vec![ColumnProfile::default(); irow.columns.len()],
        });
        self.probed = false;
        Ok(())
    }
    fn row(&mut self, row: &Row<'_>, _: Pos) -> Result<(), RowFailure> {
        // The scanner refuses a D row before any I row.
        let b = self.blocks.last_mut().expect("a D row follows an I row");
        b.rows += 1;
        // Data fields follow the four D,type,subtype,version cells.
        for (j, p) in b.profile.iter_mut().enumerate() {
            p.observe(row.get(4 + j));
        }
        if !self.probed {
            self.probed = true;
            for j in 0..b.columns.len() {
                if let Some(v) = row.get(4 + j)
                    && date_like(v)
                {
                    b.bounds.push((j, v.to_vec(), v.to_vec()));
                }
            }
            return Ok(());
        }
        for (j, min, max) in &mut b.bounds {
            if let Some(v) = row.get(4 + *j) {
                if v < min.as_slice() {
                    min.clear();
                    min.extend_from_slice(v);
                } else if v > max.as_slice() {
                    max.clear();
                    max.extend_from_slice(v);
                }
            }
        }
        Ok(())
    }
    fn bad_row(&mut self, _: &[u8], _: Pos, _: &RowError) -> Flow {
        Flow::Continue
    }
    fn footer(&mut self, _: Option<u64>, _: u64, _: Pos) -> Result<(), SinkError> {
        Ok(())
    }
}

/// Every block of one CSV stream, in order: I rows, D rows counted and
/// their date-like columns bounded.
pub fn read_blocks(
    reader: impl std::io::Read,
    buf: &mut Vec<u8>,
) -> Result<Vec<RawBlock>, ScanError> {
    let mut blocks = Blocks::default();
    let opts = ScanOptions {
        max_bad_rows: u64::MAX,
        ..Default::default()
    };
    scan_with(reader, &mut blocks, &opts, buf)?;
    Ok(blocks.blocks)
}

/// Every file under `dir`, at any depth, sorted: hidden files (`.DS_Store`,
/// macOS `._` files) and partial downloads (`.part`) left out.
pub fn files_under(dir: &Path) -> Result<Vec<PathBuf>, CensusError> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).map_err(|e| read_error(&d, &e))? {
            let p = e.map_err(|e| read_error(&d, &e))?.path();
            let name = p.file_name().map(|n| n.to_string_lossy().into_owned());
            if name.as_deref().is_none_or(|n| n.starts_with('.')) {
                continue;
            }
            if p.is_dir() {
                stack.push(p);
            } else if !crate::zip::has_extension(&p.to_string_lossy(), ".part") {
                out.push(p);
            }
        }
    }
    out.sort();
    Ok(out)
}

/// Every file of a month's directory (see [`files_under`]); an error when
/// there is none, which says to download the month first.
pub fn month_files(dir: &Path) -> Result<Vec<PathBuf>, CensusError> {
    let files = files_under(dir)?;
    if files.is_empty() {
        return Err(CensusError::Empty {
            path: dir.display().to_string(),
        });
    }
    Ok(files)
}

/// The file a walked entry belongs to: the innermost zip it was read out
/// of (`MMSDM_2012_01.zip!…/PUBLIC_DVD_X_201201010000.zip!X.CSV` is
/// `PUBLIC_DVD_X_201201010000.zip`), else the file itself.
fn file_key(outer: &str, entry: &str) -> String {
    match entry.rsplit_once('!') {
        Some((zip, _)) => base(zip).to_owned(),
        None => outer.to_owned(),
    }
}

/// The base name of an entry path, nested names joined with `!`.
pub(crate) fn base(name: &str) -> &str {
    name.rsplit(['/', '\\', '!']).next().unwrap_or(name)
}

/// The blocks of each file of a month, per file they were read out of: a
/// month published both as a directory and as a whole-month zip (MMSDM
/// 2020-02) holds most files twice.
#[derive(Default)]
struct Copies(HashMap<String, Vec<(String, Vec<RawBlock>)>>);

impl Copies {
    /// Takes in what another reading found, after what this one did.
    fn merge(&mut self, other: Self) {
        for (key, copies) in other.0 {
            for (source, blocks) in copies {
                self.add(&key, &source, blocks);
            }
        }
    }

    /// Blocks of file `key` read out of the file `source`; many CSVs of
    /// one zip add to one copy.
    fn add(&mut self, key: &str, source: &str, blocks: Vec<RawBlock>) {
        let copies = self.0.entry(key.to_owned()).or_default();
        match copies.iter_mut().find(|(s, _)| s == source) {
            Some((_, b)) => b.extend(blocks),
            None => copies.push((source.to_owned(), blocks)),
        }
    }

    /// One entry per file: the copy that is the file itself (a month
    /// directory's zip) else the first by source name. Another copy with
    /// the same blocks (shape, version and rows of each) is the same file
    /// and is dropped; one that differs is kept as `{source}!{key}`.
    fn settle(self) -> BTreeMap<String, (String, Vec<RawBlock>)> {
        let mut out = BTreeMap::new();
        for (key, mut copies) in self.0 {
            copies.sort_by(|a, b| (a.0 != key, &a.0).cmp(&(b.0 != key, &b.0)));
            let mut kept: Vec<Vec<RawBlock>> = Vec::new();
            for (i, (source, blocks)) in copies.into_iter().enumerate() {
                if kept.contains(&blocks) {
                    continue;
                }
                let name = if i == 0 {
                    key.clone()
                } else {
                    format!("{source}!{key}")
                };
                out.insert(name, (source, blocks.clone()));
                kept.push(blocks);
            }
        }
        out
    }
}

/// What a month's files gave while they were read.
#[derive(Default)]
struct Reading {
    raw: Copies,
    /// File name to its sha256 and size.
    digests: HashMap<String, (String, u64)>,
    ctls: Vec<(String, String)>,
    problems: Vec<Unread>,
    /// One read buffer for every file and entry of the month.
    buf: Vec<u8>,
}

/// What reading one file gave: a CSV's blocks, or how the walk of a zip
/// ended.
enum Took {
    Csv(Result<Vec<RawBlock>, ScanError>),
    Zip(Result<usize, WalkError<ScanError>>),
}

/// A control file's text, kept when it can be read.
fn push_ctl(ctls: &mut Vec<(String, String)>, name: String, r: &mut dyn std::io::Read) {
    let mut text = Vec::new();
    if r.read_to_end(&mut text).is_ok() {
        ctls.push((name, String::from_utf8_lossy(&text).into_owned()));
    }
}

impl Reading {
    /// Takes in what another reading found, after what this one did.
    fn merge(&mut self, other: Self) {
        self.raw.merge(other.raw);
        self.digests.extend(other.digests);
        self.ctls.extend(other.ctls);
        self.problems.extend(other.problems);
    }

    /// What a read of file `shown` gave, or none when it failed, which is
    /// a problem of the month.
    fn readable<T>(&mut self, r: std::io::Result<T>, shown: &str) -> Option<T> {
        r.map_err(|e| {
            let error = format!("The file cannot be read: {e}.");
            self.problems.push(Unread {
                file: shown.to_owned(),
                error,
            });
        })
        .ok()
    }

    /// A file that could not be read: a problem, and a file of no blocks.
    fn unread(&mut self, name: &str, file: String, error: String) {
        self.problems.push(Unread { file, error });
        self.raw.add(name, name, Vec::new());
    }

    /// One file of the month: a control file's text, or the blocks of a
    /// CSV or of every CSV of a zip, with the file's digest.
    fn file(&mut self, path: &Path, tmp: &Path) {
        let name = base(&path.to_string_lossy()).to_owned();
        let shown = path.display().to_string();
        let is = |ext: &str| crate::zip::has_extension(&name, ext);
        if is(".ctl") {
            if let Ok(mut f) = std::fs::File::open(path) {
                push_ctl(&mut self.ctls, name, &mut f);
            }
            return;
        }
        if !is(".csv") && !is(".zip") {
            return;
        }
        // Read once: hashed as it is read, and what the reading skipped
        // (a zip's directory) after.
        let Some((f, tally)) = self.readable(crate::files::HashedFile::open(path), &shown) else {
            return;
        };
        let mut entry_err = None;
        let (raw, buf, ctls) = (&mut self.raw, &mut self.buf, &mut self.ctls);
        let read = if is(".csv") {
            Took::Csv(read_blocks(f, buf))
        } else {
            Took::Zip(walk_csv_entries_in::<_, ScanError>(
                f,
                &shown,
                tmp,
                &mut |e| {
                    let key = file_key(&name, &e.name);
                    let entry = e.name.clone();
                    let blocks =
                        read_blocks(e.reader, buf).inspect_err(|_| entry_err = Some(entry))?;
                    raw.add(&key, &name, blocks);
                    Ok(())
                },
                &mut |e| {
                    let b = base(&e.name).to_owned();
                    if crate::zip::has_extension(&b, ".ctl") {
                        push_ctl(ctls, b, e.reader);
                    }
                },
            ))
        };
        let Some(digest) = self.readable(tally.finish(path), &shown) else {
            return;
        };
        self.digests.insert(name.clone(), digest);
        let walked = match read {
            Took::Csv(Ok(blocks)) => return self.raw.add(&name, &name, blocks),
            Took::Csv(Err(e)) => return self.unread(&name, shown, e.to_string()),
            Took::Zip(walked) => walked,
        };
        match walked {
            // A whole-month zip holds control files beside data zips; a
            // zip of control files only has no CSV, and that is fine.
            Ok(_) | Err(WalkError::Archive(ArchiveError::NoCsv { .. })) => {}
            Err(WalkError::Archive(a)) => self.unread(&name, shown, a.to_string()),
            Err(WalkError::Callback(e)) => {
                let at = format!("{shown}!{}", entry_err.unwrap_or_default());
                self.unread(&name, at, e.to_string());
            }
        }
    }
}

/// The census of `month` from its `files`: every block of every data
/// file, typed by the control files among them (loose, or inside a
/// whole-month zip). A file that cannot be read is a problem of its own
/// and the census goes on. `tmp` takes nested zips while they are read.
pub fn census_month(
    catalog: &Catalog,
    month: YearMonth,
    files: &[PathBuf],
    tmp: &Path,
    jobs: usize,
) -> MonthCensus {
    // The month's documentation (the data model's create and upgrade
    // scripts and their reports) is not market data.
    let market: Vec<&PathBuf> = files
        .iter()
        .filter(|p| {
            !p.components()
                .any(|c| c.as_os_str().eq_ignore_ascii_case("DOCUMENTATION"))
        })
        .collect();
    // Each file read on its own, on `jobs` threads with a spool directory
    // each, then taken in in the files' order: the same census as one
    // thread reading them in turn.
    let read = crate::pool::map(jobs, &market, |w, path| {
        let mut r = Reading::default();
        let spool = tmp.join(format!("thread-{w}"));
        let _ = std::fs::create_dir_all(&spool);
        r.file(path, &spool);
        r
    });
    let mut r = Reading::default();
    for one in read {
        r.merge(one);
    }
    let ctls = Ctls::parse(r.ctls.iter().map(|(n, t)| (n.as_str(), t.as_str())));
    let files = r
        .raw
        .settle()
        .into_iter()
        .map(|(name, (source, blocks))| {
            let file_table = crate::names::parse(&name).map(|n| n.table);
            let blocks = blocks
                .into_iter()
                .map(|b| block_shape(catalog, &ctls, file_table.as_deref(), b))
                .collect();
            let (sha256, size) = r.digests.get(&source).cloned().unwrap_or_default();
            let kind = file_kind(&name);
            let file = FileCensus {
                source,
                sha256,
                size,
                kind,
                blocks,
            };
            (name, file)
        })
        .collect();
    MonthCensus {
        month,
        files,
        problems: r.problems,
    }
}

/// A block's shape: its columns typed by the control file of the table
/// that claims its report pair, else of the table its file is named for.
pub fn block_shape(
    catalog: &Catalog,
    ctls: &Ctls,
    file_table: Option<&str>,
    b: RawBlock,
) -> BlockShape {
    let declared = catalog.declared_by_pair(&b.report_type, &b.report_subtype);
    let table = declared.map(|t| t.name.as_str());
    let keys: Vec<&str> = table.into_iter().chain(file_table).collect();
    let ctl = ctls.for_table(&keys);
    // The span is of the table's time column when it is there and dated,
    // else of the first time-naming column, else of the first dated column.
    let bounded = |name: &str| {
        b.columns
            .iter()
            .position(|c| c == name)
            .and_then(|j| b.bounds.iter().find(|(i, ..)| *i == j))
    };
    let span = declared
        .and_then(|t| t.time_column.as_deref())
        .and_then(bounded)
        .or_else(|| TARGET_TIME.iter().find_map(|n| bounded(n)))
        .or_else(|| b.bounds.first())
        .map(|(j, min, max)| Span {
            first: String::from_utf8_lossy(min).into_owned(),
            last: String::from_utf8_lossy(max).into_owned(),
            column: b.columns[*j].clone(),
        });
    let class = |column: &str| {
        ctl.and_then(|c| {
            c.data_columns()
                .find(|d| d.name.eq_ignore_ascii_case(column))
                .and_then(|d| d.ty.type_class())
        })
        .unwrap_or(TypeClass::Unknown)
    };
    BlockShape {
        version: b.version,
        rows: b.rows,
        span,
        ctl: b.columns.iter().map(|c| class(c)).collect(),
        profile: b.profile,
        shape: CanonicalShape {
            columns: b.columns,
            report_type: b.report_type,
            report_subtype: b.report_subtype,
        },
    }
}

/// The kind of a file by its name: `archive` (`PUBLIC_ARCHIVE#…`), `dvd`
/// (`PUBLIC_DVD_…`) or `other`, with `#all` when it holds every run of a
/// forecast (`#ALL#`, `_ALL_`). The report folders' files are of kind
/// `reports/archive` and `reports/current` ([`crate::reports::kind`]).
pub fn file_kind(name: &str) -> String {
    let upper = name.to_ascii_uppercase();
    let naming = if upper.starts_with("PUBLIC_ARCHIVE#") {
        "archive"
    } else if upper.starts_with("PUBLIC_DVD_") {
        "dvd"
    } else {
        "other"
    };
    if upper.contains("#ALL#") || upper.contains("_ALL_") {
        format!("{naming}#all")
    } else {
        naming.to_owned()
    }
}

/// One file's entry in a month's census.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileRecord {
    /// The file it was read out of.
    pub source: String,
    /// The sha256 of that file.
    pub sha256: String,
    /// Its size in bytes.
    pub size: u64,
    /// `dvd`, `archive` or `other`, with `#all` for a file of every run.
    pub kind: String,
    /// Its blocks, in file order.
    #[serde(default)]
    pub block: Vec<BlockRecord>,
}

/// One block of a file in a month's census: its shape, and per column (in
/// column order) what the control file declared and what the values were.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BlockRecord {
    /// Its shape.
    pub shape: ShapeId,
    /// Its report type and subtype.
    pub report: (String, String),
    /// The version AEMO stamped on it.
    pub version: u16,
    /// Its D rows.
    pub rows: u64,
    /// The time column its span is of.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time: Option<String>,
    /// The least value of that column, as written.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first: Option<String>,
    /// The greatest value of that column, as written.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last: Option<String>,
    /// The column names.
    pub columns: Vec<String>,
    /// The declared class of each column, one letter each
    /// ([`TypeClass::letter`]).
    pub ctl: String,
    /// The observed kind of each column, one letter each
    /// ([`utilake_iceberg::Kind::letter`]).
    pub kinds: String,
    /// Rows without a value, per column.
    pub nulls: Vec<u64>,
    /// The most digits before the point, per column.
    pub int_digits: Vec<u8>,
    /// The most digits after it, per column.
    pub scale: Vec<u8>,
    /// The most significant digits of any one value, per column.
    #[serde(default)]
    pub sig_digits: Vec<u8>,
    /// The longest value in bytes, per column.
    pub max_len: Vec<u32>,
    /// The least number or time, per column; empty for none.
    pub min: Vec<String>,
    /// The greatest number or time, per column; empty for none.
    pub max: Vec<String>,
}

impl BlockRecord {
    /// The record of a block.
    pub fn of(b: &BlockShape) -> Self {
        let text = |v: &Option<Vec<u8>>| {
            v.as_deref()
                .map(|v| String::from_utf8_lossy(v).into_owned())
                .unwrap_or_default()
        };
        Self {
            shape: b.shape.id(),
            report: (b.shape.report_type.clone(), b.shape.report_subtype.clone()),
            version: b.version,
            rows: b.rows,
            time: b.span.as_ref().map(|s| s.column.clone()),
            first: b.span.as_ref().map(|s| s.first.clone()),
            last: b.span.as_ref().map(|s| s.last.clone()),
            columns: b.shape.columns.clone(),
            ctl: b.ctl.iter().map(|c| c.letter()).collect(),
            kinds: b.profile.iter().map(|p| p.kind.letter()).collect(),
            nulls: b.profile.iter().map(|p| p.nulls).collect(),
            int_digits: b.profile.iter().map(|p| p.int_digits).collect(),
            scale: b.profile.iter().map(|p| p.scale).collect(),
            sig_digits: b.profile.iter().map(|p| p.sig_digits).collect(),
            max_len: b.profile.iter().map(|p| p.max_len).collect(),
            min: b.profile.iter().map(|p| text(&p.min)).collect(),
            max: b.profile.iter().map(|p| text(&p.max)).collect(),
        }
    }

    /// The declared class of column `j`.
    pub fn ctl_of(&self, j: usize) -> TypeClass {
        self.ctl
            .chars()
            .nth(j)
            .and_then(TypeClass::of_letter)
            .unwrap_or(TypeClass::Unknown)
    }

    /// The profile of column `j`, as recorded.
    pub fn profile_of(&self, j: usize) -> ColumnProfile {
        let some = |v: Option<&String>| v.filter(|s| !s.is_empty()).map(|s| s.as_bytes().to_vec());
        ColumnProfile {
            nulls: self.nulls.get(j).copied().unwrap_or(0),
            kind: self
                .kinds
                .chars()
                .nth(j)
                .and_then(utilake_iceberg::Kind::of_letter)
                .unwrap_or_default(),
            int_digits: self.int_digits.get(j).copied().unwrap_or(0),
            scale: self.scale.get(j).copied().unwrap_or(0),
            sig_digits: self.sig_digits.get(j).copied().unwrap_or(0),
            max_len: self.max_len.get(j).copied().unwrap_or(0),
            min: some(self.min.get(j)),
            max: some(self.max.get(j)),
            ..ColumnProfile::default()
        }
    }
}

impl FileRecord {
    /// The record of a file.
    pub fn of(f: &FileCensus) -> Self {
        Self {
            source: f.source.clone(),
            sha256: f.sha256.clone(),
            size: f.size,
            kind: f.kind.clone(),
            block: f.blocks.iter().map(BlockRecord::of).collect(),
        }
    }

    /// The ids, in file order.
    pub fn ids(&self) -> Vec<ShapeId> {
        self.block.iter().map(|b| b.shape).collect()
    }
}

/// The directory of the census files under the repository root.
pub fn census_dir(root: &Path) -> PathBuf {
    root.join("evidence").join("census")
}

/// The census file of `month`.
pub fn census_path(root: &Path, month: YearMonth) -> PathBuf {
    census_dir(root).join(format!("{month}.toml"))
}

fn read_error(path: &Path, e: &dyn std::fmt::Display) -> CensusError {
    CensusError::Read {
        path: path.display().to_string(),
        message: e.to_string(),
    }
}

/// The text of `path`; `None` when there is no such file.
pub(crate) fn read_text(path: &Path) -> Result<Option<String>, CensusError> {
    match std::fs::read_to_string(path) {
        Ok(t) => Ok(Some(t)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(read_error(path, &e)),
    }
}

/// Writes a census file whole (see [`crate::files::write_atomic`]).
pub(crate) fn write_text(path: &Path, text: &str) -> Result<(), CensusError> {
    crate::files::write_atomic(path, text.as_bytes()).map_err(|source| CensusError::Write {
        path: path.display().to_string(),
        source,
    })
}

/// The census of `month` as recorded, file name to entry.
pub fn read_month(
    root: &Path,
    month: YearMonth,
) -> Result<Option<BTreeMap<String, FileRecord>>, CensusError> {
    let path = census_path(root, month);
    let Some(text) = read_text(&path)? else {
        return Ok(None);
    };
    toml::from_str(&text)
        .map(Some)
        .map_err(|e| read_error(&path, &e))
}

/// Records a month's census in its census file. Returns how many blocks
/// were recorded.
pub fn record(root: &Path, census: &MonthCensus) -> Result<usize, CensusError> {
    let month = census.month;
    let entries: BTreeMap<&str, FileRecord> = census
        .files
        .iter()
        .map(|(name, f)| (name.as_str(), FileRecord::of(f)))
        .collect();
    let text = format!(
        "# Generated by `utilake-schemas census`: every block of every file of {month}, where the file came from, and what each column held.\n{}",
        toml::to_string(&entries).expect("a census serialises")
    );
    write_text(&census_path(root, month), &text)?;
    Ok(census.files.values().map(|f| f.blocks.len()).sum())
}

#[cfg(test)]
pub(crate) mod tests;
