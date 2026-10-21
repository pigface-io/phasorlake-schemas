//! The census of AEMO's report folders: every block of every file under
//! `Reports/ARCHIVE` and `Reports/CURRENT`, nested zips included.
//!
//! The MMSDM archive publishes a table a month at a time; the report
//! folders publish the same tables (and others) as they are produced, so a
//! layout AEMO has just changed shows there first. Each CSV's month comes
//! from the first date in its name (`PUBLIC_DISPATCHIS_202610200245_….CSV`
//! is 2026-10), else its zip's. Its blocks are typed by the control files
//! of that MMSDM month, or the last month before it that has them; a table
//! no control file loads has columns of unknown type.
//!
//! A folder holds many thousands of five-minute files, so the census of a
//! folder is a list of runs, not of files: CSVs in name order (their names
//! carry their time) that held a table in one shape and version, with the
//! first and last CSV, how many, and the block record of them all (rows,
//! time span, and each column's declared class and observed profile,
//! merged). A run ends at any change: another shape or version, or a
//! column whose values are of a wider kind or scale than the run's so far,
//! so the CSV where anything changed is kept. It is written to
//! `{root}/evidence/census/reports/{ARCHIVE|CURRENT}/{folder}.toml`.
//!
//! nemweb rewrites some files in place under the same name (most gas
//! reports under `Reports/CURRENT/`). nemweb-mirror keeps the first
//! version under the file's name and each later one with new data beside
//! it as `{name}@{listed time}`, each with its sidecar `{path}.json`. A
//! version is read as its name's kind and month, its blocks counted with
//! the rest, and the census lists each such file under `[[rewritten]]`
//! with every version classed against the ones before it. The folder's
//! journal in the mirror notes every listing, so it also gives how often
//! a file was listed again with nothing new.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::Read;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::Digest as _;

use crate::catalog::Catalog;
use crate::census::{
    BlockRecord, BlockShape, CensusError, Ctls, Span, Unread, base, block_shape, census_dir,
    files_under, read_blocks, read_text, write_text,
};
use crate::files::HashedFile;
use crate::month::YearMonth;
use crate::shape::TypeClass;
use crate::zip::{ArchiveError, WalkError, walk_csv_entries, walk_csv_entries_in};

/// The report roots under `Reports/`.
pub const ROOTS: [&str; 2] = ["ARCHIVE", "CURRENT"];

/// How far back a report month looks for an MMSDM month's control files.
const CTL_LOOKBACK_MONTHS: usize = 24;

/// The kind of a file of a report root: `reports/archive` or
/// `reports/current`.
pub fn kind(root: &str) -> String {
    format!("reports/{}", root.to_ascii_lowercase())
}

/// CSVs in a row that held one table in one shape and version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tally {
    /// The month of its first CSV.
    pub month: YearMonth,
    /// CSVs that held a block of it.
    pub csvs: u64,
    /// The first CSV, by name.
    pub first_csv: String,
    /// The last CSV, by name.
    pub last_csv: String,
    /// The file AEMO published the first CSV in (the CSV, or its zip).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub first_file: String,
    /// Its SHA-256.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub first_sha256: String,
    /// The file the last CSV was published in.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub last_file: String,
    /// Its SHA-256.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub last_sha256: String,
    /// Their blocks as one: rows summed, span and profiles merged.
    #[serde(flatten)]
    pub block: BlockRecord,
}

/// The census of one folder, as written.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FolderCensus {
    /// Files read.
    pub files: u64,
    /// CSVs read out of them.
    pub csvs: u64,
    /// Files that are neither zip nor CSV (a PDF, a web page).
    pub other: u64,
    /// Files or entries that could not be read.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unread: Vec<Unread>,
    /// Every run, in the order its first CSV was read.
    #[serde(default, rename = "block")]
    pub blocks: Vec<Tally>,
    /// Every file nemweb rewrote in place, with each of its versions.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rewritten: Vec<Rewritten>,
}

/// A file nemweb rewrote in place under the same name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rewritten {
    /// The file's name.
    pub file: String,
    /// Listings the mirror found with the bytes of a version it holds
    /// (journal `same`).
    #[serde(default)]
    pub relisted: u64,
    /// Listings of a zip packed again with the same entries (journal
    /// `repacked`).
    #[serde(default)]
    pub repacked: u64,
    /// Every version archived, the first first.
    #[serde(rename = "version")]
    pub versions: Vec<Version>,
}

/// One version of a file rewritten in place.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Version {
    /// The archived file: `{name}`, or `{name}@{listed time}`.
    pub file: String,
    /// What changed from the versions before it.
    pub change: Change,
    /// For `reverted`, the earlier version whose data it holds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub same_data_as: Option<String>,
    /// When nemweb listed it, from its sidecar: RFC 3339, AEST.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub listed_at: Option<String>,
    /// When AEMO published it (HTTP `Last-Modified`), from its sidecar:
    /// RFC 3339, UTC.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub published_at: Option<String>,
    /// Its SHA-256.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub sha256: String,
}

/// What a version changed from the ones before it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Change {
    /// The first version archived.
    First,
    /// Its data differs from the last version's.
    Data,
    /// Only the report's own time differs from the last version's.
    ReportTime,
    /// Its data is an earlier version's, not the last's: published, then
    /// withdrawn.
    Reverted,
}

/// The first month a name gives: the first run of at least six digits
/// that starts with a year from 1990 to 2099 and a month 01 to 12.
#[must_use]
pub fn name_month(name: &str) -> Option<YearMonth> {
    let b = name.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if !b[i].is_ascii_digit() {
            i += 1;
            continue;
        }
        let start = i;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
        if i - start >= 6 {
            let year: i32 = name[start..start + 4].parse().ok()?;
            let month: u8 = name[start + 4..start + 6].parse().ok()?;
            if (1990..2100).contains(&year)
                && let Some(m) = YearMonth::new(year, month)
            {
                return Some(m);
            }
        }
    }
    None
}

/// Control files per report month, read once from a directory of MMSDM
/// months (`{dir}/MMSDM_{YYYY}_{MM}/`, as `fetch` writes them).
pub struct CtlSource {
    dir: Option<PathBuf>,
    by_month: HashMap<YearMonth, Ctls>,
    read: HashMap<YearMonth, Option<Vec<(String, String)>>>,
}

impl CtlSource {
    /// Control files of the MMSDM months under `dir`; none without one.
    #[must_use]
    pub fn new(dir: Option<&Path>) -> Self {
        Self {
            dir: dir.map(Path::to_path_buf),
            by_month: HashMap::new(),
            read: HashMap::new(),
        }
    }

    /// The control files of `month`, else of the last MMSDM month before
    /// it that has them.
    pub fn for_month(&mut self, month: YearMonth) -> &Ctls {
        if !self.by_month.contains_key(&month) {
            let mut texts = Vec::new();
            let mut m = month;
            for _ in 0..CTL_LOOKBACK_MONTHS {
                if let Some(t) = self.month_texts(m) {
                    texts = t;
                    break;
                }
                m = m.prev();
            }
            let ctls = Ctls::parse(texts.iter().map(|(n, t)| (n.as_str(), t.as_str())));
            self.by_month.insert(month, ctls);
        }
        &self.by_month[&month]
    }

