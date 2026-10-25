//! Walking a zip, including zips inside zips, and handing each wanted
//! entry (every `.CSV`, by default) to a callback as a streaming reader.
//!
//! The walk is in archive order. An entry that cannot be read, or a nested
//! zip that cannot be opened or is nested too deep, is left behind in
//! [`Walked::failed`] and the walk goes on; only an archive that cannot be
//! opened at all is an error, and so is the callback's own. An entry whose
//! local header disagrees with the central directory (AEMO's monthly
//! archives have such) is read from where the directory says, and refused
//! unless it inflates to the directory's size and CRC-32.
//!
//! [`zipdir`] reads a zip's central directory alone, from byte spans the
//! caller fetches (ranged requests, or a file on disk), so what a zip holds
//! is known before, or without, downloading the rest of it.

/// Opens a zone of `zonetrace` until the end of the block, with the
/// feature `zones`; nothing without it.
macro_rules! zone {
    ($z:ident) => {
        #[cfg(feature = "zones")]
        let _zone = zonetrace::enter(&crate::zones::$z);
    };
}

/// Counts bytes `read` from the source or `copied` between buffers in the
/// open zone, with the feature `zones`; nothing without it.
macro_rules! count {
    ($what:ident, $n:expr) => {
        #[cfg(feature = "zones")]
        zonetrace::$what($n as u64);
    };
}

pub mod zipdir;
#[cfg(feature = "zones")]
pub mod zones;

use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write as _};
use std::path::Path;

/// Zips nested deeper than this are refused.
pub const MAX_DEPTH: u8 = 2;

/// Why an archive could not be walked.
#[derive(Debug, thiserror::Error)]
pub enum ArchiveError {
    /// The file could not be opened as a zip.
    #[error("The archive {path} could not be opened: {message}")]
    Open {
        /// The file.
        path: String,
        /// Cause.
        message: String,
    },
    /// An entry could not be read.
    #[error("Entry {entry} of {path} could not be read: {message}")]
    Entry {
        /// The file.
        path: String,
        /// The entry.
        entry: String,
        /// Cause.
        message: String,
    },
    /// Nested too deep.
    #[error("Entry {entry} of {path} is a zip nested {depth} deep; the limit is {MAX_DEPTH}.")]
    TooDeep {
        /// The file.
        path: String,
        /// The entry.
        entry: String,
        /// Depth reached.
        depth: u8,
    },
    /// No wanted entry, at any depth.
    #[error("The archive {path} has no {wanted} entry at any depth.")]
    NoEntry {
        /// The archive.
        path: String,
        /// What was looked for, e.g. `.CSV`.
        wanted: String,
    },
    /// Spooling a nested zip to disk failed.
    #[error("Spooling nested entry {entry} of {path} to {tmp} failed.")]
    Spool {
        /// The file.
        path: String,
        /// The entry.
        entry: String,
        /// Temp path.
        tmp: String,
        /// Cause.
        #[source]
        source: std::io::Error,
    },
}

/// What the walker found at one entry.
pub struct Entry<'a> {
    /// Entry path, nested ones joined with `!` (`inner.zip!FILE.CSV`).
    pub name: String,
    /// Uncompressed size when the zip records it.
    pub size: Option<u64>,
    /// The streaming reader of the entry's bytes.
    pub reader: &'a mut dyn Read,
}

/// Which entries a walk hands to `on_entry`: those whose name `accepts`,
/// named for the message when there is none.
#[derive(Clone, Copy)]
pub struct Accepts<'a> {
    /// Whether an entry name (nested names joined with `!`) is wanted.
    pub accepts: &'a dyn Fn(&str) -> bool,
    /// What is wanted, for the message when nothing is: `.CSV`.
    pub wanted: &'a str,
}

/// Every `.CSV` entry.
pub const CSV: Accepts<'static> = Accepts {
    accepts: &|name| has_extension(name, ".csv"),
    wanted: ".CSV",
};

/// Whether `name` ends in `ext`, in any case.
pub fn has_extension(name: &str, ext: &str) -> bool {
    name.len() >= ext.len() && name[name.len() - ext.len()..].eq_ignore_ascii_case(ext)
}

