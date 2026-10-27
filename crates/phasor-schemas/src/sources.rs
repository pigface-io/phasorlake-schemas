//! `sources/{YYYY-MM}.tsv` from a mirror archive: every file the evidence
//! was taken from, when the census read a local copy of nemweb rather than
//! what `fetch` downloaded.
//!
//! The archive holds nemweb as it is laid out under its host
//! (`{archive}/www.nemweb.com.au/Data_Archive/…`, `…/Reports/…`), with a
//! sidecar `{file}.json` beside every file that gives its URL, SHA-256,
//! size and the `Last-Modified` the server sent. Each file a census records
//! becomes one row of its month's sources file, in the format
//! [`write_sources`] writes:
//!
//! - an MMSDM month's files are found by name under the month's directory
//!   of the archive (`MMSDM/{YYYY}/MMSDM_{YYYY}_{MM}/`, at any depth) or as
//!   its whole-month zip beside it, indexed once per month;
//! - a report folder's files are the first and last file of each run and
//!   every version of a file rewritten in place, under the folder; each
//!   goes in the month its name gives, else the month the server says it
//!   was published (else fetched).
//!
//! The SHA-256 comes from the census (and the size, where the census
//! records one), and must be the sidecar's; the URL and the publication
//! time come from the sidecar. A file without a sidecar, or whose sidecar
//! disagrees, is a [`Mismatch`], and every one is found before any file is
//! written. The publication time is the `Last-Modified` as sent, else the
//! instant the sidecar keeps; nothing else this tool writes holds it.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use crate::census::{CensusError, base, files_under, read_month};
use crate::fetch::{FetchError, Source, sources_path, sources_text, write_sources};
use crate::fold::{recorded_folders, recorded_months};
use crate::month::YearMonth;
use crate::reports::{Feed, feeds_of, published_at, read_folder};
use phasor_iceberg::Market;
use phasor_provenance::Archive;

/// Why `sources/` could not be made.
#[derive(Debug, thiserror::Error)]
pub enum SourcesError {
    /// A census that cannot be read.
    #[error(transparent)]
    Census(#[from] CensusError),
    /// A sources file that cannot be written.
    #[error(transparent)]
    Write(#[from] FetchError),
}

/// A file a census records that the archive does not vouch for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mismatch {
    /// The file: `{YYYY-MM}/{name}`, or `Reports/{ROOT}/{folder}/{name}`.
    pub file: String,
    /// What is wrong.
    pub problem: String,
}

impl std::fmt::Display for Mismatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.file, self.problem)
    }
}

/// Every sources file, as it would be written, and every mismatch found.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Sources {
    /// Per month, its files by URL.
    pub months: BTreeMap<YearMonth, BTreeMap<String, Source>>,
    /// Every file that could not be listed, in census order.
    pub mismatches: Vec<Mismatch>,
}

impl Sources {
    /// The files listed, over every month.
    #[must_use]
    pub fn files(&self) -> usize {
        self.months.values().map(BTreeMap::len).sum()
    }
}

/// The directory of the archive's host `host`: `archive` itself when it
/// is that directory, else `{archive}/{host}`.
#[must_use]
pub fn host_dir(archive: &Path, host: &str) -> PathBuf {
    let under = archive.join(host);
    if under.is_dir() {
        under
    } else {
        archive.to_path_buf()
    }
}

/// A file found under the archive: its URL, its row, and the month the
/// sidecar gives it (when it was published, else fetched).
type Found = (String, Source, Option<YearMonth>);

/// The rows a census gives, each with the month it goes in, and what did
/// not agree.
type Listed = Result<(Vec<(YearMonth, String, Source)>, Vec<Mismatch>), CensusError>;

fn mismatch(file: &str, problem: impl Into<String>) -> Mismatch {
    Mismatch {
        file: file.to_owned(),
        problem: problem.into(),
    }
}

/// The month an RFC 3339 time is written in.
fn month_of(at: &str) -> Option<YearMonth> {
    at.get(..7)?.parse().ok()
}

/// The names each folder holds, read once.
type Folders = HashMap<PathBuf, std::collections::HashSet<String>>;

