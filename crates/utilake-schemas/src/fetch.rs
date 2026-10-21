//! A polite download of one month of the MMSDM archive from nemweb.
//!
//! ```text
//! {base}/Data_Archive/Wholesale_Electricity/MMSDM/{YYYY}/MMSDM_{YYYY}_{MM}/MMSDM_Historical_Data_SQLLoader/
//!     DATA/  P5MIN_ALL_DATA/  PREDISP_ALL_DATA/      per-table zips
//!     CTL/                                           control files
//!     DOCUMENTATION/README.txt
//! {base}/Data_Archive/Wholesale_Electricity/MMSDM/{YYYY}/MMSDM_{YYYY}_{MM}.zip   before 2015: one zip
//! ```
//!
//! Directories are listed rather than URLs guessed, because split tables
//! and renamed files only show up in the listing. Months from 2015 on have
//! the directory; earlier ones, and a month whose directory is missing, are
//! looked up in the year's listing. Files land under the month's directory
//! as nemweb lays it out below `MMSDM_Historical_Data_SQLLoader/`
//! (`DATA/…`, `CTL/…`), or as `MMSDM_{YYYY}_{MM}.zip`.
//!
//! Politeness: one request at a time, at least an interval between the end
//! of one and the start of the next, a User-Agent that names this
//! repository, and a stop at the first 403 or 429. A file is written to
//! `{name}.part` and renamed when complete; one already there with the size
//! the listing gives is not fetched again. Every file is recorded in
//! `sources/{YYYY-MM}.tsv`: its URL, sha256, size and the `Last-Modified`
//! the server sent, sorted by URL.
//!
//! The HTTP client is behind [`Http`], so tests serve pages from memory.

use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use memchr::{memchr, memmem, memrchr};

use crate::month::YearMonth;

/// nemweb.
pub const BASE: &str = "https://www.nemweb.com.au";

/// The User-Agent of every request.
pub const USER_AGENT: &str = concat!(
    "utilake-schemas/",
    env!("CARGO_PKG_VERSION"),
    " (+https://github.com/g0itb4/utilake-schemas)"
);

/// The least time between two requests, by default.
pub const DEFAULT_INTERVAL_MS: u64 = 1000;

/// The first year whose months are published as directories of per-table
/// zips. Earlier months exist only as one whole-month zip.
const FIRST_DIRECTORY_YEAR: i32 = 2015;

/// The sub-directories of a month that hold per-table data zips.
const DATA_DIRS: [&str; 3] = ["DATA/", "P5MIN_ALL_DATA/", "PREDISP_ALL_DATA/"];

/// The sub-directories of a month that hold its control files and README.
const SUPPORT_DIRS: [&str; 2] = ["CTL/", "DOCUMENTATION/"];

/// What a server answered.
pub struct Response {
    /// The HTTP status.
    pub status: u16,
    /// The `Last-Modified` header, as sent.
    pub last_modified: Option<String>,
    /// The body.
    pub body: Box<dyn Read>,
}

/// An HTTP client that GETs one URL.
pub trait Http {
    /// The answer to `GET url`, any status; an error only when there was
    /// none.
    fn get(&mut self, url: &str, user_agent: &str) -> std::io::Result<Response>;
}

/// Why a month could not be fetched.
#[derive(Debug, thiserror::Error)]
pub enum FetchError {
    /// No answer, or the answer broke off.
    #[error("The request for {url} failed.")]
    Http {
        /// The URL.
        url: String,
        /// Cause.
        #[source]
        source: std::io::Error,
    },
    /// The server refused: fetching stops.
    #[error(
        "nemweb answered {url} with HTTP {status}, so fetching stops; try again later with a longer --interval-ms."
    )]
    Refused {
        /// The URL.
        url: String,
        /// 403 or 429.
        status: u16,
    },
    /// Not there.
    #[error("nemweb has no {url} (HTTP 404).")]
    NotFound {
        /// The URL.
        url: String,
    },
    /// Any other status.
    #[error("nemweb answered {url} with HTTP {status}.")]
    Status {
        /// The URL.
        url: String,
        /// The status.
        status: u16,
    },
    /// The month is not in the archive.
    #[error("{month} is not in the archive at {url}; it has {available}.")]
    MonthMissing {
        /// The month.
        month: YearMonth,
        /// The year's listing.
        url: String,
        /// What it has instead.
        available: String,
    },
    /// A listing of no file.
    #[error("The listing {url} links to no file; nemweb may have changed its listing pages.")]
    ListingParse {
        /// The URL.
        url: String,
    },
    /// A file that could not be written.
    #[error("The file {path} could not be written.")]
    Write {
        /// The file.
        path: String,
        /// Cause.
        #[source]
        source: std::io::Error,
    },
    /// A file already fetched that cannot be read.
    #[error("The file {path} cannot be read.")]
    Read {
        /// The file.
        path: String,
        /// Cause.
        #[source]
        source: std::io::Error,
    },
    /// A sources file that cannot be read.
    #[error("The sources file {path} cannot be read: {message}")]
    Sources {
        /// The file.
        path: String,
        /// What is wrong.
        message: String,
    },
}

