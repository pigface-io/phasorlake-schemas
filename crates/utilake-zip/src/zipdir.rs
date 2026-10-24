//! A zip's central directory from byte spans of the file, without the rest
//! of it: the end record in the file's last bytes says where the directory
//! is, and the directory lists every entry's name, sizes, CRC-32, method,
//! time and where its local header is. Nothing is decompressed, so a zip
//! bomb costs nothing here; sizes are what the zip declares.
//!
//! No I/O happens here. A [`DirectoryReader`] asks for byte spans and is
//! fed them, so the same reader serves ranged requests to nemweb (the
//! mirror's peek, the lake's picks) and a file on disk ([`read_from`]):
//!
//! ```
//! # use std::io::Write as _;
//! # let mut w = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
//! # w.start_file("a.csv", zip::write::SimpleFileOptions::default()).unwrap();
//! # w.write_all(b"x").unwrap();
//! # let bytes = w.finish().unwrap().into_inner();
//! use utilake_zip::zipdir::{DirectoryReader, Step};
//! let mut reader = DirectoryReader::new(bytes.len() as u64);
//! let mut step = reader.next();
//! let entries = loop {
//!     match step {
//!         Step::Read { start, end } => {
//!             step = reader.feed(bytes[start as usize..end as usize].to_vec()).unwrap();
//!         }
//!         Step::Done(entries) => break entries,
//!     }
//! };
//! assert_eq!(entries[0].name, "a.csv");
//! ```

use std::io::{Read, Seek, SeekFrom};

use utilake_provenance::Stamp;

/// The end record, the longest comment it may be followed by, and the
/// zip64 locator that may come before it.
pub const TAIL: u64 = 20 + 22 + 0xFFFF;

/// A local file header's fixed part: the lengths of the name and the
/// extra field that follow it are in its last four bytes.
pub const LOCAL_HEADER: usize = 30;

const EOCD: &[u8; 4] = b"PK\x05\x06";
const ZIP64_LOCATOR: &[u8; 4] = b"PK\x06\x07";
const ZIP64_EOCD: &[u8; 4] = b"PK\x06\x06";
const CENTRAL: &[u8; 4] = b"PK\x01\x02";
const LOCAL: &[u8; 4] = b"PK\x03\x04";

/// One entry of a zip, as its central directory records it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// The name, folders separated by `/` (a `\` in the zip is read as
    /// one); a folder's ends in `/`.
    pub name: String,
    /// Uncompressed size, as declared.
    pub size: u64,
    /// Compressed size, as declared.
    pub compressed: u64,
    /// CRC-32 of the uncompressed bytes.
    pub crc32: u32,
    /// Compression method (0 stored, 8 deflated).
    pub method: u16,
    /// The DOS date word: year since 1980 in the top seven bits, month,
    /// day.
    pub dos_date: u16,
    /// The DOS time word: hour, minute, seconds halved.
    pub dos_time: u16,
    /// Where the entry's local header starts in the file; its data follows
    /// the header, whose length [`local_header_len`] reads.
    pub local_header_offset: u64,
}

impl Entry {
    /// Modified, `YYYY-MM-DD HH:MM:SS`, from the DOS date and time (as
    /// packed; a month 0 prints as `00`).
    pub fn modified_text(&self) -> String {
        dos_text(self.dos_date, self.dos_time)
    }

    /// The calendar fields of the DOS date and time: year, month, day,
    /// hour, minute, second. Not checked for range.
    pub fn modified_fields(&self) -> (i64, u8, u8, u8, u8, u8) {
        let (date, time) = (self.dos_date, self.dos_time);
        (
            1980 + i64::from(date >> 9),
            ((date >> 5) & 0xF) as u8,
            (date & 0x1F) as u8,
            (time >> 11) as u8,
            ((time >> 5) & 0x3F) as u8,
            ((time & 0x1F) * 2) as u8,
        )
    }