    /// The loose control files of a month directory: its `CTL/`, as
    /// `fetch` lays it out or as nemweb does.
    fn month_texts(&mut self, month: YearMonth) -> Option<Vec<(String, String)>> {
        if !self.read.contains_key(&month) {
            let name = format!("MMSDM_{}_{:02}", month.year(), month.month());
            let texts = self.dir.as_ref().and_then(|d| {
                let m = d.join(name);
                [m.join("CTL"), m.join("MMSDM_Historical_Data_SQLLoader/CTL")]
                    .iter()
                    .find_map(|d| ctl_texts(d))
            });
            self.read.insert(month, texts);
        }
        self.read[&month].clone()
    }
}

/// The control files directly in `dir`, by name; `None` for none.
fn ctl_texts(dir: &Path) -> Option<Vec<(String, String)>> {
    let mut t: Vec<(String, String)> = std::fs::read_dir(dir)
        .ok()?
        .filter_map(Result::ok)
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let wanted = !name.starts_with("._") && crate::zip::has_extension(&name, ".ctl");
            let bytes = wanted.then(|| std::fs::read(e.path()).ok()).flatten()?;
            Some((name, String::from_utf8_lossy(&bytes).into_owned()))
        })
        .collect();
    t.sort();
    (!t.is_empty()).then_some(t)
}

/// One run, gathering.
struct Acc {
    month: YearMonth,
    csvs: u64,
    first_csv: String,
    last_csv: String,
    /// The files the first and last CSV were read out of.
    first_path: PathBuf,
    last_path: PathBuf,
    merged: BlockShape,
}

/// What a folder's census gathers before it is written.
#[derive(Default)]
struct Gather {
    census: FolderCensus,
    runs: Vec<Acc>,
    /// The open run of each report pair.
    open: HashMap<(String, String), usize>,
    undated: u64,
}

/// Whether `b` continues run `a`: the same shape, version and declared
/// classes, and no column of a wider kind or scale than the run has seen.
fn continues(a: &Acc, b: &BlockShape) -> bool {
    let m = &a.merged;
    m.shape.id() == b.shape.id()
        && m.version == b.version
        && m.ctl == b.ctl
        && m.profile
            .iter()
            .zip(&b.profile)
            .all(|(p, q)| p.kind.join(q.kind) == p.kind && q.scale <= p.scale)
}

impl Gather {
    fn add(&mut self, month: YearMonth, csv: &str, path: &Path, b: BlockShape) {
        let key = (b.shape.report_type.clone(), b.shape.report_subtype.clone());
        let open = self.open.get(&key).copied();
        let Some(i) = open.filter(|i| continues(&self.runs[*i], &b)) else {
            self.open.insert(key, self.runs.len());
            self.runs.push(Acc {
                month,
                csvs: 1,
                first_csv: csv.to_owned(),
                last_csv: csv.to_owned(),
                first_path: path.to_path_buf(),
                last_path: path.to_path_buf(),
                merged: b,
            });
            return;
        };
        let a = &mut self.runs[i];
        a.csvs += 1;
        if csv < a.first_csv.as_str() {
            csv.clone_into(&mut a.first_csv);
            path.clone_into(&mut a.first_path);
        }
        if csv > a.last_csv.as_str() {
            csv.clone_into(&mut a.last_csv);
            path.clone_into(&mut a.last_path);
        }
        let m = &mut a.merged;
        m.rows += b.rows;
        m.span = match (m.span.take(), b.span) {
            (Some(x), Some(y)) => Some(Span {
                first: x.first.min(y.first),
                last: x.last.max(y.last),
                column: x.column,
            }),
            (x, y) => x.or(y),
        };
        for (p, q) in m.profile.iter_mut().zip(&b.profile) {
            p.merge(q);
        }
        for (c, d) in m.ctl.iter_mut().zip(b.ctl) {
            if d != TypeClass::Unknown {
                *c = d;
            }
        }
    }
}

/// A version's file name and listed time: `x.csv@20261021T191200+1000` is
/// version `20261021T191200+1000` of `x.csv`. An `@` not followed by a
/// compact RFC 3339 time is part of the name.
#[must_use]
pub fn version_of(name: &str) -> (&str, Option<&str>) {
    let stamp = |at: &str| {
        let b = at.as_bytes();
        let digits =
            |r: std::ops::Range<usize>| b.get(r).is_some_and(|d| d.iter().all(u8::is_ascii_digit));
        let zone = match b.get(15..) {
            Some(b"Z") => true,
            Some([b'+' | b'-', ..]) => b.len() == 20 && digits(16..20),
            _ => false,
        };
        digits(0..8) && b.get(8) == Some(&b'T') && digits(9..15) && zone
    };
    match name.rsplit_once('@') {
        Some((file, at)) if stamp(at) => (file, Some(at)),
        _ => (name, None),
    }
}

/// Where nemweb-mirror's journals of the report folders are, under its
/// home: `{mirror}/mirror/journal/{host}/Reports`.
#[must_use]
pub fn journals(mirror: &Path) -> PathBuf {
    let host = crate::fetch::BASE
        .split_once("://")
        .map_or(crate::fetch::BASE, |(_, h)| h);
    mirror.join("mirror/journal").join(host).join("Reports")
}

/// What the census reads of a version's sidecar, `{path}.json`. Fields it
/// does not know are passed over.
#[derive(Debug, Deserialize)]
struct Sidecar {
    url: String,
    #[serde(rename = "size")]
    _size: serde::de::IgnoredAny,
    sha256: String,
    fetched_at: String,
    /// The server's `Last-Modified`, as it sent it.
    #[serde(default)]
    last_modified: Option<String>,
    /// The same as an instant, RFC 3339.
    #[serde(default)]
    last_modified_at: Option<String>,
    /// The time the listing showed, as it wrote it.
    #[serde(default)]
    listed_modified: Option<String>,
    /// The same as an instant, RFC 3339.
    #[serde(default)]
    listed_modified_at: Option<String>,
}

/// The sidecar of the file at `path`, when it has one that reads. Its
/// instants are written again as RFC 3339; one a sidecar written before
/// they were kept lacks is read from the text beside it (market time when
/// the text gives no offset), and a text that is not a time has none. An
/// instant without an offset, or a `fetched_at` that is not a time with
/// one, is a sidecar that does not read.
fn sidecar(path: &Path) -> Option<Sidecar> {
    use crate::stamp::{NEM_OFFSET_SECS, parse, parse_rfc3339};
    let mut p = path.as_os_str().to_owned();
    p.push(".json");
    let mut sc: Sidecar = serde_json::from_slice(&std::fs::read(p).ok()?).ok()?;
    parse_rfc3339(&sc.fetched_at)?;
    let instant = |at: Option<String>, text: Option<&str>| -> Option<Option<String>> {
        match at {
            Some(at) => parse_rfc3339(&at).map(|t| Some(t.to_string())),
            None => Some(
                text.and_then(|t| parse(t, NEM_OFFSET_SECS))
                    .map(|t| t.to_string()),
            ),
        }
    };
    sc.last_modified_at = instant(sc.last_modified_at.take(), sc.last_modified.as_deref())?;
    sc.listed_modified_at = instant(sc.listed_modified_at.take(), sc.listed_modified.as_deref())?;
    Some(sc)
}