/// One link of an IIS directory listing, the only kind nemweb serves:
///
/// ```html
/// <pre><A HREF="/x/">[To Parent Directory]</A><br><br>
///  2/13/2024  8:27 AM      1584114 <A HREF="/x/PUBLIC_DVD_DISPATCHPRICE_202401010000.zip">PUBLIC_DVD_DISPATCHPRICE_202401010000.zip</A><br>
///  2/13/2024  8:27 AM        &lt;dir&gt; <A HREF="/x/sub/">sub</A><br>
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listed {
    /// The href as written (usually an absolute path).
    pub href: String,
    /// The link text.
    pub name: String,
    /// Size in bytes when the line showed one.
    pub size: Option<u64>,
    /// Whether the line marked it `<dir>`.
    pub is_dir: bool,
}

/// `b` without ASCII whitespace at either end.
fn trim(b: &[u8]) -> &[u8] {
    let start = b
        .iter()
        .position(|c| !c.is_ascii_whitespace())
        .unwrap_or(b.len());
    let end = b
        .iter()
        .rposition(|c| !c.is_ascii_whitespace())
        .map_or(start, |i| i + 1);
    &b[start..end]
}

/// Every link of a listing with the size or `<dir>` mark before it on its
/// line; the parent link (`[To Parent Directory]`) left out.
pub fn parse_listing(html: &[u8]) -> Vec<Listed> {
    let lower = html.to_ascii_lowercase();
    let finder = memmem::Finder::new(b"href=\"");
    let text = |b: &[u8]| String::from_utf8_lossy(b).into_owned();
    let mut out = Vec::new();
    let mut last_end = 0usize;
    while let Some(at) = finder.find(&lower[last_end..]).map(|i| last_end + i) {
        let href_start = at + 6;
        let Some(href_len) = memchr(b'"', &html[href_start..]) else {
            break;
        };
        let after_tag = href_start + href_len;
        let Some(gt) = memchr(b'>', &html[after_tag..]) else {
            break;
        };
        let text_start = after_tag + gt + 1;
        let text_len = memchr(b'<', &html[text_start..]).unwrap_or(html.len() - text_start);
        let name = trim(&html[text_start..text_start + text_len]);
        // The prefix: the text between the previous anchor's end and this
        // tag's '<'.
        let tag_start = memrchr(b'<', &html[last_end..at]).map_or(at, |i| last_end + i);
        let prefix_start =
            memrchr(b'>', &html[last_end..tag_start]).map_or(last_end, |i| last_end + i + 1);
        let prefix = trim(&html[prefix_start..tag_start]);
        if name.first() != Some(&b'[') {
            let is_dir = prefix.ends_with(b"&lt;dir&gt;");
            let size = prefix
                .iter()
                .rposition(u8::is_ascii_whitespace)
                .map(|i| &prefix[i + 1..])
                .filter(|t| !is_dir && !t.is_empty() && t.iter().all(u8::is_ascii_digit))
                .and_then(|t| text(t).parse().ok());
            out.push(Listed {
                href: text(&html[href_start..after_tag]),
                name: text(name),
                size,
                is_dir,
            });
        }
        last_end = text_start + text_len;
    }
    out
}