    /// Modified, as an instant in the clock of the machine that packed the
    /// zip (`offset_secs`: AEST for AEMO's zips); none when the DOS fields
    /// are out of range.
    pub fn modified_at(&self, offset_secs: i32) -> Option<Stamp> {
        let (y, m, d, hh, mm, ss) = self.modified_fields();
        let y = i32::try_from(y).ok()?;
        Stamp::at(
            y,
            m.into(),
            d.into(),
            hh.into(),
            mm.into(),
            ss.into(),
            offset_secs,
        )
    }

    /// Whether the entry is a folder.
    pub fn is_dir(&self) -> bool {
        self.name.ends_with('/')
    }
}

/// A DOS date and time as `YYYY-MM-DD HH:MM:SS`.
pub fn dos_text(date: u16, time: u16) -> String {
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        1980 + (date >> 9),
        (date >> 5) & 0xF,
        date & 0x1F,
        time >> 11,
        (time >> 5) & 0x3F,
        (time & 0x1F) * 2
    )
}

/// Why a directory could not be read.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DirError {
    /// No end record in the last bytes: not a zip.
    #[error("no end-of-central-directory record in the last {0} bytes: not a zip")]
    NotAZip(u64),
    /// The records point somewhere they cannot be.
    #[error("the central directory is damaged: {0}")]
    Damaged(&'static str),
    /// Fewer bytes were fed than asked for.
    #[error("{got} bytes were read where {wanted} were asked for")]
    Short {
        /// Asked for.
        wanted: u64,
        /// Fed.
        got: u64,
    },
}

/// What a [`DirectoryReader`] wants next.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// Bytes `start..end` of the file, to be fed back.
    Read {
        /// First byte.
        start: u64,
        /// One past the last.
        end: u64,
    },
    /// Every entry, in directory order.
    Done(Vec<Entry>),
}

/// Where the directory is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Found {
    /// The directory's offset and size.
    Directory(u64, u64),
    /// A zip64 end record at this offset holds them.
    Zip64(u64),
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum State {
    /// The tail is wanted.
    Tail,
    /// The tail is kept; a zip64 end record outside it is wanted.
    Zip64 { tail_start: u64, tail: Vec<u8> },
    /// The directory, outside the tail, is wanted.
    Directory,
    /// Done.
    Done,
}

/// Reads a zip's directory in at most three spans: its tail, a zip64 end
/// record when it lies outside the tail, and the directory when it does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectoryReader {
    len: u64,
    state: State,
    wanted: (u64, u64),
}

impl DirectoryReader {
    /// A reader of the directory of a zip of `len` bytes.
    pub fn new(len: u64) -> Self {
        let tail_start = len.saturating_sub(TAIL);
        Self {
            len,
            state: State::Tail,
            wanted: (tail_start, len),
        }
    }

    /// The span wanted now.
    pub fn next(&self) -> Step {
        Step::Read {
            start: self.wanted.0,
            end: self.wanted.1,
        }
    }

    /// Feeds the bytes of the span [`next`](Self::next) asked for; the
    /// next step follows. Feeding after `Done` is an error.
    pub fn feed(&mut self, bytes: Vec<u8>) -> Result<Step, DirError> {
        let wanted = self.wanted.1 - self.wanted.0;
        if (bytes.len() as u64) < wanted {
            return Err(DirError::Short {
                wanted,
                got: bytes.len() as u64,
            });
        }
        let state = std::mem::replace(&mut self.state, State::Done);
        match state {
            State::Tail => {
                let tail_start = self.wanted.0;
                match locate(&bytes)? {
                    Found::Directory(offset, size) => {
                        self.directory(tail_start, &bytes, offset, size)
                    }
                    Found::Zip64(at) if at >= tail_start => {
                        let from = usize::try_from(at - tail_start).map_err(|_| damaged())?;
                        let record = bytes.get(from..).ok_or_else(damaged)?;
                        let (offset, size) = zip64_directory(record)?;
                        self.directory(tail_start, &bytes, offset, size)
                    }
                    Found::Zip64(at) => {
                        self.state = State::Zip64 {
                            tail_start,
                            tail: bytes,
                        };
                        self.wanted = (at, at + 56);
                        Ok(self.next())
                    }
                }
            }
            State::Zip64 { tail_start, tail } => {
                let (offset, size) = zip64_directory(&bytes)?;
                self.directory(tail_start, &tail, offset, size)
            }
            State::Directory => Ok(Step::Done(entries(&bytes)?)),
            State::Done => Err(DirError::Damaged("fed after the directory was read")),
        }
    }