/// One line of a folder's journal, as far as the census reads it.
#[derive(Deserialize)]
struct Event {
    url: String,
    event: String,
}

/// How often the journal at `journal` found each file, by URL, listed
/// again with nothing new: (`same`, `repacked`). A line that does not read
/// is passed over.
fn journal_counts(journal: Option<&Path>) -> HashMap<String, (u64, u64)> {
    let text = journal
        .and_then(|p| std::fs::read_to_string(p).ok())
        .unwrap_or_default();
    let mut out: HashMap<String, (u64, u64)> = HashMap::new();
    for e in text
        .lines()
        .filter_map(|l| serde_json::from_str::<Event>(l).ok())
    {
        let same = match e.event.as_str() {
            "same" => true,
            "repacked" => false,
            _ => continue,
        };
        let c = out.entry(e.url).or_default();
        if same {
            c.0 += 1;
        } else {
            c.1 += 1;
        }
    }
    out
}

/// A file rewritten in place, before its versions are read.
struct Rewrite {
    file: String,
    relisted: u64,
    repacked: u64,
    /// Each version's index in the folder's files and its sidecar, the
    /// first first.
    versions: Vec<(usize, Option<Sidecar>)>,
}

/// The files of `files` nemweb rewrote in place: more than one version,
/// or listings `counts` found with nothing new.
fn rewrites(files: &[PathBuf], counts: &HashMap<String, (u64, u64)>) -> Vec<Rewrite> {
    let mut by_file: BTreeMap<String, Vec<(Option<String>, usize)>> = BTreeMap::new();
    for (i, path) in files.iter().enumerate() {
        let name = base(&path.to_string_lossy()).to_owned();
        let (file, at) = version_of(&name);
        by_file
            .entry(file.to_owned())
            .or_default()
            .push((at.map(str::to_owned), i));
    }
    by_file
        .into_iter()
        .filter(|(_, v)| v.len() > 1 || !counts.is_empty())
        .filter_map(|(file, mut v)| {
            // The first under the file's name, the rest by listed time.
            v.sort();
            let versions: Vec<_> = v
                .into_iter()
                .map(|(_, i)| (i, sidecar(&files[i])))
                .collect();
            let (relisted, repacked) = versions
                .iter()
                .find_map(|(_, s)| s.as_ref())
                .and_then(|s| counts.get(&s.url))
                .copied()
                .unwrap_or_default();
            (versions.len() > 1 || relisted + repacked > 0).then_some(Rewrite {
                file,
                relisted,
                repacked,
                versions,
            })
        })
        .collect()
}

/// A print of what a CSV holds apart from when the report was made, taken
/// as the CSV streams past: AEMO's `C,` rows are left out when the first
/// row is one, else the values of a `current_date` column the header
/// names. Two versions of one print differ only in their report time.
struct Print<R> {
    inner: R,
    hasher: sha2::Sha256,
    /// The line so far.
    line: Vec<u8>,
    /// What the first line said to leave out: `None` before it.
    leave: Option<Leave>,
}

/// What a [`Print`] leaves out of each line.
#[derive(Clone, Copy)]
enum Leave {
    /// Lines that start `C,`.
    CRows,
    /// The field of this column, or nothing.
    Column(Option<usize>),
}

impl<R: Read> Print<R> {
    fn new(inner: R) -> Self {
        Self {
            inner,
            hasher: sha2::Sha256::new(),
            line: Vec::new(),
            leave: None,
        }
    }

    /// Takes in a whole line.
    fn end_line(&mut self) {
        let l = self.line.strip_suffix(b"\r").unwrap_or(&self.line);
        let leave = *self.leave.get_or_insert_with(|| {
            if l.starts_with(b"C,") {
                Leave::CRows
            } else {
                Leave::Column(
                    l.split(|&c| c == b',')
                        .position(|f| f.trim_ascii().eq_ignore_ascii_case(b"current_date")),
                )
            }
        });
        match leave {
            Leave::CRows if l.starts_with(b"C,") => {}
            Leave::Column(Some(j)) => {
                for (k, f) in l.split(|&c| c == b',').enumerate().filter(|(k, _)| *k != j) {
                    if k > usize::from(j == 0) {
                        self.hasher.update(b",");
                    }
                    self.hasher.update(f);
                }
                self.hasher.update(b"\n");
            }
            _ => {
                self.hasher.update(l);
                self.hasher.update(b"\n");
            }
        }
        self.line.clear();
    }

    /// The print, once the rest of the stream is read through.
    fn finish(mut self) -> std::io::Result<String> {
        std::io::copy(&mut self, &mut std::io::sink())?;
        if !self.line.is_empty() {
            self.end_line();
        }
        Ok(crate::files::hex(&self.hasher.finalize()))
    }
}

impl<R: Read> Read for Print<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        let mut rest = &buf[..n];
        while let Some(i) = memchr::memchr(b'\n', rest) {
            self.line.extend_from_slice(&rest[..i]);
            self.end_line();
            rest = &rest[i + 1..];
        }
        self.line.extend_from_slice(rest);
        Ok(n)
    }
}

/// The print of a file from the prints of its CSVs, by entry name.
fn print_of(mut parts: Vec<(String, String)>) -> String {
    parts.sort();
    let mut h = sha2::Sha256::new();
    for (name, print) in parts {
        h.update(format!("{name}\t{print}\n").as_bytes());
    }
    crate::files::hex(&h.finalize())
}

/// Each version of `r` classed against the ones before it, by the print
/// and SHA-256 each was read with (`read`, by index in `files`).
fn classed(
    r: Rewrite,
    files: &[PathBuf],
    read: &mut HashMap<usize, (Option<String>, String)>,
) -> Rewritten {
    // Each version before, by its print (none when it could not be read).
    let mut seen: Vec<(Option<String>, String)> = Vec::new();
    let mut versions = Vec::new();
    for (i, sc) in r.versions {
        let file = base(&files[i].to_string_lossy()).to_owned();
        let (print, sha256) = read.remove(&i).unwrap_or_default();
        let same = |p: &(Option<String>, String)| print.is_some() && p.0 == print;
        let (change, same_data_as) = match seen.split_last() {
            None => (Change::First, None),
            Some((last, _)) if same(last) => (Change::ReportTime, None),
            Some((_, before)) => match before.iter().rev().find(|p| same(p)) {
                Some(p) => (Change::Reverted, Some(p.1.clone())),
                None => (Change::Data, None),
            },
        };
        // A version that could not be read has the SHA-256 its sidecar
        // records.
        let (sha256, listed_at, published_at) = match sc {
            Some(s) => (
                if sha256.is_empty() { s.sha256 } else { sha256 },
                s.listed_modified_at,
                s.last_modified_at,
            ),
            None => (sha256, None, None),
        };
        versions.push(Version {
            file: file.clone(),
            change,
            same_data_as,
            listed_at,
            published_at,
            sha256,
        });
        seen.push((print, file));
    }
    Rewritten {
        file: r.file,
        relisted: r.relisted,
        repacked: r.repacked,
        versions,
    }
}