/// `%XX` escapes decoded; anything else as written.
fn decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let hex = b
            .get(i + 1..i + 3)
            .filter(|h| b[i] == b'%' && h.iter().all(u8::is_ascii_hexdigit));
        match hex {
            Some(h) => {
                out.push(u8::from_str_radix(&String::from_utf8_lossy(h), 16).unwrap_or(b'%'));
                i += 3;
            }
            None => {
                out.push(b[i]);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// `href` as a URL, from the listing at `dir` (which ends in `/`).
fn join(dir: &str, href: &str) -> String {
    if href.contains("://") {
        return href.to_owned();
    }
    if href.starts_with('/') {
        let host = dir.find("://").map_or(0, |i| i + 3);
        let end = dir[host..].find('/').map_or(dir.len(), |j| host + j);
        return format!("{}{href}", &dir[..end]);
    }
    format!("{dir}{href}")
}

/// One file to fetch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Remote {
    /// Its URL.
    pub url: String,
    /// Where it goes below the month's directory, `/`-separated.
    pub rel: String,
    /// Its size, as listed.
    pub size: Option<u64>,
}

/// One request at a time, an interval apart, stopping at a refusal.
pub struct Polite<'h> {
    http: &'h mut dyn Http,
    interval: Duration,
    last: Option<Instant>,
    /// Requests made.
    pub requests: u64,
}

impl<'h> Polite<'h> {
    /// Requests through `http`, at least `interval` apart.
    pub fn new(http: &'h mut dyn Http, interval: Duration) -> Self {
        Self {
            http,
            interval,
            last: None,
            requests: 0,
        }
    }

    /// `GET url`, its body handed to `read` when the status is 200; the
    /// interval counts from when `read` is done.
    pub fn request<T>(
        &mut self,
        url: &str,
        read: impl FnOnce(Response) -> Result<T, FetchError>,
    ) -> Result<T, FetchError> {
        if let Some(last) = self.last {
            std::thread::sleep(self.interval.saturating_sub(last.elapsed()));
        }
        self.requests += 1;
        let answer = self.http.get(url, USER_AGENT);
        self.last = Some(Instant::now());
        let url = url.to_owned();
        let response = answer.map_err(|source| FetchError::Http {
            url: url.clone(),
            source,
        })?;
        match response.status {
            200 => {}
            403 | 429 => {
                let status = response.status;
                return Err(FetchError::Refused { url, status });
            }
            404 => return Err(FetchError::NotFound { url }),
            status => return Err(FetchError::Status { url, status }),
        }
        let out = read(response);
        self.last = Some(Instant::now());
        out
    }

    /// The links of the listing at `url`.
    pub fn listing(&mut self, url: &str) -> Result<Vec<Listed>, FetchError> {
        self.request(url, |mut r| {
            let mut body = Vec::new();
            let read = r.body.read_to_end(&mut body);
            read.map_err(|source| http_error(url, source))?;
            Ok(parse_listing(&body))
        })
    }
}

fn http_error(url: &str, source: std::io::Error) -> FetchError {
    FetchError::Http {
        url: url.to_owned(),
        source,
    }
}

/// The year directory of the archive.
fn year_root(base: &str, year: i32) -> String {
    format!(
        "{}/Data_Archive/Wholesale_Electricity/MMSDM/{year}/",
        base.trim_end_matches('/')
    )
}

/// The month directory of the archive (2015 on).
pub fn month_root(base: &str, month: YearMonth) -> String {
    format!(
        "{}MMSDM_{}_{:02}/MMSDM_Historical_Data_SQLLoader/",
        year_root(base, month.year()),
        month.year(),
        month.month()
    )
}

/// A listed file to fetch into `dir` (a prefix of `rel`), when its URL is
/// on `base` and its name is a plain file name.
fn remote(base: &str, listing: &str, dir: &str, e: &Listed) -> Option<Remote> {
    let url = join(listing, &e.href);
    let name = decode(url.rsplit('/').next().unwrap_or_default());
    let plain = !name.is_empty() && !name.starts_with('.') && !name.contains(['/', '\\']);
    let ours = url
        .strip_prefix(base.trim_end_matches('/'))
        .is_some_and(|r| r.starts_with('/'));
    (plain && ours).then(|| Remote {
        url,
        rel: format!("{dir}{name}"),
        size: e.size,
    })
}