    /// The directory at `offset..offset + size`: out of the tail when it
    /// lies there, else asked for.
    fn directory(
        &mut self,
        tail_start: u64,
        tail: &[u8],
        offset: u64,
        size: u64,
    ) -> Result<Step, DirError> {
        let end = offset
            .checked_add(size)
            .filter(|&e| e <= self.len)
            .ok_or_else(damaged)?;
        if offset >= tail_start {
            let from = usize::try_from(offset - tail_start).map_err(|_| damaged())?;
            let to = usize::try_from(end - tail_start).map_err(|_| damaged())?;
            self.state = State::Done;
            return Ok(Step::Done(entries(&tail[from..to])?));
        }
        self.state = State::Directory;
        self.wanted = (offset, end);
        Ok(self.next())
    }
}

/// Every entry of the zip `r` of `len` bytes, read by seeking.
pub fn read_from<R: Read + Seek>(r: &mut R, len: u64) -> Result<Vec<Entry>, DirError> {
    zone!(DIR_READ_FROM);
    let mut reader = DirectoryReader::new(len);
    let mut step = reader.next();
    loop {
        match step {
            Step::Read { start, end } => {
                let mut buf = vec![0u8; usize::try_from(end - start).map_err(|_| damaged())?];
                {
                    zone!(DIR_SPAN);
                    r.seek(SeekFrom::Start(start))
                        .and_then(|_| r.read_exact(&mut buf))
                        .map_err(|_| DirError::Damaged("the file could not be read"))?;
                    count!(read, buf.len());
                }
                step = reader.feed(buf)?;
            }
            Step::Done(entries) => return Ok(entries),
        }
    }
}

/// What the zip at `path` holds: each file's name, CRC-32 and size, in
/// name order, folders left out; none when it is not a zip that reads.
/// Two zips that hold the same are the same data however they are packed.
pub fn contents(path: &std::path::Path) -> Option<Vec<(String, u32, u64)>> {
    let mut file = std::fs::File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    let entries = read_from(&mut file, len).ok()?;
    let mut files: Vec<(String, u32, u64)> = entries
        .into_iter()
        .filter(|e| !e.is_dir())
        .map(|e| (e.name, e.crc32, e.size))
        .collect();
    files.sort();
    Some(files)
}

/// The length of the local header whose first [`LOCAL_HEADER`] bytes
/// these are: the fixed part, the name and the extra field. The entry's
/// data starts that far past [`Entry::local_header_offset`].
pub fn local_header_len(header: &[u8]) -> Result<usize, DirError> {
    if header.len() < LOCAL_HEADER || &header[..4] != LOCAL {
        return Err(DirError::Damaged(
            "no local header where the directory points",
        ));
    }
    Ok(LOCAL_HEADER + usize::from(u16_at(header, 26)) + usize::from(u16_at(header, 28)))
}

fn damaged() -> DirError {
    DirError::Damaged("an offset outside the file")
}

/// The end record in `tail` and where it says the directory is.
fn locate(tail: &[u8]) -> Result<Found, DirError> {
    zone!(DIR_LOCATE);
    let at = tail
        .windows(4)
        .rposition(|w| w == EOCD)
        .filter(|&at| tail.len() >= at + 22)
        .ok_or(DirError::NotAZip(tail.len() as u64))?;
    let eocd = &tail[at..];
    let size = u64::from(u32_at(eocd, 12));
    let offset = u64::from(u32_at(eocd, 16));
    // A zip64 locator sits just before the end record.
    if at >= 20 && &tail[at - 20..at - 16] == ZIP64_LOCATOR {
        return Ok(Found::Zip64(u64_at(&tail[at - 20..], 8)));
    }
    Ok(Found::Directory(offset, size))
}