/// The census of the `files` of one folder. `journal` is the folder's
/// journal in nemweb-mirror, when there is one to count.
pub fn census_folder(
    catalog: &Catalog,
    ctls: &mut CtlSource,
    files: &[PathBuf],
    journal: Option<&Path>,
    tmp: &Path,
) -> FolderCensus {
    let mut g = Gather::default();
    let mut scan_buf = Vec::new();
    let rewrites = rewrites(files, &journal_counts(journal));
    // A version of a file rewritten in place is printed and hashed as it
    // is read; any other file only read.
    let versions: HashSet<usize> = rewrites
        .iter()
        .flat_map(|r| r.versions.iter().map(|(i, _)| *i))
        .collect();
    let mut read: HashMap<usize, (Option<String>, String)> = HashMap::new();
    for (i, path) in files.iter().enumerate() {
        let name = base(&path.to_string_lossy()).to_owned();
        let (file, _) = version_of(&name);
        let outer_month = name_month(file);
        g.census.files += 1;
        let hashed = versions.contains(&i);
        // The print of each CSV read, by entry name (a loose CSV's empty);
        // none once one cannot be taken.
        let mut prints = hashed.then(Vec::new);
        let mut one_csv =
            |csv: &str, r: &mut dyn std::io::Read, g: &mut Gather| -> Result<(), String> {
                let blocks = if let Some(parts) = prints.as_mut() {
                    let mut p = Print::new(r);
                    let blocks = read_blocks(&mut p, &mut scan_buf);
                    match p.finish() {
                        Ok(print) => {
                            let entry = if csv == name { "" } else { csv };
                            parts.push((entry.to_owned(), print));
                        }
                        Err(_) => prints = None,
                    }
                    blocks
                } else {
                    read_blocks(r, &mut scan_buf)
                }
                .map_err(|e| e.to_string())?;
                g.census.csvs += 1;
                let month = name_month(base(csv)).or(outer_month);
                let Some(month) = month else {
                    g.undated += 1;
                    return Ok(());
                };
                let c = ctls.for_month(month);
                for raw in blocks {
                    let b = block_shape(catalog, c, None, raw);
                    g.add(month, base(csv), path, b);
                }
                Ok(())
            };
        let is = |ext: &str| crate::zip::has_extension(file, ext);
        let mut tally = None;
        if is(".csv") {
            let res = if hashed {
                HashedFile::open(path)
                    .map_err(|e| e.to_string())
                    .and_then(|(mut f, t)| {
                        tally = Some(t);
                        one_csv(&name, &mut f, &mut g)
                    })
            } else {
                std::fs::File::open(path)
                    .map_err(|e| e.to_string())
                    .and_then(|mut f| one_csv(&name, &mut f, &mut g))
            };
            if let Err(error) = res {
                prints = None;
                g.census.unread.push(Unread { file: name, error });
            }
        } else if is(".zip") {
            let mut entry_err: Option<Unread> = None;
            let mut on_entry = |e: crate::zip::Entry<'_>| {
                one_csv(&e.name, e.reader, &mut g).inspect_err(|error| {
                    entry_err = Some(Unread {
                        file: format!("{name}!{}", e.name),
                        error: error.clone(),
                    });
                })
            };
            let walked = if hashed {
                let shown = path.display().to_string();
                match HashedFile::open(path) {
                    Ok((f, t)) => {
                        tally = Some(t);
                        walk_csv_entries_in(f, &shown, tmp, &mut on_entry, &mut |_| {})
                    }
                    Err(e) => Err(WalkError::Archive(ArchiveError::Open {
                        path: shown,
                        message: e.to_string(),
                    })),
                }
            } else {
                walk_csv_entries::<String>(path, tmp, &mut on_entry, &mut |_| {})
            };
            match walked {
                Ok(_) | Err(WalkError::Archive(ArchiveError::NoCsv { .. })) => {}
                Err(WalkError::Archive(a)) => {
                    prints = None;
                    g.census.unread.push(Unread {
                        file: name,
                        error: a.to_string(),
                    });
                }
                Err(WalkError::Callback(_)) => {
                    prints = None;
                    g.census.unread.extend(entry_err);
                }
            }
        } else {
            // Not a report: nothing to compare.
            prints = None;
            g.census.other += 1;
        }
        if hashed {
            let sha256 = tally
                .and_then(|t| t.finish(path).ok())
                .map(|(sha, _)| sha)
                .unwrap_or_default();
            read.insert(i, (prints.map(print_of), sha256));
        }
    }
    if g.undated > 0 {
        g.census.unread.push(Unread {
            file: String::new(),
            error: format!("{} CSV(s) whose month no name gives", g.undated),
        });
    }
    // A run's first and last files are hashed, for the marks that name
    // them; the files between are counted, not hashed.
    let hash = |p: &Path| {
        std::fs::File::open(p)
            .and_then(crate::files::sha256_of)
            .map(|(sha, _)| sha)
            .unwrap_or_default()
    };
    for a in g.runs {
        g.census.blocks.push(Tally {
            month: a.month,
            csvs: a.csvs,
            first_file: base(&a.first_path.to_string_lossy()).to_owned(),
            first_sha256: hash(&a.first_path),
            last_file: base(&a.last_path.to_string_lossy()).to_owned(),
            last_sha256: hash(&a.last_path),
            first_csv: a.first_csv,
            last_csv: a.last_csv,
            block: BlockRecord::of(&a.merged),
        });
    }
    g.census.rewritten = rewrites
        .into_iter()
        .map(|r| classed(r, files, &mut read))
        .collect();
    g.census
}

/// The folders directly under `dir`, in name order (hidden ones, macOS
/// `._` files among them, left out).
#[must_use]
pub fn top_folders(dir: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|rd| {
            rd.filter_map(Result::ok)
                .map(|e| e.path())
                .filter(|p| p.is_dir())
                .filter(|p| {
                    p.file_name()
                        .is_some_and(|n| !n.to_string_lossy().starts_with('.'))
                })
                .collect()
        })
        .unwrap_or_default();
    out.sort();
    out
}

/// Every folder at or under `under` that holds files, with its files in
/// name order, keyed by its path below `dir` (`/`-separated).
pub fn folders(dir: &Path, under: &Path) -> Result<BTreeMap<String, Vec<PathBuf>>, CensusError> {
    let mut out: BTreeMap<String, Vec<PathBuf>> = BTreeMap::new();
    for path in files_under(under)? {
        let parent = path.parent().unwrap_or(dir);
        let rel = parent.strip_prefix(dir).unwrap_or(parent);
        let key = rel
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("/");
        out.entry(key).or_default().push(path);
    }
    Ok(out)
}

/// Where the census of folder `rel` of report root `root` is written.
#[must_use]
pub fn folder_path(repo: &Path, root: &str, rel: &str) -> PathBuf {
    let name = if rel.is_empty() { "_root" } else { rel };
    census_dir(repo)
        .join("reports")
        .join(root)
        .join(format!("{name}.toml"))
}

/// Writes the census of a folder.
pub fn record_folder(
    repo: &Path,
    root: &str,
    rel: &str,
    census: &FolderCensus,
) -> Result<(), CensusError> {
    let text = format!(
        "# Generated by `utilake-schemas reports`: every shape of Reports/{root}/{rel}, per month, and what each column held.\n{}",
        toml::to_string(census).expect("a folder census serialises")
    );
    write_text(&folder_path(repo, root, rel), &text)
}