/// The row of the file at `path`, named `file`, held to its sidecar: the
/// census's `sha256`, and `size` when the census records one.
fn vouch(
    file: &str,
    path: &Path,
    (sha256, size): (&str, Option<u64>),
    folders: &mut Folders,
) -> Result<Found, Mismatch> {
    // The record found among its folder's names, read once per folder: on
    // exFAT a look for a name not there reads the whole folder.
    let dir = path.parent().unwrap_or(Path::new("."));
    let names = folders.entry(dir.to_path_buf()).or_insert_with(|| {
        std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect()
    });
    let Some(record) = Archive::record_where(path, |n| names.contains(n)) else {
        return Err(mismatch(file, "the archive has no sidecar for it"));
    };
    let Ok(sc) = Archive::read_record(&record) else {
        return Err(mismatch(file, "its sidecar does not read"));
    };
    let hex = sc.content.sha256.hex();
    if hex != sha256 {
        let problem = format!("its sidecar has SHA-256 {hex}, the census {sha256}");
        return Err(mismatch(file, problem));
    }
    if let Some(size) = size
        && sc.content.size != size
    {
        let problem = format!(
            "its sidecar has size {}, the census {size}",
            sc.content.size
        );
        return Err(mismatch(file, problem));
    }
    let published = published_at(&sc);
    let month = month_of(&published.unwrap_or_else(|| sc.fetched_at()).to_string());
    let source = Source {
        sha256: sha256.to_owned(),
        size: sc.content.size,
        last_modified: published.map(phasor_provenance::Stamp::http_date),
        archived_in: sc.archived_in().map(|a| a.url.clone()),
    };
    Ok((sc.url().to_owned(), source, month))
}

/// Every file of `month` under the archive's host, by name: the month's
/// directory at any depth and its whole-month zip. Sidecars are left out.
fn month_index(host: &Path, month: YearMonth) -> HashMap<String, Vec<PathBuf>> {
    let year = host
        .join("Data_Archive/Wholesale_Electricity/MMSDM")
        .join(format!("{:04}", month.year()));
    let stem = format!("MMSDM_{:04}_{:02}", month.year(), month.month());
    let mut files = files_under(&year.join(&stem)).unwrap_or_default();
    files.push(year.join(format!("{stem}.zip")));
    let mut index: HashMap<String, Vec<PathBuf>> = HashMap::new();
    for p in files.into_iter().filter(|p| p.is_file()) {
        let name = base(&p.to_string_lossy()).to_owned();
        if !name.ends_with(".json") {
            index.entry(name).or_default().push(p);
        }
    }
    index
}

/// The files of `want` (name to SHA-256 and size) put to their sidecars;
/// `label` names one, `paths` gives where it may be.
fn vouch_all(
    want: BTreeMap<String, (String, Option<u64>)>,
    label: impl Fn(&str) -> String,
    paths: impl Fn(&str) -> Vec<PathBuf>,
) -> (Vec<(String, Found)>, Vec<Mismatch>) {
    let (mut found, mut bad) = (Vec::new(), Vec::new());
    let mut folders = Folders::new();
    for (name, (sha256, size)) in want {
        let file = label(&name);
        // The first place whose sidecar agrees; else what the first said.
        let mut first = None;
        let mut hit = None;
        for p in paths(&name) {
            match vouch(&file, &p, (&sha256, size), &mut folders) {
                Ok(f) => {
                    hit = Some(f);
                    break;
                }
                Err(m) => {
                    first.get_or_insert(m);
                }
            }
        }
        match (hit, first) {
            (Some(f), _) => found.push((name, f)),
            (None, Some(m)) => bad.push(m),
            (None, None) => bad.push(mismatch(&file, "the archive does not have it")),
        }
    }
    (found, bad)
}

/// Adds `name` with its SHA-256 and size to `want`; a name the census
/// gives two of is a mismatch.
fn want_file(
    want: &mut BTreeMap<String, (String, Option<u64>)>,
    bad: &mut Vec<Mismatch>,
    (name, sha256, size): (&str, &str, Option<u64>),
    label: impl Fn(&str) -> String,
) {
    let entry = want
        .entry(name.to_owned())
        .or_insert_with(|| (sha256.to_owned(), size));
    if *entry != (sha256.to_owned(), size) {
        bad.push(mismatch(
            &label(name),
            "the census gives it two SHA-256s or sizes",
        ));
    }
}