/// The directory's offset and size from a zip64 end record.
fn zip64_directory(record: &[u8]) -> Result<(u64, u64), DirError> {
    if record.len() < 56 || &record[..4] != ZIP64_EOCD {
        return Err(DirError::Damaged(
            "no zip64 end record where the locator points",
        ));
    }
    Ok((u64_at(record, 48), u64_at(record, 40)))
}

/// Every record of a central directory.
fn entries(mut cd: &[u8]) -> Result<Vec<Entry>, DirError> {
    zone!(DIR_PARSE);
    let mut out = Vec::with_capacity(cd.len() / 80);
    while !cd.is_empty() {
        if cd.len() < 46 || &cd[..4] != CENTRAL {
            return Err(DirError::Damaged(
                "a record that is not a central directory record",
            ));
        }
        let name_len = usize::from(u16_at(cd, 28));
        let extra_len = usize::from(u16_at(cd, 30));
        let comment_len = usize::from(u16_at(cd, 32));
        let end = 46 + name_len + extra_len + comment_len;
        if cd.len() < end {
            return Err(DirError::Damaged("a record longer than the directory"));
        }
        let mut size = u64::from(u32_at(cd, 24));
        let mut compressed = u64::from(u32_at(cd, 20));
        let mut local_header_offset = u64::from(u32_at(cd, 42));
        // Sizes and offsets too big for 32 bits are in the zip64 extra
        // field, in order: size, compressed size, offset.
        let mut extra = &cd[46 + name_len..46 + name_len + extra_len];
        while extra.len() >= 4 {
            let (id, len) = (u16_at(extra, 0), usize::from(u16_at(extra, 2)));
            let body = &extra[4..(4 + len).min(extra.len())];
            if id == 1 {
                let mut fields = body.chunks_exact(8).map(|c| u64_at(c, 0));
                // A value marked too big for 32 bits takes the next field.
                let mut take = |value: &mut u64| -> Result<(), DirError> {
                    if *value == u64::from(u32::MAX) {
                        *value = fields
                            .next()
                            .ok_or(DirError::Damaged("a zip64 size missing"))?;
                    }
                    Ok(())
                };
                take(&mut size)?;
                take(&mut compressed)?;
                take(&mut local_header_offset)?;
            }
            extra = &extra[(4 + len).min(extra.len())..];
        }
        out.push(Entry {
            // Zips made on Windows separate folders with `\`.
            name: String::from_utf8_lossy(&cd[46..46 + name_len]).replace('\\', "/"),
            size,
            compressed,
            crc32: u32_at(cd, 16),
            method: u16_at(cd, 10),
            dos_date: u16_at(cd, 14),
            dos_time: u16_at(cd, 12),
            local_header_offset,
        });
        cd = &cd[end..];
    }
    Ok(out)
}

fn u16_at(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([b[at], b[at + 1]])
}

fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(b[at..at + 4].try_into().expect("four bytes"))
}

fn u64_at(b: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(b[at..at + 8].try_into().expect("eight bytes"))
}

#[cfg(test)]
mod tests {
    use std::io::{Cursor, Write as _};

    use utilake_provenance::stamp::AEST;

    use super::*;

    #[test]
    fn a_dos_time_out_of_range_is_no_instant() {
        let e = Entry {
            name: "a".into(),
            size: 0,
            compressed: 0,
            crc32: 0,
            method: 0,
            dos_date: 0,
            dos_time: 0,
            local_header_offset: 0,
        };
        assert_eq!(e.modified_text(), "1980-00-00 00:00:00");
        assert_eq!(e.modified_at(AEST), None);
    }

    /// Reads a zip in memory, counting the spans fed.
    fn read(bytes: &[u8]) -> (Result<Vec<Entry>, DirError>, usize) {
        let mut reader = DirectoryReader::new(bytes.len() as u64);
        let mut step = reader.next();
        let mut reads = 0;
        loop {
            match step {
                Step::Read { start, end } => {
                    reads += 1;
                    match reader.feed(bytes[start as usize..end as usize].to_vec()) {
                        Ok(s) => step = s,
                        Err(e) => return (Err(e), reads),
                    }
                }
                Step::Done(entries) => return (Ok(entries), reads),
            }
        }
    }