/// The census of a folder as written, if it was.
pub fn read_folder(
    repo: &Path,
    root: &str,
    rel: &str,
) -> Result<Option<FolderCensus>, CensusError> {
    let path = folder_path(repo, root, rel);
    let Some(text) = read_text(&path)? else {
        return Ok(None);
    };
    toml::from_str(&text)
        .map(Some)
        .map_err(|e| CensusError::Read {
            path: path.display().to_string(),
            message: e.to_string(),
        })
}

/// One folder's census as taken by [`census_reports`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Taken {
    /// `Reports/{ROOT}/{folder}`.
    pub folder: String,
    /// Its census.
    pub census: FolderCensus,
}

/// The census of every folder of both report roots under `reports` (the
/// directory holding `ARCHIVE/` and `CURRENT/`), each recorded under
/// `repo` as it is taken. `mmsdm` holds the MMSDM months whose control
/// files type the blocks; `journals` the folders' journals in
/// nemweb-mirror (see [`journals`]); `tmp` takes nested zips while they
/// are read.
pub fn census_reports(
    catalog: &Catalog,
    reports: &Path,
    (mmsdm, journals): (Option<&Path>, Option<&Path>),
    repo: &Path,
    tmp: &Path,
    jobs: usize,
) -> Result<Vec<Taken>, CensusError> {
    let mut work = Vec::new();
    for root in ROOTS {
        let dir = reports.join(root);
        for top in top_folders(&dir) {
            for (rel, files) in folders(&dir, &top)? {
                work.push((root, rel, files));
            }
        }
    }
    // Folders on `jobs` threads, each thread with its own control files
    // and spool directory; taken in the order of the folders.
    let ctls: Vec<std::sync::Mutex<CtlSource>> = (0..jobs.max(1))
        .map(|_| std::sync::Mutex::new(CtlSource::new(mmsdm)))
        .collect();
    let taken = crate::pool::map(jobs, &work, |w, (root, rel, files)| {
        let spool = tmp.join(format!("thread-{w}"));
        let _ = std::fs::create_dir_all(&spool);
        let mut c = ctls[w]
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let journal = journals.map(|j| j.join(root).join(rel).join("changes.ndjson"));
        let census = census_folder(catalog, &mut c, files, journal.as_deref(), &spool);
        record_folder(repo, root, rel, &census)?;
        let folder = format!("Reports/{root}/{rel}");
        Ok(Taken { folder, census })
    });
    taken.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::tests::scada;
    use crate::census::tests::{SCADA_CTL, put, three_tables, zip_of};

    /// The folder [`put_version`] writes into, under `Reports/`.
    const VIC_GAS: &str = "CURRENT/VicGas";

    /// The URL of `file` of [`VIC_GAS`].
    fn vic_gas_url(file: &str) -> String {
        format!("https://www.nemweb.com.au/Reports/{VIC_GAS}/{file}")
    }

    /// Writes `bytes` as nemweb-mirror keeps version `at` (`None`: the
    /// first) of `file` of [`VIC_GAS`] under `reports`, with its sidecar,
    /// listed at the time `at` names (the first at 19:00).
    fn put_version(reports: &Path, file: &str, at: Option<&str>, bytes: &[u8]) -> PathBuf {
        let rel = at.map_or(format!("{VIC_GAS}/{file}"), |a| {
            format!("{VIC_GAS}/{file}@{a}")
        });
        put(reports, &rel, bytes);
        let listed = at.map_or("2026-10-21T19:00:00+10:00".to_owned(), |a| {
            format!("2026-10-21T{}:{}:00+10:00", &a[9..11], &a[11..13])
        });
        let sidecar = format!(
            r#"{{"url":"{}","size":{},"sha256":"x","fetched_at":"2026-10-21T09:00:00Z","last_modified_at":"2026-10-21T08:00:00Z","listed_modified_at":"{listed}","tool":"t"}}"#,
            vic_gas_url(file),
            bytes.len()
        );
        put(reports, &format!("{rel}.json"), sidecar.as_bytes());
        reports.join(rel)
    }

    #[test]
    fn a_version_name_gives_its_file_and_listed_time() {
        assert_eq!(
            version_of("int128_v4_actual_linepack_1.csv@20261021T191200+1000"),
            (
                "int128_v4_actual_linepack_1.csv",
                Some("20261021T191200+1000")
            )
        );
        assert_eq!(
            version_of("a.zip@20261021T091200Z"),
            ("a.zip", Some("20261021T091200Z"))
        );
        for plain in [
            "a.zip",
            "a@b.csv",
            "a.csv@0123456789abcdef",
            "a.csv@20261021X191200+1000",
            "a.csv@20261021T191200+10",
            "a.csv@20261021T191200",
            "a.csv@20261021T191200Z.json",
        ] {
            assert_eq!(version_of(plain), (plain, None), "{plain}");
        }
        assert_eq!(
            journals(Path::new("/m")),
            Path::new("/m/mirror/journal/www.nemweb.com.au/Reports")
        );
    }

    #[test]
    fn older_sidecars_gain_their_instants_on_read() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("f.zip");
        let with = |json: &[u8]| {
            std::fs::write(dir.path().join("f.zip.json"), json).unwrap();
            sidecar(&path)
        };
        // The mirror's record before it kept the `_at` fields: the HTTP
        // date as the server sent it, and the listing's text in market
        // time with no offset. The instants are read from them.
        let sc = with(br#"{"url":"https://h/d/f.zip","size":3,"sha256":"x","crc32":1,"fetched_at":"2026-10-18T00:03:51Z","last_modified":"Mon, 18 May 2026 07:27:50 GMT","listed_size":3,"listed_modified":"Monday, May 18, 2026 05:27 PM","tool":"nemweb-mirror/0.1.0"}"#).unwrap();
        assert_eq!(
            (sc.url.as_str(), sc.sha256.as_str()),
            ("https://h/d/f.zip", "x")
        );
        assert_eq!(sc.last_modified_at.as_deref(), Some("2026-05-18T07:27:50Z"));
        assert_eq!(
            sc.listed_modified_at.as_deref(),
            Some("2026-05-18T17:27:00+10:00")
        );
        // The record with the `_at` fields reads the same, and what it
        // wrote there is kept over what the text would give.
        let kept = with(br#"{"url":"u","size":3,"sha256":"x","fetched_at":"2026-10-18T00:03:51Z","last_modified":"Mon, 18 May 2026 07:27:50 GMT","last_modified_at":"2026-05-18T08:27:50+01:00","listed_modified":"Monday, May 18, 2026 05:27 PM","listed_modified_at":"2026-05-18T17:27:00+10:00"}"#).unwrap();
        assert_eq!(
            kept.last_modified_at.as_deref(),
            Some("2026-05-18T08:27:50+01:00")
        );
        assert_eq!(kept.listed_modified_at, sc.listed_modified_at);
        // A text that is not a time has no instant; a zip entry's naive
        // time is market time.
        let odd = with(br#"{"url":"u","size":3,"sha256":"x","fetched_at":"2026-10-18T00:03:51Z","last_modified":"unknown","listed_modified":"2017-01-12 12:24:16"}"#).unwrap();
        assert_eq!(odd.last_modified_at, None);
        assert_eq!(
            odd.listed_modified_at.as_deref(),
            Some("2017-01-12T12:24:16+10:00")
        );
        // An `_at` without an offset, a fetched_at that is not a time, a
        // field missing, or no sidecar at all: none that reads.
        for bad in [
            &br#"{"url":"u","size":3,"sha256":"x","fetched_at":"2026-10-18T00:03:51Z","last_modified_at":"2026-05-18T07:27:50"}"#[..],
            br#"{"url":"u","size":3,"sha256":"x","fetched_at":"2026-10-18T00:03:51Z","listed_modified_at":"2026-05-18T07:27:50"}"#,
            br#"{"url":"u","size":3,"sha256":"x","fetched_at":"t"}"#,
            br#"{"url":"u","sha256":"x","fetched_at":"2026-10-18T00:03:51Z"}"#,
        ] {
            assert!(with(bad).is_none(), "{}", String::from_utf8_lossy(bad));
        }
        assert!(sidecar(&dir.path().join("none")).is_none());
    }

    #[test]
    fn a_print_leaves_out_the_report_time() {
        let print = |b: &[u8]| Print::new(b).finish().unwrap();
        // AEMO's C rows.
        let aemo = |at: &str| {
            format!(
                "C,NEMP.WORLD,X,AEMO,PUBLIC,2026/10/21,{at},1\r\nI,A,B,1,X\r\nD,A,B,1,5\r\nC,END OF REPORT,3\r\n"
            )
        };
        assert_eq!(
            print(aemo("19:12").as_bytes()),
            print(aemo("19:42").as_bytes())
        );
        assert_eq!(
            print(aemo("19:12").as_bytes()),
            print(b"I,A,B,1,X\nD,A,B,1,5\n")
        );
        assert_ne!(
            print(aemo("19:12").as_bytes()),
            print(aemo("19:12").replace("1,5", "1,6").as_bytes())
        );
        // A current_date column, wherever it is; the last line unended.
        let gas = |at: &str| format!("gas_date,flow,Current_Date\n2026-10-21,5,{at}");
        assert_eq!(
            print(gas("19:12").as_bytes()),
            print(gas("19:42").as_bytes())
        );
        assert_eq!(
            print(gas("x").as_bytes()),
            print(b"gas_date,flow\n2026-10-21,5\n")
        );
        let first = |at: &str| format!("current_date,flow\n{at},5\n");
        assert_eq!(print(first("19:12").as_bytes()), print(b"flow\n5\n"));
        assert_eq!(
            print(first("19:12").as_bytes()),
            print(first("19:42").as_bytes())
        );
        // Neither: every byte counts.
        assert_ne!(print(b"a,b\n1,2\n"), print(b"a,b\n1,3\n"));
        assert_eq!(print(b""), print(b""));
    }

    #[test]
    fn each_version_of_a_file_rewritten_in_place_is_classed() {
        let dir = tempfile::tempdir().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let reports = dir.path().join("Reports");
        let v = |file: &str, at: Option<&str>, bytes: &[u8]| put_version(&reports, file, at, bytes);
        let (t1912, t1942) = (Some("20261021T191200+1000"), Some("20261021T194200+1000"));
        // A gas report: new data, its report time only, then the first
        // version's data again.
        let gas = "int128_v4_actual_linepack_1.csv";
        let row = |flow: u32, made: &str| {
            format!("gas_date,flow,current_date\n2026-10-21,{flow},{made}\n")
        };
        let mut files = vec![
            v(gas, None, row(5, "19:00").as_bytes()),
            v(gas, t1912, row(6, "19:12").as_bytes()),
            v(gas, t1942, row(6, "19:42").as_bytes()),
            v(
                gas,
                Some("20261021T201200+1000"),
                row(5, "20:12").as_bytes(),
            ),
        ];
        // A zip of AEMO CSVs: its report time only, then one that is not
        // a zip, whose data cannot be compared.
        let day = "PUBLIC_DISPATCHSCADA_20261021.zip";
        let inner = |made: &str| {
            let csv = String::from_utf8(three_tables(2))
                .unwrap()
                .replace("00:00:00", made);
            zip_of(&[("PUBLIC_DISPATCHSCADA_202610210005_1.CSV", csv.as_bytes())])
        };
        files.push(v(day, None, &inner("19:00:00")));
        files.push(v(day, t1912, &inner("19:12:00")));
        files.push(v(day, t1942, b"not a zip"));
        // A zip whose entry does not read, and one whose entry is cut
        // short in its compressed bytes.
        let bad = "PUBLIC_BAD_20261021.zip";
        files.push(v(bad, None, &inner("19:00:00")));
        let unread = zip_of(&[("PUBLIC_BAD_202610210005_1.CSV", b"D,A,B,1,x\n")]);
        files.push(v(bad, t1912, &unread));
        let big: Vec<u8> = (0..200_000u32)
            .flat_map(|i| format!("D,A,B,1,{i}\n").into_bytes())
            .collect();
        let mut cut = zip_of(&[("PUBLIC_BAD_202610210010_1.CSV", &big)]);
        cut[2000..4000].fill(0x55);
        files.push(v(bad, t1942, &cut));
        // A CSV that does not read, then a version gone before it was
        // read, with no sidecar; a zip, then a version gone but for its
        // sidecar; a file of no kind, then a version with no sidecar.
        let version = |file: &str| reports.join(format!("{VIC_GAS}/{file}@20261021T191200+1000"));
        let worse = "PUBLIC_WORSE_20261021.CSV";
        files.push(v(worse, None, b"D,A,B,1,x\n"));
        files.push(version(worse));
        let gone = "PUBLIC_GONE_20261021.zip";
        files.push(v(gone, None, &inner("19:00:00")));
        let gone_since = v(gone, t1912, &inner("19:12:00"));
        std::fs::remove_file(&gone_since).unwrap();
        files.push(gone_since);
        let pdf = "notes.pdf";
        files.push(v(pdf, None, b"%PDF"));
        std::fs::write(version(pdf), b"%PDF-2").unwrap();
        files.push(version(pdf));
        // One version only, listed again with nothing new; and one never.
        let lone = "int934_v4_ecgs_contacts_1.csv";
        files.push(v(lone, None, b"a,b\n1,2\n"));
        let quiet = "int111_v5_sdpc_1.csv";
        files.push(v(quiet, None, b"a,b\n1,2\n"));
        files.sort();

        let mirror = dir.path().join("mirror-home");
        let journal = journals(&mirror).join(VIC_GAS).join("changes.ndjson");
        let line = |f: &str, event: &str| {
            format!("{{\"url\":\"{}\",\"event\":\"{event}\"}}\n", vic_gas_url(f))
        };
        let text = [
            line(lone, "same"),
            line(lone, "same"),
            line(day, "repacked"),
            line(gas, "version"),
            "{\"event\":\"same\"}\n".to_owned(),
            "not json\n".to_owned(),
        ]
        .concat();
        put(journal.parent().unwrap(), "changes.ndjson", text.as_bytes());

        let cat = scada();
        let mut ctls = CtlSource::new(None);
        // Without a journal, only the files of more than one version.
        let census = census_folder(&cat, &mut ctls, &files, None, tmp.path());
        let names: Vec<&str> = census.rewritten.iter().map(|w| w.file.as_str()).collect();
        assert_eq!(names, [bad, day, gone, worse, gas, pdf]);
        let census = census_folder(&cat, &mut ctls, &files, Some(&journal), tmp.path());
        let by: HashMap<&str, &Rewritten> = census
            .rewritten
            .iter()
            .map(|w| (w.file.as_str(), w))
            .collect();
        assert_eq!(by.len(), 7, "{:?}", by.keys());
        let changes = |f: &str| by[f].versions.iter().map(|v| v.change).collect::<Vec<_>>();
        use Change::{Data, First, ReportTime, Reverted};
        assert_eq!(changes(gas), [First, Data, ReportTime, Reverted]);
        let g = &by[gas].versions;
        assert_eq!(g[3].same_data_as.as_deref(), Some(gas));
        assert_eq!(g[1].file, format!("{gas}@20261021T191200+1000"));
        assert_eq!(g[1].listed_at.as_deref(), Some("2026-10-21T19:12:00+10:00"));
        assert_eq!(g[0].published_at.as_deref(), Some("2026-10-21T08:00:00Z"));
        assert_eq!(g[0].sha256, crate::ctl::sha256(row(5, "19:00").as_bytes()));
        assert_eq!(changes(day), [First, ReportTime, Data]);
        assert_eq!((by[day].relisted, by[day].repacked), (0, 1));
        assert_eq!(by[day].versions[2].sha256, crate::ctl::sha256(b"not a zip"));
        assert_eq!(changes(bad), [First, Data, Data]);
        assert_eq!(changes(worse), [First, Data]);
        assert_eq!(by[worse].versions[1].sha256, "", "gone before it was read");
        assert_eq!(by[worse].versions[1].listed_at, None, "no sidecar");
        assert_eq!(changes(gone), [First, Data]);
        assert_eq!(by[gone].versions[1].sha256, "x", "its sidecar's");
        assert_eq!(by[pdf].versions[0].sha256, "x", "not read: its sidecar's");
        assert_eq!(changes(pdf), [First, Data]);
        assert_eq!((by[lone].relisted, by[lone].versions.len()), (2, 1));
        // Every version is read as its name's kind: none is another file.
        assert_eq!(census.other, 2, "the PDF's two versions");

        // Recorded, read back, and taken by the census of the roots.
        let repo = dir.path().join("repo");
        record_folder(&repo, "CURRENT", "VicGas", &census).unwrap();
        let text = std::fs::read_to_string(folder_path(&repo, "CURRENT", "VicGas")).unwrap();
        assert!(text.contains("[[rewritten.version]]"), "{text}");
        assert_eq!(
            read_folder(&repo, "CURRENT", "VicGas").unwrap().unwrap(),
            census
        );
        let taken = census_reports(
            &cat,
            &reports,
            (None, Some(&journals(&mirror))),
            &repo,
            tmp.path(),
            2,
        )
        .unwrap();
        // There the versions gone from disk are not listed, and the
        // sidecars beside the rest are files of no kind.
        let there: Vec<&str> = taken[0]
            .census
            .rewritten
            .iter()
            .map(|w| w.file.as_str())
            .collect();
        assert_eq!(there, [bad, day, gas, lone, pdf]);
        assert_eq!(&taken[0].census.rewritten[2], by[gas]);
    }

    #[test]
    fn a_name_gives_its_first_month() {
        let m = |s: &str| name_month(s).map(|m| m.to_string());
        assert_eq!(
            m("PUBLIC_DISPATCHIS_202610200245_0000000543803367.zip").as_deref(),
            Some("2026-10")
        );
        assert_eq!(m("PUBLIC_BILLING_20250902.zip").as_deref(), Some("2025-09"));
        assert_eq!(
            m("FILE01_123_202509.CSV").as_deref(),
            Some("2025-09"),
            "short runs are skipped"
        );
        assert_eq!(m("X_000000123456_201213.CSV"), None, "year 0000, month 13");
        assert_eq!(
            m("X_209913_200001.CSV").as_deref(),
            Some("2000-01"),
            "month 13 is not a month"
        );
        assert_eq!(m("PUBLIC_TER_DAILY.zip"), None);
        assert_eq!(kind("ARCHIVE"), "reports/archive");
        assert_eq!(kind("CURRENT"), "reports/current");
    }

    #[test]
    fn a_folder_is_a_list_of_runs_and_recorded() {
        let dir = tempfile::tempdir().unwrap();
        let cat = scada();
        // The control file of 2026-08 types the SCADA block of 2026-08 and,
        // looking back, of 2026-10; nothing types 2026-07. 2026-09 has a
        // CTL folder of no control file, so it is looked past.
        let mmsdm = dir.path().join("mmsdm");
        put(
            &mmsdm,
            "MMSDM_2026_08/MMSDM_Historical_Data_SQLLoader/CTL/PUBLIC_ARCHIVE#DISPATCH_UNIT_SCADA#FILE01#202608.ctl",
            SCADA_CTL.as_bytes(),
        );
        put(
            &mmsdm,
            "MMSDM_2026_08/MMSDM_Historical_Data_SQLLoader/CTL/._x.ctl",
            b"x",
        );
        put(&mmsdm, "MMSDM_2026_09/CTL/README.txt", b"x");
        let reports = dir.path().join("Reports");
        let folder = "ARCHIVE/Dispatch_SCADA";
        // A day's zip of five-minute zips, each a CSV of three blocks.
        let inner = |stamp: &str, rows: usize| {
            zip_of(&[(
                &format!("PUBLIC_DISPATCHSCADA_{stamp}_0000000001.CSV"),
                &three_tables(rows),
            )])
        };
        // Out of name order: the run's first CSV is the least by name.
        let day = zip_of(&[
            (
                "PUBLIC_DISPATCHSCADA_202608010010.zip",
                &inner("202608010010", 3),
            ),
            (
                "PUBLIC_DISPATCHSCADA_202608010005.zip",
                &inner("202608010005", 2),
            ),
        ]);
        put(
            &reports,
            &format!("{folder}/PUBLIC_DISPATCHSCADA_20260801.zip"),
            &day,
        );
        put(
            &reports,
            &format!("{folder}/PUBLIC_DISPATCHSCADA_202610200005_1.CSV"),
            &three_tables(4),
        );
        put(
            &reports,
            &format!("{folder}/PUBLIC_DISPATCHSCADA_202607310005_1.CSV"),
            &three_tables(1),
        );
        // A value with a second decimal place: a wider scale starts a run.
        let finer = String::from_utf8(three_tables(1))
            .unwrap()
            .replace(",0.5\r", ",0.25\r");
        put(
            &reports,
            &format!("{folder}/PUBLIC_DISPATCHSCADA_202610200010_1.CSV"),
            finer.as_bytes(),
        );
        // A zip named for its month, of a CSV that is not.
        put(
            &reports,
            &format!("{folder}/PUBLIC_DISPATCHSCADA_20261021.zip"),
            &zip_of(&[("PUBLIC_UNDATED.CSV", &three_tables(1))]),
        );
        // No date in its name: no month, so not counted in a run.
        put(
            &reports,
            &format!("{folder}/PUBLIC_UNDATED.CSV"),
            &three_tables(1),
        );
        put(&reports, &format!("{folder}/notes.pdf"), b"%PDF");
        put(
            &reports,
            &format!("{folder}/PUBLIC_BAD_20260801.zip"),
            b"not a zip",
        );
        put(
            &reports,
            &format!("{folder}/PUBLIC_BAD_20260802.CSV"),
            b"D,A,B,1,x\n",
        );
        put(
            &reports,
            &format!("{folder}/PUBLIC_BADENTRY_20260803.zip"),
            &zip_of(&[("PUBLIC_BADENTRY_20260803.CSV", b"D,A,B,1,x\n")]),
        );
        put(
            &reports,
            &format!("{folder}/PUBLIC_NOCSV_20260803.zip"),
            &zip_of(&[("README.txt", b"x")]),
        );
        put(
            &reports,
            "ARCHIVE/GSH/Benchmark_Price/PUBLIC_GSH_20260801.CSV",
            &three_tables(1),
        );
        put(&reports, "CURRENT/._Dispatch_SCADA/x.CSV", &three_tables(1));

        let root_dir = reports.join("ARCHIVE");
        let tops = top_folders(&root_dir);
        assert_eq!(tops.len(), 2);
        assert!(top_folders(&dir.path().join("none")).is_empty());
        let all = folders(&root_dir, &tops[0]).unwrap();
        assert_eq!(all.keys().collect::<Vec<_>>(), ["Dispatch_SCADA"]);
        let gsh = folders(&root_dir, &tops[1]).unwrap();
        assert_eq!(gsh.keys().collect::<Vec<_>>(), ["GSH/Benchmark_Price"]);
        assert!(folders(&root_dir, &root_dir.join("none")).is_err());
        assert!(folder_path(Path::new("s"), "ARCHIVE", "").ends_with("reports/ARCHIVE/_root.toml"));

        let tmp = tempfile::tempdir().unwrap();
        let mut ctls = CtlSource::new(Some(&mmsdm));
        assert!(
            ctls.for_month(YearMonth::new(2026, 7).unwrap())
                .for_table(&["DISPATCH_UNIT_SCADA"])
                .is_none()
        );
        let census = census_folder(&cat, &mut ctls, &all["Dispatch_SCADA"], None, tmp.path());
        assert_eq!((census.files, census.other), (11, 1));
        assert_eq!(census.csvs, 7, "three nested, four loose");
        let unread: Vec<&str> = census.unread.iter().map(|u| u.file.as_str()).collect();
        assert_eq!(
            unread,
            [
                "PUBLIC_BADENTRY_20260803.zip!PUBLIC_BADENTRY_20260803.CSV",
                "PUBLIC_BAD_20260801.zip",
                "PUBLIC_BAD_20260802.CSV",
                ""
            ],
            "{:?}",
            census.unread
        );
        assert_eq!(census.unread[3].error, "1 CSV(s) whose month no name gives");
        let scada: Vec<&Tally> = census
            .blocks
            .iter()
            .filter(|t| t.block.report == ("DISPATCH".into(), "UNIT_SCADA".into()))
            .collect();
        // Runs in name order: July untyped; August's two nested CSVs and
        // October's first typed alike (October looks back to August's
        // control file); October's finer value and the CSV of the zip
        // named for October 21st after it.
        let months: Vec<String> = scada.iter().map(|t| t.month.to_string()).collect();
        assert_eq!(months, ["2026-07", "2026-08", "2026-10"]);
        let aug = scada[1];
        assert_eq!((aug.csvs, aug.block.rows), (3, 9));
        assert_eq!(
            aug.first_csv,
            "PUBLIC_DISPATCHSCADA_202608010005_0000000001.CSV"
        );
        assert_eq!(aug.last_csv, "PUBLIC_DISPATCHSCADA_202610200005_1.CSV");
        assert_eq!(aug.first_sha256.len(), 64, "the run's first file is hashed");
        assert_eq!(aug.last_sha256.len(), 64);
        assert!(!aug.first_file.is_empty() && !aug.last_file.is_empty());
        // The CSVs' blocks are one record: rows summed, values merged.
        assert_eq!(aug.block.nulls, [0, 0, 0]);
        assert_eq!(aug.block.max[2], "3.5");
        assert_eq!(aug.block.first.as_deref(), Some("2026/08/01 00:05:00"));
        assert_eq!(
            scada[2].first_csv,
            "PUBLIC_DISPATCHSCADA_202610200010_1.CSV"
        );
        assert_eq!(scada[2].last_csv, "PUBLIC_UNDATED.CSV");
        assert_eq!(scada[0].block.ctl_of(0), TypeClass::Unknown);
        assert_eq!(aug.block.ctl_of(0), TypeClass::Timestamp);
        // Blocks without a span merge to none.
        let extra: Vec<&Tally> = census
            .blocks
            .iter()
            .filter(|t| t.block.report.1 == "EXTRA")
            .collect();
        assert!(extra.iter().all(|t| t.block.time.is_none()));

        // Recorded, and read back as it was.
        let repo = dir.path().join("repo");
        record_folder(&repo, "ARCHIVE", "Dispatch_SCADA", &census).unwrap();
        assert_eq!(
            read_folder(&repo, "ARCHIVE", "Dispatch_SCADA")
                .unwrap()
                .unwrap(),
            census
        );
        assert_eq!(read_folder(&repo, "ARCHIVE", "None").unwrap(), None);
        std::fs::write(folder_path(&repo, "ARCHIVE", "Broken"), "files = [").unwrap();
        let e = read_folder(&repo, "ARCHIVE", "Broken")
            .unwrap_err()
            .to_string();
        assert!(e.contains("Broken.toml cannot be read"), "{e}");
        std::fs::create_dir_all(folder_path(&repo, "ARCHIVE", "Dir")).unwrap();
        assert!(read_folder(&repo, "ARCHIVE", "Dir").is_err());

        // Every folder of both roots at once, each recorded.
        let taken =
            census_reports(&cat, &reports, (Some(&mmsdm), None), &repo, tmp.path(), 2).unwrap();
        let names: Vec<&str> = taken.iter().map(|t| t.folder.as_str()).collect();
        assert_eq!(
            names,
            [
                "Reports/ARCHIVE/Dispatch_SCADA",
                "Reports/ARCHIVE/GSH/Benchmark_Price"
            ]
        );
        assert_eq!(taken[0].census, census);
        assert!(folder_path(&repo, "ARCHIVE", "GSH/Benchmark_Price").is_file());
        let blocked = dir.path().join("blocked");
        std::fs::write(&blocked, "a file").unwrap();
        assert!(census_reports(&cat, &reports, (None, None), &blocked, tmp.path(), 2).is_err());
        // Without MMSDM months nothing is typed.
        let mut none = CtlSource::new(None);
        assert!(
            none.for_month(YearMonth::new(2026, 8).unwrap())
                .for_table(&["DISPATCH_UNIT_SCADA"])
                .is_none()
        );
    }
}