/// Walks every entry of the zip at `path` that `accepts` wants, nested
/// zips included, in archive order. `on_entry` may return an error to
/// stop the walk. An entry that cannot be read, or a nested zip that
/// cannot be opened or is nested too deep, is left behind in
/// [`Walked::failed`] and the walk goes on; only an archive that cannot be
/// opened at all is an error.
///
/// A nested zip is walked in memory up to 64 MiB, else from a temporary
/// file in `tmp_dir`, removed once read: the zip format needs a seekable
/// central directory. Every other entry (a control file, a README) goes to
/// `on_other` with its reader.
pub fn walk_entries<E>(
    path: &Path,
    tmp_dir: &Path,
    accepts: Accepts<'_>,
    on_entry: &mut dyn FnMut(Entry<'_>) -> Result<(), E>,
    on_other: &mut dyn FnMut(Entry<'_>),
) -> Result<Walked, WalkError<E>> {
    walk(
        path,
        tmp_dir,
        NESTED_IN_MEMORY,
        0,
        "",
        accepts,
        on_entry,
        on_other,
    )
}

/// [`walk_entries`] over every `.CSV` entry.
pub fn walk_csv_entries<E>(
    path: &Path,
    tmp_dir: &Path,
    on_entry: &mut dyn FnMut(Entry<'_>) -> Result<(), E>,
    on_other: &mut dyn FnMut(Entry<'_>),
) -> Result<Walked, WalkError<E>> {
    walk_entries(path, tmp_dir, CSV, on_entry, on_other)
}

/// [`walk_entries`] over a zip already open as `reader` (read in large
/// chunks here), named `label` in errors.
pub fn walk_entries_in<R: Read + Seek, E>(
    reader: R,
    label: &str,
    tmp_dir: &Path,
    accepts: Accepts<'_>,
    on_entry: &mut dyn FnMut(Entry<'_>) -> Result<(), E>,
    on_other: &mut dyn FnMut(Entry<'_>),
) -> Result<Walked, WalkError<E>> {
    zone!(WALK);
    let archive =
        open_archive(Chunked::new(reader)).map_err(|e| open_error(label, e.to_string()))?;
    walk_zip(
        archive,
        label,
        tmp_dir,
        NESTED_IN_MEMORY,
        0,
        "",
        accepts,
        on_entry,
        on_other,
    )
}

/// [`walk_entries_in`] over every `.CSV` entry.
pub fn walk_csv_entries_in<R: Read + Seek, E>(
    reader: R,
    label: &str,
    tmp_dir: &Path,
    on_entry: &mut dyn FnMut(Entry<'_>) -> Result<(), E>,
    on_other: &mut dyn FnMut(Entry<'_>),
) -> Result<Walked, WalkError<E>> {
    walk_entries_in(reader, label, tmp_dir, CSV, on_entry, on_other)
}

/// What a walk found: how many `.CSV` entries it handed on, and the
/// entries it could not read, each left behind while the walk went on.
#[derive(Debug, Default)]
pub struct Walked {
    /// Wanted entries handed to `on_entry`.
    pub entries: usize,
    /// Entries that could not be read, nested zips among them.
    pub failed: Vec<ArchiveError>,
}

impl Walked {
    /// Takes in what one entry gave: its count, or the error it is left
    /// behind with. A callback's error ends the walk.
    fn take<E>(&mut self, r: Result<Self, WalkError<E>>) -> Result<(), WalkError<E>> {
        match r {
            Ok(w) => {
                self.entries += w.entries;
                self.failed.extend(w.failed);
                Ok(())
            }
            Err(WalkError::Archive(a)) => {
                self.failed.push(a);
                Ok(())
            }
            Err(e) => Err(e),
        }
    }
}

/// Either the archive failed or the callback did.
#[derive(Debug, thiserror::Error)]
pub enum WalkError<E> {
    /// The archive.
    #[error(transparent)]
    Archive(ArchiveError),
    /// The callback.
    #[error("The callback for an entry of the archive failed.")]
    Callback(E),
}

/// The chunk a zip on disk is read in. The zip reader asks for its
/// compressed bytes 8 KiB at a time, and scans for its directory in 1 KiB
/// windows, seeking before each; a chunk this size brings many of either
/// in one system call, and keeps each visit of a spinning disk's head a
/// long sequential read while several files are read at once.
const ZIP_READ_BUFFER: usize = 8 << 20;

/// The smallest chunk: a zip on disk is read in an eighth of its size, at
/// least this and at most `ZIP_READ_BUFFER`, so a small zip does not cost
/// a whole chunk of memory and a big one is read in long runs.
const ZIP_READ_MIN: usize = 256 << 10;

/// A reader of `R` in aligned chunks of one size: a seek costs nothing,
/// and a read of any length within a chunk reads that chunk from `R` once.
///
/// A `BufReader` would not do: its buffer is dropped by every seek and
/// refilled whole by the next read, so the zip reader's scan for the
/// directory (a seek and a 1 KiB read per window, from the start of the
/// file to the directory at its end) reads the file over thousands of
/// times, which a volume whose every read is dear (exFAT under FSKit)
/// turns into hours per file.
struct Chunked<R> {
    inner: R,
    chunk: usize,
    /// Whether `chunk` is fitted to the file's size on the first load.
    fit: bool,
    /// Where the next read is from.
    pos: u64,
    /// The offset of `buf` in `inner`.
    start: u64,
    buf: Vec<u8>,
    /// The length of `inner`, once a seek from the end asked for it.
    len: Option<u64>,
}

impl<R: Read + Seek> Chunked<R> {
    fn new(inner: R) -> Self {
        Self {
            fit: true,
            ..Self::with_chunk(inner, ZIP_READ_BUFFER)
        }
    }

    fn with_chunk(inner: R, chunk: usize) -> Self {
        Self {
            inner,
            chunk,
            fit: false,
            pos: 0,
            start: 0,
            buf: Vec::new(),
            len: None,
        }
    }

    /// The length of `inner`, asked for once.
    fn inner_len(&mut self) -> std::io::Result<u64> {
        match self.len {
            Some(len) => Ok(len),
            None => {
                let len = self.inner.seek(SeekFrom::End(0))?;
                self.len = Some(len);
                Ok(len)
            }
        }
    }

    /// Loads the chunk holding `pos`, short at the end of `inner`. The
    /// buffer is no longer than what is left of `inner`, so a small file
    /// costs its own size, not a whole chunk.
    fn load(&mut self) -> std::io::Result<()> {
        let len = self.inner_len()?;
        if self.fit {
            self.fit = false;
            let eighth = usize::try_from(len / 8).unwrap_or(usize::MAX);
            self.chunk = eighth.clamp(ZIP_READ_MIN, ZIP_READ_BUFFER);
        }
        let start = self.pos - self.pos % self.chunk as u64;
        let left = len.saturating_sub(start);
        let want = usize::try_from(left).map_or(self.chunk, |n| n.min(self.chunk));
        zone!(CHUNK_LOAD);
        self.inner.seek(SeekFrom::Start(start))?;
        self.buf.clear();
        self.buf.shrink_to(want);
        self.buf.reserve_exact(want);
        // Short when the file is cut short under the reader.
        (&mut self.inner)
            .take(want as u64)
            .read_to_end(&mut self.buf)?;
        count!(read, self.buf.len());
        self.start = start;
        Ok(())
    }
}

impl<R: Read + Seek> Read for Chunked<R> {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        if out.is_empty() {
            return Ok(0);
        }
        let end = self.start + self.buf.len() as u64;
        if self.buf.is_empty() || self.pos < self.start || self.pos >= end {
            self.load()?;
        }
        let at = usize::try_from(self.pos - self.start).unwrap_or(usize::MAX);
        let have = self.buf.get(at..).unwrap_or(&[]);
        let n = have.len().min(out.len());
        zone!(CHUNK_COPY);
        count!(copied, n);
        out[..n].copy_from_slice(&have[..n]);
        self.pos += n as u64;
        Ok(n)
    }
}

impl<R: Read + Seek> Seek for Chunked<R> {
    fn seek(&mut self, to: SeekFrom) -> std::io::Result<u64> {
        let (base, delta) = match to {
            SeekFrom::Start(p) => (p, 0),
            SeekFrom::Current(d) => (self.pos, d),
            SeekFrom::End(d) => (self.inner_len()?, d),
        };
        self.pos = base.checked_add_signed(delta).ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "a seek before the start of the file",
            )
        })?;
        Ok(self.pos)
    }
}
/// A nested zip held in memory: a cursor whose reads, each a copy out of
/// the zip's bytes, are timed and counted as [`Chunked`]'s are.
struct InMemory(std::io::Cursor<Vec<u8>>);

impl Read for InMemory {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        zone!(MEMORY_COPY);
        let n = self.0.read(out)?;
        count!(copied, n);
        Ok(n)
    }
}

impl Seek for InMemory {
    fn seek(&mut self, to: SeekFrom) -> std::io::Result<u64> {
        self.0.seek(to)
    }
}

/// A nested zip up to this size is read into memory and walked there;
/// a larger one goes to a temporary file.
const NESTED_IN_MEMORY: u64 = 64 << 20;
/// Write buffer of a nested zip spooled to disk.
const SPOOL_BUFFER: usize = 1 << 20;

#[allow(clippy::too_many_arguments)]
fn walk<E>(
    path: &Path,
    tmp_dir: &Path,
    in_memory: u64,
    depth: u8,
    prefix: &str,
    accepts: Accepts<'_>,
    on_entry: &mut dyn FnMut(Entry<'_>) -> Result<(), E>,
    on_other: &mut dyn FnMut(Entry<'_>),
) -> Result<Walked, WalkError<E>> {
    let p = path.display().to_string();
    zone!(WALK);
    let file = File::open(path).map_err(|e| open_error(&p, e.to_string()))?;
    let archive = open_archive(Chunked::new(file)).map_err(|e| open_error(&p, e.to_string()))?;
    walk_zip(
        archive, &p, tmp_dir, in_memory, depth, prefix, accepts, on_entry, on_other,
    )
}

/// Where to look for what a walk wants, and what to do with it.
struct Walk<'w, 'a, E> {
    tmp_dir: &'w Path,
    in_memory: u64,
    accepts: Accepts<'a>,
    on_entry: &'w mut dyn FnMut(Entry<'_>) -> Result<(), E>,
    on_other: &'w mut dyn FnMut(Entry<'_>),
}

/// Every entry of `archive`, in archive order, except that an entry whose
/// local header disagrees with the central directory (see [`Misplaced`])
/// comes after the others.
#[allow(clippy::too_many_arguments)]
fn walk_zip<R: Read + Seek, E>(
    archive: zip::ZipArchive<R>,
    p: &str,
    tmp_dir: &Path,
    in_memory: u64,
    depth: u8,
    prefix: &str,
    accepts: Accepts<'_>,
    on_entry: &mut dyn FnMut(Entry<'_>) -> Result<(), E>,
    on_other: &mut dyn FnMut(Entry<'_>),
) -> Result<Walked, WalkError<E>> {
    let mut w = Walk {
        tmp_dir,
        in_memory,
        accepts,
        on_entry,
        on_other,
    };
    let (mut archive, misplaced) = find_misplaced(archive, p)?;
    let mut found = Walked::default();
    for i in 0..archive.len() {
        if misplaced.binary_search_by_key(&i, |m| m.index).is_ok() {
            continue;
        }
        let opened = {
            zone!(ENTRY_OPEN);
            archive.by_index(i)
        };
        let mut entry = match opened {
            Ok(e) => e,
            Err(e) => {
                found
                    .failed
                    .push(entry_failed(p, i.to_string(), e.to_string()));
                continue;
            }
        };
        if entry.is_dir() {
            continue;
        }
        // Entry names are ASCII in AEMO's archives; any other byte is kept
        // as the replacement character, never an error.
        let name = String::from_utf8_lossy(entry.name_raw()).into_owned();
        let size = entry.size();
        found.take(visit(&mut w, p, depth, prefix, &name, size, &mut entry))?;
    }
    if !misplaced.is_empty() {
        let mut r = archive.into_inner();
        for m in &misplaced {
            let full = joined(prefix, &m.name);
            r.seek(SeekFrom::Start(m.start))
                .map_err(|e| entry_error(p, full.clone(), e.to_string()))?;
            let mut reader = Verified::new(Read::take(&mut r, m.compressed), m);
            found.take(visit(
                &mut w,
                p,
                depth,
                prefix,
                &m.name,
                m.size,
                &mut reader,
            ))?;
        }
    }
    if depth == 0 && found.entries == 0 && found.failed.is_empty() {
        return Err(WalkError::Archive(ArchiveError::NoEntry {
            path: p.to_owned(),
            wanted: w.accepts.wanted.to_owned(),
        }));
    }
    Ok(found)
}

/// The zip reader over `r`, its directory read.
///
/// The zip reader's default looks for data put before the zip (a
/// self-extractor's) by scanning the whole file from its first byte for
/// directory signatures, every time it opens one: for a month's archive
/// and the zips inside it, gigabytes read to find a directory whose place
/// the end record already gives. A zip starting at its first byte is
/// opened from there, which checks four bytes; only one that does not is
/// scanned for.
fn open_archive<R: Read + Seek>(mut r: R) -> zip::result::ZipResult<zip::ZipArchive<R>> {
    zone!(ARCHIVE_OPEN);
    let len = r.seek(SeekFrom::End(0))?;
    if zipdir::starts_at_zero(&mut r, len) {
        let at_start = zip::read::Config {
            archive_offset: zip::read::ArchiveOffset::Known(0),
        };
        return zip::ZipArchive::with_config(at_start, r);
    }
    zip::ZipArchive::new(r)
}

/// The archive at `path` could not be opened as a zip.
fn open_error<E>(path: &str, message: String) -> WalkError<E> {
    WalkError::Archive(ArchiveError::Open {
        path: path.to_owned(),
        message,
    })
}

/// Entry `entry` of the archive at `path` could not be read.
fn entry_failed(path: &str, entry: String, message: String) -> ArchiveError {
    ArchiveError::Entry {
        path: path.to_owned(),
        entry,
        message,
    }
}

/// [`entry_failed`], as a walk's error.
fn entry_error<E>(path: &str, entry: String, message: String) -> WalkError<E> {
    WalkError::Archive(entry_failed(path, entry, message))
}

/// `name` under `prefix`, nested names joined with `!`.
fn joined(prefix: &str, name: &str) -> String {
    if prefix.is_empty() {
        name.to_owned()
    } else {
        format!("{prefix}!{name}")
    }
}

/// One entry named `name` (in its own zip) read from `reader`: handed to
/// `on_entry` when a `.CSV`, walked when a zip, else handed to `on_other`.
/// Returns what it held.
fn visit<E>(
    w: &mut Walk<'_, '_, E>,
    p: &str,
    depth: u8,
    prefix: &str,
    name: &str,
    size: u64,
    entry: &mut dyn Read,
) -> Result<Walked, WalkError<E>> {
    let full = joined(prefix, name);
    if (w.accepts.accepts)(&full) {
        let mut reader = Counting {
            inner: entry,
            path: p,
            entry: &full,
            failed: None,
        };
        let result = {
            zone!(ON_ENTRY);
            (w.on_entry)(Entry {
                name: full.clone(),
                size: Some(size),
                reader: &mut reader,
            })
        };
        if let Some(e) = reader.failed.take() {
            return Err(WalkError::Archive(e));
        }
        result.map_err(WalkError::Callback)?;
        return Ok(Walked {
            entries: 1,
            failed: Vec::new(),
        });
    }
    if !has_extension(name, ".zip") {
        zone!(ON_OTHER);
        (w.on_other)(Entry {
            name: full,
            size: Some(size),
            reader: entry,
        });
        return Ok(Walked::default());
    }
    if depth + 1 > MAX_DEPTH {
        return Err(WalkError::Archive(ArchiveError::TooDeep {
            path: p.to_owned(),
            entry: full,
            depth: depth + 1,
        }));
    }
    let inner_label = format!("{p}!{name}");
    let bad_inner = |message: String| entry_error(p, full.clone(), message);
    if size <= w.in_memory {
        // The zip format needs to seek to its central directory; a small
        // inner zip is walked in memory, never on disk.
        let bytes = {
            zone!(NESTED_MEMORY);
            let mut bytes = Vec::with_capacity(size as usize);
            entry
                .read_to_end(&mut bytes)
                .map_err(|e| bad_inner(e.to_string()))?;
            count!(copied, bytes.len());
            bytes
        };
        let inner = open_archive(InMemory(std::io::Cursor::new(bytes)))
            .map_err(|e| bad_inner(e.to_string()))?;
        return walk_zip(
            inner,
            &inner_label,
            w.tmp_dir,
            w.in_memory,
            depth + 1,
            &full,
            w.accepts,
            w.on_entry,
            w.on_other,
        );
    }
    let spool_err = |tmp: &Path, source: std::io::Error| {
        WalkError::Archive(ArchiveError::Spool {
            path: p.to_owned(),
            entry: full.clone(),
            tmp: tmp.display().to_string(),
            source,
        })
    };
    std::fs::create_dir_all(w.tmp_dir).map_err(|e| spool_err(w.tmp_dir, e))?;
    // Unique, and removed when dropped, whatever happens.
    let tmp = tempfile::NamedTempFile::new_in(w.tmp_dir).map_err(|e| spool_err(w.tmp_dir, e))?;
    {
        zone!(NESTED_SPOOL);
        let mut out = std::io::BufWriter::with_capacity(SPOOL_BUFFER, tmp.as_file());
        let _n = std::io::copy(entry, &mut out)
            .and_then(|n| out.flush().map(|()| n))
            .map_err(|e| spool_err(tmp.path(), e))?;
        count!(copied, _n);
    }
    let file = tmp.reopen().map_err(|e| spool_err(tmp.path(), e))?;
    let inner = open_archive(Chunked::new(file)).map_err(|e| bad_inner(e.to_string()))?;
    walk_zip(
        inner,
        &inner_label,
        w.tmp_dir,
        w.in_memory,
        depth + 1,
        &full,
        w.accepts,
        w.on_entry,
        w.on_other,
    )
}

/// An entry whose local header disagrees with its central directory
/// record: the header carries the PK signature but not the entry's name
/// where the name belongs, so the offset of its data computed from the
/// header is wrong. AEMO's archive has such entries (MMSDM 2013-10,
/// `PUBLIC_DVD_PREDISPATCHCONSTRAINT2_201310010000.zip`: 8-byte sizes
/// written over the name and extra-field lengths). Its data is the
/// `compressed` bytes that end where the next record begins; read from
/// there, it must inflate to the central record's size and CRC-32, or the
/// entry is refused as before.
#[derive(Debug)]
struct Misplaced {
    index: usize,
    name: String,
    start: u64,
    compressed: u64,
    size: u64,
    crc32: u32,
    deflated: bool,
}

/// The central directory record of one entry.
struct Central {
    header_start: u64,
    compressed: u64,
    size: u64,
    crc32: u32,
    method: zip::CompressionMethod,
    name_raw: Vec<u8>,
    dir: bool,
}

/// Size of a local file header before its name.
const LOCAL_HEADER: u64 = 30;

/// `archive` again, with every entry of it that is [`Misplaced`]. An entry
/// whose local header cannot be read, or lacks the signature, is left to
/// the zip reader, which reports it as before.
fn find_misplaced<R: Read + Seek, E>(
    mut archive: zip::ZipArchive<R>,
    p: &str,
) -> Result<(zip::ZipArchive<R>, Vec<Misplaced>), WalkError<E>> {
    zone!(FIND_MISPLACED);
    let mut central = Vec::with_capacity(archive.len());
    for i in 0..archive.len() {
        let Ok(f) = archive.by_index_raw(i) else {
            // Reported by the walk itself.
            return Ok((archive, Vec::new()));
        };
        central.push(Central {
            header_start: f.header_start(),
            compressed: f.compressed_size(),
            size: f.size(),
            crc32: f.crc32(),
            method: f.compression(),
            name_raw: f.name_raw().to_vec(),
            dir: f.is_dir(),
        });
    }
    let directory = archive.central_directory_start();
    // Where each entry's data ends is where the next record begins: the
    // record starts in order, each found by a binary search.
    let mut starts: Vec<u64> = central.iter().map(|c| c.header_start).collect();
    starts.sort_unstable();
    let mut r = archive.into_inner();
    let mut out = Vec::new();
    for (index, c) in central.iter().enumerate() {
        if c.dir || local_header_agrees(&mut r, c) {
            continue;
        }
        let deflated = match c.method {
            zip::CompressionMethod::Deflated => true,
            zip::CompressionMethod::Stored => false,
            _ => continue,
        };
        let next = starts.partition_point(|&h| h <= c.header_start);
        let end = starts.get(next).copied().unwrap_or(directory);
        let Some(start) = end.checked_sub(c.compressed) else {
            continue;
        };
        if start < c.header_start + LOCAL_HEADER {
            continue;
        }
        out.push(Misplaced {
            index,
            name: String::from_utf8_lossy(&c.name_raw).into_owned(),
            start,
            compressed: c.compressed,
            size: c.size,
            crc32: c.crc32,
            deflated,
        });
    }
    let archive = {
        zone!(REOPEN);
        open_archive(r).map_err(|e| open_error(p, e.to_string()))?
    };
    Ok((archive, out))
}

/// Whether the local header of `c` names the entry as its central record
/// does. A header that cannot be read or has no signature counts as
/// agreeing: the zip reader reports it.
fn local_header_agrees<R: Read + Seek>(r: &mut R, c: &Central) -> bool {
    zone!(LOCAL_HEADER);
    let mut head = [0u8; LOCAL_HEADER as usize];
    if r.seek(SeekFrom::Start(c.header_start)).is_err() || r.read_exact(&mut head).is_err() {
        return true;
    }
    if head[..4] != *b"PK\x03\x04" {
        return true;
    }
    let name_len = usize::from(u16::from_le_bytes([head[26], head[27]]));
    if name_len != c.name_raw.len() {
        return false;
    }
    let mut name = vec![0u8; name_len];
    if r.read_exact(&mut name).is_err() {
        return true;
    }
    name == c.name_raw
}

/// The bytes of a [`Misplaced`] entry: inflated (or stored), counted and
/// hashed, and refused at the end unless they are the size and CRC-32 its
/// central record gives.
struct Verified<'m, R: Read> {
    inner: Raw<R>,
    m: &'m Misplaced,
    hasher: flate2::Crc,
    read: u64,
}

enum Raw<R: Read> {
    Deflated(flate2::read::DeflateDecoder<R>),
    Stored(R),
}

impl<'m, R: Read> Verified<'m, R> {
    fn new(raw: R, m: &'m Misplaced) -> Self {
        let inner = if m.deflated {
            Raw::Deflated(flate2::read::DeflateDecoder::new(raw))
        } else {
            Raw::Stored(raw)
        };
        Self {
            inner,
            m,
            hasher: flate2::Crc::new(),
            read: 0,
        }
    }
}

impl<R: Read> Read for Verified<'_, R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        zone!(VERIFIED_READ);
        let n = match &mut self.inner {
            Raw::Deflated(d) => d.read(buf)?,
            Raw::Stored(s) => s.read(buf)?,
        };
        self.hasher.update(&buf[..n]);
        self.read += n as u64;
        if n == 0 || self.read > self.m.size {
            let crc = self.hasher.sum();
            if self.read != self.m.size || crc != self.m.crc32 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!(
                        "its local header disagrees with the central directory, and its data read from where the next record begins is {} bytes with CRC-32 {crc:08x}, not {} bytes with {:08x}",
                        self.read, self.m.size, self.m.crc32
                    ),
                ));
            }
        }
        Ok(n)
    }
}