    fn zip(comment: &str) -> Vec<u8> {
        let mut w = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let at = zip::DateTime::from_date_and_time(2026, 10, 1, 12, 34, 56).unwrap();
        let opts = zip::write::SimpleFileOptions::default().last_modified_time(at);
        w.add_directory("d/", opts).unwrap();
        w.start_file("d/x.csv", opts).unwrap();
        w.write_all(&b"x,y\n".repeat(100)).unwrap();
        w.start_file(
            "s.bin",
            opts.compression_method(zip::CompressionMethod::Stored),
        )
        .unwrap();
        w.write_all(b"abc").unwrap();
        w.set_comment(comment).unwrap();
        w.finish().unwrap().into_inner()
    }

    #[test]
    fn lists_a_zip_from_its_tail() {
        let bytes = zip("");
        let (r, reads) = read(&bytes);
        let e = r.unwrap();
        assert_eq!(reads, 1, "the directory is in the tail");
        let names: Vec<_> = e.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["d/", "d/x.csv", "s.bin"]);
        assert!(e[0].is_dir() && !e[1].is_dir());
        assert_eq!((e[1].size, e[1].method), (400, 8));
        assert!(e[1].compressed < 400);
        assert_eq!(e[1].crc32, crc32(&b"x,y\n".repeat(100)));
        assert_eq!(e[1].modified_text(), "2026-10-01 12:34:56");
        assert_eq!(e[1].modified_fields(), (2026, 10, 1, 12, 34, 56));
        assert_eq!(
            e[1].modified_at(AEST).map(|t| t.to_string()).as_deref(),
            Some("2026-10-01T12:34:56+10:00")
        );
        assert_eq!((e[2].size, e[2].compressed, e[2].method), (3, 3, 0));
        // The local header offsets point at local headers; the stored
        // entry's data follows its header.
        for entry in &e[1..] {
            let at = entry.local_header_offset as usize;
            let len = local_header_len(&bytes[at..at + LOCAL_HEADER]).unwrap();
            assert_eq!(&bytes[at..at + 4], b"PK\x03\x04");
            assert_eq!(
                len,
                LOCAL_HEADER + entry.name.len() + extra_len(&bytes[at..])
            );
        }
        let s = &e[2];
        let at = s.local_header_offset as usize;
        let data = at + local_header_len(&bytes[at..at + LOCAL_HEADER]).unwrap();
        assert_eq!(&bytes[data..data + 3], b"abc");
        assert_eq!(dos_text(0, 0), "1980-00-00 00:00:00");
        assert_eq!(
            local_header_len(b"PK\x01\x02"),
            Err(DirError::Damaged(
                "no local header where the directory points"
            ))
        );
        // The same by seeking.
        let seeked = read_from(&mut Cursor::new(&bytes), bytes.len() as u64).unwrap();
        assert_eq!(seeked, e);
    }

    fn extra_len(local: &[u8]) -> usize {
        usize::from(u16::from_le_bytes([local[28], local[29]]))
    }

    fn crc32(bytes: &[u8]) -> u32 {
        let mut c = flate2::Crc::new();
        c.update(bytes);
        c.sum()
    }

    #[test]
    fn a_long_comment_puts_the_directory_outside_the_tail() {
        let (r, reads) = read(&zip(&"c".repeat(0xFFFF)));
        assert_eq!(r.unwrap().len(), 3);
        assert_eq!(reads, 2);
    }

    #[test]
    fn contents_of_a_file_and_of_what_is_not_a_zip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.zip");
        std::fs::write(&path, zip("")).unwrap();
        let files = contents(&path).unwrap();
        assert_eq!(files.len(), 2, "folders are left out");
        assert_eq!(files[1].0, "s.bin");
        std::fs::write(&path, b"text").unwrap();
        assert!(contents(&path).is_none());
        assert!(contents(&dir.path().join("missing.zip")).is_none());
        // A reader that reads short of the span fails as the disk.
        let mut short = Cursor::new(&b"PK\x05\x06"[..]);
        assert_eq!(
            read_from(&mut short, 100),
            Err(DirError::Damaged("the file could not be read"))
        );
    }

    /// A zip64 archive of one stored entry: its sizes and offset in the
    /// zip64 extra field, the directory found through the zip64 end record.
    fn zip64(comment_len: usize) -> Vec<u8> {
        let name = b"big.bin";
        let mut z = Vec::new();
        z.extend_from_slice(b"PK\x03\x04");
        z.extend_from_slice(&[0; 26]);
        z.extend_from_slice(b"data");
        let cd_at = z.len() as u64;
        z.extend_from_slice(b"PK\x01\x02");
        z.extend_from_slice(&[45, 0, 45, 0, 0, 0, 0, 0]);
        z.extend_from_slice(&[0, 0, 0x21, 0x5A]); // time 0, date 2025-01-01
        z.extend_from_slice(&7u32.to_le_bytes()); // crc
        z.extend_from_slice(&u32::MAX.to_le_bytes()); // compressed
        z.extend_from_slice(&u32::MAX.to_le_bytes()); // size
        z.extend_from_slice(&(name.len() as u16).to_le_bytes());
        z.extend_from_slice(&28u16.to_le_bytes()); // extra
        z.extend_from_slice(&[0; 10]); // comment length, disk, attributes
        z.extend_from_slice(&u32::MAX.to_le_bytes()); // local header offset
        z.extend_from_slice(name);
        z.extend_from_slice(&1u16.to_le_bytes());
        z.extend_from_slice(&24u16.to_le_bytes());
        z.extend_from_slice(&5_000_000_000u64.to_le_bytes());
        z.extend_from_slice(&4_000_000_000u64.to_le_bytes());
        z.extend_from_slice(&0u64.to_le_bytes());
        let cd_len = z.len() as u64 - cd_at;
        let z64_at = z.len() as u64;
        z.extend_from_slice(b"PK\x06\x06");
        z.extend_from_slice(&44u64.to_le_bytes());
        z.extend_from_slice(&[45, 0, 45, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        z.extend_from_slice(&1u64.to_le_bytes());
        z.extend_from_slice(&1u64.to_le_bytes());
        z.extend_from_slice(&cd_len.to_le_bytes());
        z.extend_from_slice(&cd_at.to_le_bytes());
        z.extend_from_slice(b"PK\x06\x07");
        z.extend_from_slice(&0u32.to_le_bytes());
        z.extend_from_slice(&z64_at.to_le_bytes());
        z.extend_from_slice(&1u32.to_le_bytes());
        z.extend_from_slice(b"PK\x05\x06");
        z.extend_from_slice(&[0xFF; 16]);
        z.extend_from_slice(&(comment_len as u16).to_le_bytes());
        z.extend(std::iter::repeat_n(b'c', comment_len));
        z
    }

    #[test]
    fn zip64_sizes_and_records() {
        let (r, reads) = read(&zip64(0));
        let e = r.unwrap();
        assert_eq!(reads, 1);
        assert_eq!((e[0].size, e[0].compressed), (5_000_000_000, 4_000_000_000));
        assert_eq!(e[0].local_header_offset, 0, "from the zip64 field");
        assert_eq!(e[0].modified_text(), "2025-01-01 00:00:00");
        // The zip64 end record and the directory both outside the tail.
        let mut padded = vec![0u8; 70_000];
        let small = zip64(0xFFFF);
        let shift = padded.len() as u64;
        // Move everything along: offsets in the records grow with it.
        let mut z = small.clone();
        let cd_at = 4 + 26 + 4;
        let z64_at = z.windows(4).position(|w| w == b"PK\x06\x06").unwrap();
        let loc = z.windows(4).position(|w| w == b"PK\x06\x07").unwrap();
        z[z64_at + 48..z64_at + 56].copy_from_slice(&(cd_at as u64 + shift).to_le_bytes());
        z[loc + 8..loc + 16].copy_from_slice(&(z64_at as u64 + shift).to_le_bytes());
        padded.extend_from_slice(&z);
        let (r, reads) = read(&padded);
        assert_eq!(r.unwrap()[0].name, "big.bin");
        assert_eq!(reads, 3);
    }

    #[test]
    fn not_a_zip_and_damaged_directories() {
        assert_eq!(read(b"just text").0, Err(DirError::NotAZip(9)));
        // The end record points past the file.
        let mut z = zip("");
        let eocd = z.len() - 22;
        z[eocd + 16..eocd + 20].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(matches!(read(&z).0, Err(DirError::Damaged(_))));
        // A record that is not one.
        let mut z = zip("");
        let cd = z.windows(4).position(|w| w == b"PK\x01\x02").unwrap();
        z[cd] = b'X';
        assert!(matches!(read(&z).0, Err(DirError::Damaged(_))));
        // A record longer than the directory.
        let mut z = zip("");
        let cd = z.windows(4).rposition(|w| w == b"PK\x01\x02").unwrap();
        z[cd + 28] = 0xFF;
        assert!(matches!(read(&z).0, Err(DirError::Damaged(_))));
        // A locator pointing at no zip64 end record.
        let mut z = zip64(0);
        let z64 = z.windows(4).position(|w| w == b"PK\x06\x06").unwrap();
        z[z64] = b'X';
        assert!(matches!(read(&z).0, Err(DirError::Damaged(_))));
        // A locator pointing past the file.
        let mut z = zip64(0);
        let loc = z.windows(4).position(|w| w == b"PK\x06\x07").unwrap();
        z[loc + 8..loc + 16].copy_from_slice(&u64::MAX.to_le_bytes());
        assert!(matches!(read(&z).0, Err(DirError::Damaged(_))));
        // Sizes marked as zip64 with no zip64 field for them.
        let mut z = zip64(0);
        let extra = z.windows(4).position(|w| w == b"big.").unwrap() + 7;
        z[extra..extra + 2].copy_from_slice(&9u16.to_le_bytes());
        let e = read(&z).0.unwrap();
        assert_eq!(
            e[0].size,
            u64::from(u32::MAX),
            "an unknown extra field is skipped"
        );
        for len in [0u16, 8, 16] {
            let mut z = zip64(0);
            z[extra + 2..extra + 4].copy_from_slice(&len.to_le_bytes());
            assert!(matches!(read(&z).0, Err(DirError::Damaged(_))), "{len}");
        }
        // Only one of the sizes too big for 32 bits: it takes the field.
        let cd = zip64(0)
            .windows(4)
            .position(|w| w == b"PK\x01\x02")
            .unwrap();
        let mut z = zip64(0);
        z[cd + 20..cd + 24].copy_from_slice(&9u32.to_le_bytes());
        let e = read(&z).0.unwrap();
        assert_eq!((e[0].size, e[0].compressed), (5_000_000_000, 9));
        let mut z = zip64(0);
        z[cd + 24..cd + 28].copy_from_slice(&9u32.to_le_bytes());
        let e = read(&z).0.unwrap();
        assert_eq!((e[0].size, e[0].compressed), (9, 5_000_000_000));
        assert_eq!(
            DirError::NotAZip(9).to_string(),
            "no end-of-central-directory record in the last 9 bytes: not a zip"
        );
        // Fed short, or fed after the end.
        let mut reader = DirectoryReader::new(10);
        assert_eq!(
            reader.feed(vec![0; 3]),
            Err(DirError::Short { wanted: 10, got: 3 })
        );
        let bytes = zip("");
        let mut reader = DirectoryReader::new(bytes.len() as u64);
        assert!(matches!(reader.feed(bytes.clone()), Ok(Step::Done(_))));
        assert!(matches!(reader.feed(bytes), Err(DirError::Damaged(_))));
    }

    #[test]
    fn windows_folders_read_as_slashes() {
        let mut w = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for (name, bytes) in [("M|S|t.csv", &b"x"[..]), ("M|", &b""[..])] {
            w.start_file(name, zip::write::SimpleFileOptions::default())
                .unwrap();
            w.write_all(bytes).unwrap();
        }
        let mut z = w.finish().unwrap().into_inner();
        for b in &mut z {
            if *b == b'|' {
                *b = b'\\';
            }
        }
        let names: Vec<String> = read(&z).0.unwrap().into_iter().map(|e| e.name).collect();
        assert_eq!(names, ["M/S/t.csv", "M/"]);
    }
}