/// The rows of one MMSDM month's census.
fn month_rows((root, market): (&Path, Market), host: &Path, month: YearMonth) -> Listed {
    let label = |name: &str| format!("{month}/{name}");
    let mut want = BTreeMap::new();
    let mut bad = Vec::new();
    for f in read_month(root, market, month)?
        .unwrap_or_default()
        .values()
    {
        let file = (f.source.as_str(), f.sha256.as_str(), Some(f.size));
        want_file(&mut want, &mut bad, file, label);
    }
    let index = month_index(host, month);
    let paths = |name: &str| index.get(name).cloned().unwrap_or_default();
    let (found, more) = vouch_all(want, label, paths);
    bad.extend(more);
    let rows = found
        .into_iter()
        .map(|(_, (url, s, _))| (month, url, s))
        .collect();
    Ok((rows, bad))
}

/// The rows of one folder's census, each with its month.
fn folder_rows(
    (root, market): (&Path, Market),
    archive: &Path,
    (feed, feed_root, rel): (&Feed, &str, &str),
) -> Listed {
    let host = host_dir(archive, feed.host);
    let folder = if rel.is_empty() {
        feed_root.to_owned()
    } else {
        format!("{feed_root}/{rel}")
    };
    let label = |name: &str| format!("{folder}/{name}");
    let census = read_folder(root, market, (feed, rel))?.unwrap_or_default();
    let mut named: Vec<(&str, &str)> = Vec::new();
    for t in &census.blocks {
        named.push((&t.first_file, &t.first_sha256));
        named.push((&t.last_file, &t.last_sha256));
    }
    for v in census.rewritten.iter().flat_map(|r| &r.versions) {
        named.push((&v.file, &v.sha256));
    }
    let mut want = BTreeMap::new();
    let mut bad = Vec::new();
    // Records from before the files were hashed name none.
    for (name, sha256) in named
        .into_iter()
        .filter(|(n, s)| !n.is_empty() && !s.is_empty())
    {
        want_file(&mut want, &mut bad, (name, sha256, None), label);
    }
    let dir = host.join(&folder);
    let (found, more) = vouch_all(want, label, |name| vec![dir.join(name)]);
    bad.extend(more);
    // A sidecar that reads always gives a month: its fetch's at least.
    let rows = found
        .into_iter()
        .filter_map(|(name, (url, s, at))| Some((feed.release_month(&name).or(at)?, url, s)))
        .collect();
    Ok((rows, bad))
}

/// Every sources file the censuses under `root` give, from the archive at
/// `archive` (see [`host_dir`]), months and folders read on up to `jobs`
/// threads.
pub fn collect(
    root: &Path,
    market: Market,
    archive: &Path,
    jobs: usize,
) -> Result<Sources, SourcesError> {
    let host = host_dir(archive, crate::reports::nem_reports().host);
    let mut out = Sources::default();
    let months = recorded_months(root, market)?;
    // Every month has a sources file, if only of its header.
    for &m in &months {
        out.months.entry(m).or_default();
    }
    let mut done = crate::pool::map(jobs, &months, |_, &m| month_rows((root, market), &host, m));
    let mut folders = Vec::new();
    for feed in feeds_of(market.id()) {
        let feed_root = feed.local_roots().into_iter().next().unwrap_or_default();
        for rel in recorded_folders(root, market, feed)? {
            folders.push((feed, feed_root.clone(), rel));
        }
    }
    done.extend(crate::pool::map(jobs, &folders, |_, (f, r, rel)| {
        folder_rows((root, market), archive, (f, r.as_str(), rel.as_str()))
    }));
    for r in done {
        let (rows, bad) = r?;
        for (month, url, s) in rows {
            out.months.entry(month).or_default().insert(url, s);
        }
        out.mismatches.extend(bad);
    }
    Ok(out)
}

/// The sources files under `root` that writing `s` would change.
#[must_use]
pub fn stale(root: &Path, market: Market, s: &Sources) -> Vec<PathBuf> {
    s.months
        .iter()
        .map(|(&m, rows)| (sources_path(root, market, m), sources_text(rows)))
        .filter(|(p, text)| std::fs::read_to_string(p).ok().as_deref() != Some(text.as_str()))
        .map(|(p, _)| p)
        .collect()
}

/// Writes every sources file of `s` that would change; returns how many.
pub fn write(root: &Path, market: Market, s: &Sources) -> Result<usize, SourcesError> {
    let stale = stale(root, market, s);
    for (&m, rows) in &s.months {
        let path = sources_path(root, market, m);
        if stale.contains(&path) {
            write_sources(&path, rows)?;
        }
    }
    Ok(stale.len())
}

#[cfg(test)]
mod tests;