/// Wraps the entry reader so an inflate error is reported as the archive's,
/// with the entry name, rather than as a plain I/O error of the scanner.
struct Counting<'a> {
    inner: &'a mut dyn Read,
    path: &'a str,
    entry: &'a str,
    failed: Option<ArchiveError>,
}

impl Read for Counting<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        zone!(ENTRY_READ);
        match self.inner.read(buf) {
            Ok(n) => Ok(n),
            Err(e) => {
                self.failed = Some(ArchiveError::Entry {
                    path: self.path.to_owned(),
                    entry: self.entry.to_owned(),
                    message: e.to_string(),
                });
                Err(e)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;

    /// Builds a zip in `dir` with the given (name, bytes) entries.
    fn make_zip(path: &Path, entries: &[(&str, &[u8])]) {
        let f = File::create(path).unwrap();
        let mut w = zip::ZipWriter::new(f);
        for (name, bytes) in entries {
            w.start_file(*name, zip::write::SimpleFileOptions::default())
                .unwrap();
            w.write_all(bytes).unwrap();
        }
        w.finish().unwrap();
    }

    fn fixture(name: &str) -> Vec<u8> {
        std::fs::read(format!(
            "{}/../../testdata/csv/{name}",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap()
    }

    type Collected = (Vec<(String, Vec<u8>)>, Vec<String>, Vec<ArchiveError>);

    fn collect(path: &Path, tmp: &Path) -> Result<Collected, WalkError<std::io::Error>> {
        collect_with(path, tmp, NESTED_IN_MEMORY)
    }

    /// As [`collect`], with nested zips above `in_memory` bytes spooled.
    fn collect_with(
        path: &Path,
        tmp: &Path,
        in_memory: u64,
    ) -> Result<Collected, WalkError<std::io::Error>> {
        let mut got = Vec::new();
        let mut others = Vec::new();
        let w = walk(
            path,
            tmp,
            in_memory,
            0,
            "",
            CSV,
            &mut |e| {
                let mut buf = Vec::new();
                e.reader.read_to_end(&mut buf)?;
                got.push((e.name, buf));
                Ok(())
            },
            &mut |o| others.push(o.name),
        )?;
        Ok((got, others, w.failed))
    }

    /// A cursor that counts what is read from it.
    struct Counting {
        inner: std::io::Cursor<Vec<u8>>,
        bytes: u64,
        reads: u64,
    }

    impl Read for Counting {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            let n = self.inner.read(buf)?;
            self.bytes += n as u64;
            self.reads += 1;
            Ok(n)
        }
    }

    impl Seek for Counting {
        fn seek(&mut self, to: SeekFrom) -> std::io::Result<u64> {
            self.inner.seek(to)
        }
    }

    /// A file of zeros of a given length, never held in memory.
    struct Sized(u64);

    impl Read for Sized {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            buf.fill(0);
            Ok(buf.len())
        }
    }

    impl Seek for Sized {
        fn seek(&mut self, to: SeekFrom) -> std::io::Result<u64> {
            Ok(match to {
                SeekFrom::End(d) => self.0.saturating_add_signed(d),
                SeekFrom::Start(p) => p,
                SeekFrom::Current(_) => 0,
            })
        }
    }

    #[test]
    fn a_zip_is_read_in_chunks_once_however_its_reader_seeks() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.zip");
        let entries: Vec<(String, Vec<u8>)> = (0..40)
            .map(|i| (format!("{i}.CSV"), vec![b'a' + (i % 26) as u8; 997]))
            .collect();
        let borrowed: Vec<(&str, &[u8])> = entries
            .iter()
            .map(|(n, b)| (n.as_str(), b.as_slice()))
            .collect();
        make_zip(&path, &borrowed);
        let bytes = std::fs::read(&path).unwrap();
        let len = bytes.len() as u64;
        // Chunks far smaller than the zip: every chunk is read from the
        // cursor at most a few times over the whole walk, in whole chunks.
        let mut archive = zip::ZipArchive::new(Chunked::with_chunk(
            Counting {
                inner: std::io::Cursor::new(bytes.clone()),
                bytes: 0,
                reads: 0,
            },
            512,
        ))
        .unwrap();
        for (i, (name, body)) in entries.iter().enumerate() {
            let mut e = archive.by_index(i).unwrap();
            assert_eq!(e.name().unwrap(), name.as_str());
            let mut got = Vec::new();
            e.read_to_end(&mut got).unwrap();
            assert_eq!(&got, body);
        }
        let c = archive.into_inner();
        assert!(c.inner.bytes <= 4 * len, "{} of {len}", c.inner.bytes);
        assert!(
            c.inner.reads <= 4 * len / 512 + 4,
            "{} reads",
            c.inner.reads
        );
        // The seeks of a reader, and reads across a chunk's end and past
        // the file's.
        let mut r = Chunked::with_chunk(std::io::Cursor::new(bytes.clone()), 100);
        let mut buf = [0u8; 150];
        assert_eq!(r.seek(SeekFrom::Start(70)).unwrap(), 70);
        r.read_exact(&mut buf).unwrap();
        assert_eq!(&buf[..], &bytes[70..220]);
        assert_eq!(r.seek(SeekFrom::Current(-20)).unwrap(), 200);
        assert_eq!(r.read(&mut buf).unwrap(), 100, "to the chunk's end");
        assert_eq!(&buf[..100], &bytes[200..300]);
        assert_eq!(r.seek(SeekFrom::End(-10)).unwrap(), len - 10);
        r.read_exact(&mut buf[..10]).unwrap();
        assert_eq!(&buf[..10], &bytes[bytes.len() - 10..]);
        assert_eq!(r.read(&mut buf).unwrap(), 0, "at the end");
        assert_eq!(r.seek(SeekFrom::End(10)).unwrap(), len + 10);
        assert_eq!(r.read(&mut buf).unwrap(), 0, "past the end");
        assert_eq!(r.read(&mut []).unwrap(), 0);
        // A file smaller than a chunk is held at its own size.
        let mut small = Chunked::new(std::io::Cursor::new(vec![7u8; 1000]));
        let mut one = [0u8; 1];
        small.read_exact(&mut one).unwrap();
        assert!(small.buf.capacity() <= 1000, "{}", small.buf.capacity());
        assert_eq!(small.chunk, ZIP_READ_MIN);
        // A big one in an eighth of its size, at most a whole chunk.
        let mut big = Chunked::new(std::io::Cursor::new(vec![0u8; 4 << 20]));
        big.read_exact(&mut one).unwrap();
        assert_eq!(big.chunk, 512 << 10);
        let mut huge = Chunked::new(Sized(1 << 40));
        huge.seek(SeekFrom::End(-1)).unwrap();
        assert_eq!(huge.inner.stream_position().unwrap(), 0);
        huge.read_exact(&mut one).unwrap();
        assert_eq!(huge.chunk, ZIP_READ_BUFFER);
        assert_eq!(
            r.seek(SeekFrom::Start(0))
                .and_then(|_| r.seek(SeekFrom::Current(-1)))
                .unwrap_err()
                .kind(),
            std::io::ErrorKind::InvalidInput
        );
    }

    #[test]
    fn walks_flat_nested_and_multi_entry_zips() {
        let dir = tempfile::tempdir().unwrap();
        let region = fixture("REGION_v1_head.csv");
        let inter = fixture("INTERCONNECTOR_v1_head.csv");
        let inner = dir.path().join("inner.zip");
        make_zip(&inner, &[("INNER.CSV", &inter), ("notes.txt", b"x")]);
        let inner_bytes = std::fs::read(&inner).unwrap();
        let outer = dir.path().join("outer.zip");
        make_zip(
            &outer,
            &[
                ("A.CSV", &region),
                ("dir/", b""),
                ("nested.zip", &inner_bytes),
                ("B.csv", &region),
                ("readme.md", b"hi"),
            ],
        );
        let tmp = dir.path().join("tmp");
        let (got, others, failed) = collect(&outer, &tmp).unwrap();
        let names: Vec<&str> = got.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["A.CSV", "nested.zip!INNER.CSV", "B.csv"]);
        assert!(failed.is_empty());
        assert_eq!(got[1].1, inter);
        assert_eq!(others, ["nested.zip!notes.txt", "readme.md"]);
        assert!(!tmp.exists(), "a small nested zip is read in memory");
        // A big one goes through a temporary file, removed once read.
        let spooled = collect_with(&outer, &tmp, 0).unwrap();
        assert_eq!(spooled.0, got);
        assert!(
            std::fs::read_dir(&tmp).unwrap().next().is_none(),
            "spooled zip removed"
        );
    }

    #[test]
    fn a_zip_with_data_before_it_is_still_found() {
        let dir = tempfile::tempdir().unwrap();
        let region = fixture("REGION_v1_head.csv");
        let path = dir.path().join("sfx.zip");
        make_zip(&path, &[("A.CSV", &region)]);
        let mut bytes = b"a self-extractor's stub".to_vec();
        bytes.extend_from_slice(&std::fs::read(&path).unwrap());
        std::fs::write(&path, bytes).unwrap();
        let (got, _, failed) = collect(&path, dir.path()).unwrap();
        assert!(failed.is_empty(), "{failed:?}");
        assert_eq!(got, [("A.CSV".to_owned(), region)]);
    }

    #[test]
    fn errors() {
        let dir = tempfile::tempdir().unwrap();
        let tmp = dir.path().join("tmp");
        let e = collect(&dir.path().join("none.zip"), &tmp).unwrap_err();
        assert!(
            matches!(e, WalkError::Archive(ArchiveError::Open { .. })),
            "{e}"
        );
        let bad = dir.path().join("bad.zip");
        std::fs::write(&bad, b"not a zip").unwrap();
        assert!(matches!(
            collect(&bad, &tmp).unwrap_err(),
            WalkError::Archive(ArchiveError::Open { .. })
        ));
        let empty = dir.path().join("empty.zip");
        make_zip(&empty, &[("a.txt", b"x")]);
        let e = collect(&empty, &tmp).unwrap_err();
        assert!(
            matches!(e, WalkError::Archive(ArchiveError::NoEntry { .. })),
            "{e}"
        );
        assert_eq!(
            e.to_string(),
            format!(
                "The archive {} has no .CSV entry at any depth.",
                empty.display()
            )
        );
        // Depth: zip in zip in zip.
        let l2 = dir.path().join("l2.zip");
        make_zip(&l2, &[("X.CSV", b"C,x\n")]);
        let l1 = dir.path().join("l1.zip");
        make_zip(&l1, &[("l2.zip", &std::fs::read(&l2).unwrap())]);
        let l0 = dir.path().join("l0.zip");
        make_zip(&l0, &[("l1.zip", &std::fs::read(&l1).unwrap())]);
        let (got, ..) = collect(&l1, &tmp).unwrap();
        assert_eq!(got[0].0, "l2.zip!X.CSV");
        let (got, ..) = collect(&l0, &tmp).unwrap();
        assert_eq!(
            got[0].0, "l1.zip!l2.zip!X.CSV",
            "two levels of nesting are allowed"
        );
        // Too deep: left behind, beside the entries that read.
        let l_minus = dir.path().join("lm.zip");
        make_zip(
            &l_minus,
            &[
                ("l0.zip", &std::fs::read(&l0).unwrap()),
                ("Y.CSV", b"C,y\n"),
            ],
        );
        let (got, _, failed) = collect(&l_minus, &tmp).unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(failed.len(), 1);
        let deep = failed[0].to_string();
        assert!(deep.contains("is a zip nested 3 deep"), "{deep}");
        assert_eq!(
            ArchiveError::TooDeep {
                path: "p".into(),
                entry: "e".into(),
                depth: 3,
            }
            .to_string(),
            "Entry e of p is a zip nested 3 deep; the limit is 2."
        );
        // Callback error propagates.
        let one = dir.path().join("one.zip");
        make_zip(&one, &[("A.CSV", b"C,x\n")]);
        let r: Result<Walked, WalkError<&str>> =
            walk_csv_entries(&one, &tmp, &mut |_| Err("stop"), &mut |_| {});
        assert!(matches!(r, Err(WalkError::Callback("stop"))));
        assert_eq!(
            r.unwrap_err().to_string(),
            "The callback for an entry of the archive failed."
        );
        // Spool failure: tmp dir is a file.
        let tmp_file = dir.path().join("tmpfile");
        std::fs::write(&tmp_file, b"").unwrap();
        let (got, _, failed) = collect_with(&l1, &tmp_file, 0).unwrap();
        assert!(got.is_empty());
        assert!(
            matches!(failed[0], ArchiveError::Spool { .. }),
            "{failed:?}"
        );
        // An entry of a method the reader lacks (bzip2, 12) is left behind
        // where it is opened, and the entry after it is read.
        let two = dir.path().join("two.zip");
        make_zip(&two, &[("A.CSV", b"C,a\n"), ("B.CSV", b"C,b\n")]);
        let mut bytes = std::fs::read(&two).unwrap();
        let cd = zip::ZipArchive::new(std::io::Cursor::new(bytes.clone()))
            .unwrap()
            .central_directory_start() as usize;
        bytes[8..10].copy_from_slice(&12_u16.to_le_bytes());
        bytes[cd + 10..cd + 12].copy_from_slice(&12_u16.to_le_bytes());
        std::fs::write(&two, &bytes).unwrap();
        let (got, _, failed) = collect(&two, &tmp).unwrap();
        assert_eq!(got.len(), 1, "{failed:?}");
        assert_eq!(got[0].0, "B.CSV");
        assert!(
            matches!(failed[0], ArchiveError::Entry { ref entry, .. } if entry == "0"),
            "{failed:?}"
        );
        // A nested entry that is not a zip is left behind, in memory or
        // spooled alike.
        let fake = dir.path().join("fake.zip");
        make_zip(&fake, &[("inner.zip", b"not a zip")]);
        for in_memory in [NESTED_IN_MEMORY, 0] {
            let (got, _, failed) = collect_with(&fake, &tmp, in_memory).unwrap();
            assert!(got.is_empty());
            assert!(
                matches!(failed[0], ArchiveError::Entry { .. }),
                "{failed:?}"
            );
        }
        assert_eq!(
            ArchiveError::Entry {
                path: "p".into(),
                entry: "e".into(),
                message: "m".into()
            }
            .to_string(),
            "Entry e of p could not be read: m"
        );
        assert_eq!(
            ArchiveError::Open {
                path: "p".into(),
                message: "m".into()
            }
            .to_string(),
            "The archive p could not be opened: m"
        );
        let e = ArchiveError::Spool {
            path: "p".into(),
            entry: "e".into(),
            tmp: "t".into(),
            source: std::io::Error::other("x"),
        };
        assert_eq!(e.to_string(), "Spooling nested entry e of p to t failed.");
        assert_eq!(
            std::error::Error::source(&e).map(ToString::to_string),
            Some("x".to_owned())
        );
    }

    /// Bytes that do not compress, so a deflate stream is long.
    fn noise(len: usize) -> Vec<u8> {
        let mut state = 0x9E37_79B9_u32;
        (0..len)
            .map(|_| {
                state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                (state >> 24) as u8
            })
            .collect()
    }

    /// A zip of `entries`, the local header of entry `broken` damaged the
    /// way AEMO's are: its name-length field holds a size's high bytes.
    fn misplaced_zip(entries: &[(&str, &[u8], bool)], broken: usize) -> Vec<u8> {
        let mut cursor = std::io::Cursor::new(Vec::new());
        {
            let mut w = zip::ZipWriter::new(&mut cursor);
            for (name, bytes, stored) in entries {
                let method = if *stored {
                    zip::CompressionMethod::Stored
                } else {
                    zip::CompressionMethod::Deflated
                };
                w.start_file(
                    *name,
                    zip::write::SimpleFileOptions::default().compression_method(method),
                )
                .unwrap();
                w.write_all(bytes).unwrap();
            }
            w.finish().unwrap();
        }
        let mut bytes = cursor.into_inner();
        let at = zip::ZipArchive::new(std::io::Cursor::new(bytes.clone()))
            .unwrap()
            .by_index_raw(broken)
            .unwrap()
            .header_start() as usize;
        bytes[at + 26..at + 28].copy_from_slice(&0x4BAF_u16.to_le_bytes());
        bytes
    }

    #[test]
    fn an_entry_whose_local_header_disagrees_is_read_from_the_central_directory() {
        let dir = tempfile::tempdir().unwrap();
        let tmp = dir.path().join("tmp");
        let big = noise(200_000);
        let region = fixture("REGION_v1_head.csv");
        // The plain zip reader refuses it.
        let bytes = misplaced_zip(&[("A.CSV", &region, false), ("B.CSV", &big, false)], 1);
        let mut plain = zip::ZipArchive::new(std::io::Cursor::new(bytes.clone())).unwrap();
        let mut sink = Vec::new();
        assert!(plain.by_index(1).unwrap().read_to_end(&mut sink).is_err());
        // The walk reads it, after the others.
        let z = dir.path().join("m.zip");
        std::fs::write(&z, &bytes).unwrap();
        let (got, ..) = collect(&z, &tmp).unwrap();
        assert_eq!(got.len(), 2);
        assert_eq!(got[0], ("A.CSV".to_owned(), region.clone()));
        assert_eq!(got[1], ("B.CSV".to_owned(), big.clone()));
        // The last entry (data ends at the central directory), and a
        // stored one with a text entry and a directory beside it.
        let stored = misplaced_zip(
            &[
                ("C.CSV", &region, true),
                ("d/", b"", true),
                ("n.txt", b"x", false),
            ],
            0,
        );
        let z = dir.path().join("s.zip");
        std::fs::write(&z, &stored).unwrap();
        let (got, others, _) = collect(&z, &tmp).unwrap();
        assert_eq!(got, [("C.CSV".to_owned(), region.clone())]);
        assert_eq!(others, ["n.txt"]);
        // A callback's error ends the walk at a misplaced entry too.
        let r: Result<Walked, WalkError<&str>> =
            walk_csv_entries(&z, &tmp, &mut |_| Err("stop"), &mut |_| {});
        assert!(matches!(r, Err(WalkError::Callback("stop"))));
        // Nested in a whole-month zip, in memory and spooled.
        let outer = dir.path().join("outer.zip");
        make_zip(&outer, &[("in/X.zip", &bytes), ("README.txt", b"r")]);
        for in_memory in [NESTED_IN_MEMORY, 0] {
            let (got, others, _) = collect_with(&outer, &tmp, in_memory).unwrap();
            let names: Vec<&str> = got.iter().map(|(n, _)| n.as_str()).collect();
            assert_eq!(names, ["in/X.zip!A.CSV", "in/X.zip!B.CSV"]);
            assert_eq!(got[1].1, big);
            assert_eq!(others, ["README.txt"]);
        }
        // A misplaced zip entry is walked too.
        let holder = misplaced_zip(&[("in/X.zip", &bytes, false)], 0);
        let z = dir.path().join("h.zip");
        std::fs::write(&z, &holder).unwrap();
        let (got, ..) = collect(&z, &tmp).unwrap();
        assert_eq!(got[1], ("in/X.zip!B.CSV".to_owned(), big.clone()));
    }

    #[test]
    fn a_misplaced_entry_that_does_not_match_its_crc_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let tmp = dir.path().join("tmp");
        let big = noise(50_000);
        // Stored, one byte changed: right size, wrong CRC-32.
        let mut bytes = misplaced_zip(&[("A.CSV", &big, true)], 0);
        let at = 30 + "A.CSV".len() + 100;
        bytes[at] ^= 0xFF;
        let z = dir.path().join("c.zip");
        std::fs::write(&z, &bytes).unwrap();
        let (got, _, failed) = collect(&z, &tmp).unwrap();
        assert!(got.is_empty());
        assert!(
            failed[0].to_string().contains("local header disagrees"),
            "{failed:?}"
        );
        // A name of the right length but other bytes is misplaced too; a
        // central size larger than the bytes there overruns into the next
        // record and is refused, and one with no room is left to the reader.
        let mut renamed = misplaced_zip(&[("A.CSV", &big, true), ("B.CSV", b"b", true)], 0);
        renamed[26..28].copy_from_slice(&5_u16.to_le_bytes());
        renamed[30] = b'Z';
        let z = dir.path().join("r.zip");
        std::fs::write(&z, &renamed).unwrap();
        let (got, ..) = collect(&z, &tmp).unwrap();
        assert_eq!(got[0], ("B.CSV".to_owned(), b"b".to_vec()));
        assert_eq!(got[1], ("A.CSV".to_owned(), big.clone()));
        let header = Central {
            header_start: 0,
            compressed: 0,
            size: 0,
            crc32: 0,
            method: zip::CompressionMethod::Stored,
            name_raw: b"A.CSV".to_vec(),
            dir: false,
        };
        assert!(local_header_agrees(
            &mut std::io::Cursor::new(b"PK"),
            &header
        ));
        assert!(local_header_agrees(
            &mut std::io::Cursor::new(vec![0u8; 40]),
            &header
        ));
        let mut short = b"PK\x03\x04".to_vec();
        short.resize(30, 0);
        short[26] = 5;
        assert!(local_header_agrees(
            &mut std::io::Cursor::new(short),
            &header
        ));
        let m = Misplaced {
            index: 0,
            name: "A.CSV".into(),
            start: 0,
            compressed: 4,
            size: 2,
            crc32: 0,
            deflated: false,
        };
        let mut v = Verified::new(&b"abcd"[..], &m);
        let mut out = Vec::new();
        assert!(v.read_to_end(&mut out).is_err(), "longer than its size");
    }

    #[test]
    fn corrupt_entry_is_reported_as_the_archives() {
        let dir = tempfile::tempdir().unwrap();
        let z = dir.path().join("c.zip");
        // Incompressible data so the deflate stream is long enough to damage.
        let mut state = 0x9E37_79B9_u32;
        let noise: Vec<u8> = (0..100_000)
            .map(|_| {
                state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                (state >> 24) as u8
            })
            .collect();
        make_zip(&z, &[("A.CSV", &noise)]);
        // Flip bytes in the middle of the deflate stream.
        let mut bytes = std::fs::read(&z).unwrap();
        for b in &mut bytes[5_000..5_100] {
            *b ^= 0xFF;
        }
        std::fs::write(&z, &bytes).unwrap();
        let (got, _, failed) = collect(&z, &dir.path().join("tmp")).unwrap();
        assert!(got.is_empty());
        assert!(
            matches!(failed[0], ArchiveError::Entry { .. }),
            "{failed:?}"
        );
    }

    #[test]
    fn records_the_walk_cannot_place_are_left_to_the_zip_reader() {
        let dir = tempfile::tempdir().unwrap();
        let tmp = dir.path().join("tmp");
        let big = noise(5_000);
        // The entry is left behind; any other entry is read.
        let walk = |bytes: &[u8]| {
            let z = dir.path().join("odd.zip");
            std::fs::write(&z, bytes).unwrap();
            let (_, _, mut failed) = collect(&z, &tmp).unwrap();
            assert_eq!(failed.len(), 1, "{failed:?}");
            failed.remove(0)
        };
        // A local header without its signature.
        let plain = dir.path().join("plain.zip");
        make_zip(&plain, &[("A.CSV", &big)]);
        let mut bytes = std::fs::read(&plain).unwrap();
        bytes[..2].copy_from_slice(b"XX");
        let e = walk(&bytes);
        assert!(matches!(e, ArchiveError::Entry { .. }), "{e}");
        // A misplaced entry whose central record gives a method the walk
        // cannot read, a size larger than everything before the next
        // record, or one that leaves no room for the local header.
        let broken = misplaced_zip(&[("A.CSV", &big, true), ("B.CSV", b"b", true)], 0);
        let cd = zip::ZipArchive::new(std::io::Cursor::new(broken.clone()))
            .unwrap()
            .central_directory_start() as usize;
        let next = 30 + "A.CSV".len() + big.len();
        for (at, value) in [
            (cd + 10, 12_u32.to_le_bytes()),
            (cd + 20, 0x00FF_FFFF_u32.to_le_bytes()),
            (cd + 20, (next as u32 - 10).to_le_bytes()),
        ] {
            let mut bytes = broken.clone();
            let len = if at == cd + 10 { 2 } else { 4 };
            bytes[at..at + len].copy_from_slice(&value[..len]);
            let e = walk(&bytes);
            assert!(matches!(e, ArchiveError::Entry { .. }), "{at}: {e}");
        }
    }
}