/// The files of a month published as a directory.
fn directory_files(
    p: &mut Polite<'_>,
    base: &str,
    month: YearMonth,
) -> Result<Vec<Remote>, FetchError> {
    let root = month_root(base, month);
    let mut out = Vec::new();
    for (i, d) in DATA_DIRS.iter().enumerate() {
        let url = format!("{root}{d}");
        let entries = match p.listing(&url) {
            Ok(e) => e,
            // The big-table directories do not exist for every month.
            Err(FetchError::NotFound { .. }) if i > 0 => continue,
            Err(e) => return Err(e),
        };
        if entries.is_empty() && i == 0 {
            return Err(FetchError::ListingParse { url });
        }
        let zips = entries
            .iter()
            .filter(|e| crate::zip::has_extension(&e.name, ".zip"));
        out.extend(zips.filter_map(|e| remote(base, &url, d, e)));
    }
    for d in SUPPORT_DIRS {
        let url = format!("{root}{d}");
        let entries = match p.listing(&url) {
            Ok(e) => e,
            Err(FetchError::NotFound { .. }) => continue,
            Err(e) => return Err(e),
        };
        let wanted = |e: &&Listed| {
            !e.is_dir
                && (crate::zip::has_extension(&e.name, ".ctl")
                    || e.name.eq_ignore_ascii_case("readme.txt"))
        };
        out.extend(
            entries
                .iter()
                .filter(wanted)
                .filter_map(|e| remote(base, &url, d, e)),
        );
    }
    Ok(out)
}

/// Every file of `month`: from its directory (2015 on), else as its year's
/// listing says the month is published. Sorted by where it goes.
pub fn month_files(
    p: &mut Polite<'_>,
    base: &str,
    month: YearMonth,
) -> Result<Vec<Remote>, FetchError> {
    let listed = (month.year() >= FIRST_DIRECTORY_YEAR).then(|| directory_files(p, base, month));
    let mut files = match listed {
        None | Some(Err(FetchError::NotFound { .. })) => by_year(p, base, month)?,
        Some(other) => other?,
    };
    files.sort_by(|a, b| a.rel.cmp(&b.rel));
    Ok(files)
}

/// The files of `month` as its year's listing has it: a directory, or one
/// zip of the whole month.
fn by_year(p: &mut Polite<'_>, base: &str, month: YearMonth) -> Result<Vec<Remote>, FetchError> {
    let url = year_root(base, month.year());
    let missing = |available: &str| FetchError::MonthMissing {
        month,
        url: url.clone(),
        available: available.to_owned(),
    };
    let entries = match p.listing(&url) {
        Ok(e) => e,
        Err(FetchError::NotFound { .. }) => return Err(missing("no such year")),
        Err(e) => return Err(e),
    };
    let dir_name = format!("MMSDM_{}_{:02}", month.year(), month.month());
    let zip_name = format!("{dir_name}.zip");
    let named = |n: &str| entries.iter().find(|e| e.name.eq_ignore_ascii_case(n));
    if named(&dir_name).is_some() {
        return directory_files(p, base, month);
    }
    if let Some(e) = named(&zip_name).and_then(|e| remote(base, &url, "", e)) {
        return Ok(vec![e]);
    }
    let mut months: Vec<&str> = entries
        .iter()
        .filter(|e| e.name.starts_with("MMSDM_"))
        .map(|e| e.name.trim_end_matches(".zip"))
        .collect();
    months.sort_unstable();
    months.dedup();
    Err(missing(&if months.is_empty() {
        "no months".to_owned()
    } else {
        months.join(", ")
    }))
}

/// One fetched file, as `sources/{YYYY-MM}.tsv` records it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Source {
    /// Its sha256.
    pub sha256: String,
    /// Its size in bytes.
    pub size: u64,
    /// The `Last-Modified` the server sent, as sent.
    pub last_modified: Option<String>,
}

/// The sources file of `month`.
pub fn sources_path(repo: &Path, month: YearMonth) -> PathBuf {
    repo.join("sources").join(format!("{month}.tsv"))
}

/// The header line of a sources file.
const SOURCES_HEADER: &str = "url\tsha256\tsize\tlast_modified";

/// A sources file, by URL; empty when there is none.
pub fn read_sources(path: &Path) -> Result<BTreeMap<String, Source>, FetchError> {
    let bad = |message: String| FetchError::Sources {
        path: path.display().to_string(),
        message,
    };
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
        Err(e) => return Err(bad(e.to_string())),
    };
    let mut out = BTreeMap::new();
    for (n, line) in text.lines().enumerate().skip(1) {
        let f: Vec<&str> = line.split('\t').collect();
        let size = f.get(2).and_then(|s| s.parse().ok());
        let (Some(size), [url, sha256, _, last]) = (size, f.as_slice()) else {
            return Err(bad(format!(
                "line {} is not a URL, sha256, size and Last-Modified separated by tabs",
                n + 1
            )));
        };
        let last_modified = (!last.is_empty()).then(|| (*last).to_owned());
        let sha256 = (*sha256).to_owned();
        let s = Source {
            sha256,
            size,
            last_modified,
        };
        out.insert((*url).to_owned(), s);
    }
    Ok(out)
}

/// Writes a sources file whole, sorted by URL.
pub fn write_sources(path: &Path, sources: &BTreeMap<String, Source>) -> Result<(), FetchError> {
    use std::fmt::Write as _;
    let mut text = format!("{SOURCES_HEADER}\n");
    for (url, s) in sources {
        let last = s.last_modified.as_deref().unwrap_or_default();
        let _ = writeln!(text, "{url}\t{}\t{}\t{last}", s.sha256, s.size);
    }
    crate::files::write_atomic(path, text.as_bytes()).map_err(|e| write_error(path, e))
}

fn write_error(path: &Path, source: std::io::Error) -> FetchError {
    FetchError::Write {
        path: path.display().to_string(),
        source,
    }
}

/// A reader that hashes and counts what it reads.
struct Tee<R> {
    inner: R,
    hash: sha2::Sha256,
    len: u64,
}

impl<R: Read> Read for Tee<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        use sha2::Digest as _;
        let n = self.inner.read(buf)?;
        self.hash.update(&buf[..n]);
        self.len += n as u64;
        Ok(n)
    }
}

/// Downloads `url` to `path` through `{path}.part`.
fn download(p: &mut Polite<'_>, url: &str, path: &Path) -> Result<Source, FetchError> {
    use sha2::Digest as _;
    let mut part = path.as_os_str().to_owned();
    part.push(".part");
    let part = PathBuf::from(part);
    p.request(url, |r| {
        let dir = part.parent().unwrap_or_else(|| Path::new("."));
        std::fs::create_dir_all(dir).map_err(|e| write_error(&part, e))?;
        let mut out = std::fs::File::create(&part).map_err(|e| write_error(&part, e))?;
        let mut tee = Tee {
            inner: r.body,
            hash: sha2::Sha256::new(),
            len: 0,
        };
        std::io::copy(&mut tee, &mut out).map_err(|e| http_error(url, e))?;
        out.sync_all().map_err(|e| write_error(&part, e))?;
        std::fs::rename(&part, path).map_err(|e| write_error(path, e))?;
        Ok(Source {
            sha256: crate::files::hex(&tee.hash.finalize()),
            size: tee.len,
            last_modified: r.last_modified,
        })
    })
}

/// What [`fetch_month`] did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Fetched {
    /// Files the listings gave.
    pub listed: usize,
    /// Files downloaded.
    pub fetched: usize,
    /// Files already there.
    pub skipped: usize,
    /// Bytes downloaded.
    pub bytes: u64,
    /// Requests made, listings included.
    pub requests: u64,
}

/// Fetches every file of `month` from `base` into `dir`, at least
/// `interval` between requests, and records each in its sources file under
/// `repo` as it lands.
pub fn fetch_month(
    http: &mut dyn Http,
    base: &str,
    month: YearMonth,
    dir: &Path,
    repo: &Path,
    interval: Duration,
) -> Result<Fetched, FetchError> {
    let mut p = Polite::new(http, interval);
    let files = month_files(&mut p, base, month)?;
    let tsv = sources_path(repo, month);
    let mut sources = read_sources(&tsv)?;
    let mut done = Fetched {
        listed: files.len(),
        ..Fetched::default()
    };
    for f in &files {
        let path = dir.join(&f.rel);
        let len = std::fs::metadata(&path).map(|m| m.len()).ok();
        if f.size.is_some() && len == f.size {
            done.skipped += 1;
            if sources.contains_key(&f.url) {
                continue;
            }
            let read = std::fs::File::open(&path).and_then(crate::files::sha256_of);
            let (sha256, size) = read.map_err(|source| FetchError::Read {
                path: path.display().to_string(),
                source,
            })?;
            let s = Source {
                sha256,
                size,
                last_modified: None,
            };
            sources.insert(f.url.clone(), s);
        } else {
            let s = download(&mut p, &f.url, &path)?;
            done.fetched += 1;
            done.bytes += s.size;
            sources.insert(f.url.clone(), s);
        }
        write_sources(&tsv, &sources)?;
    }
    done.requests = p.requests;
    Ok(done)
}

#[cfg(test)]
mod tests;
