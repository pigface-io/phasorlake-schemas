//! Files the census writes and reads whole.

use std::cell::RefCell;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write as _};
use std::path::Path;
use std::rc::Rc;

use sha2::Digest as _;

/// Writes `bytes` to `path` through a temporary file beside it, renamed
/// into place: a reader sees the old file or the new one, never part of
/// either. Missing directories are made.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(dir)?;
    let mut tmp = tempfile::NamedTempFile::new_in(dir)?;
    tmp.write_all(bytes)?;
    tmp.persist(path).map_err(|e| e.error)?;
    Ok(())
}

/// The SHA-256 of a stream, lower-case hex, and its length in bytes.
pub fn sha256_of(mut r: impl Read) -> std::io::Result<(String, u64)> {
    let mut h = sha2::Sha256::new();
    let mut buf = vec![0u8; 8 << 20];
    let mut len = 0u64;
    loop {
        let n = r.read(&mut buf)?;
        if n == 0 {
            return Ok((hex(&h.finalize()), len));
        }
        h.update(&buf[..n]);
        len += n as u64;
    }
}

/// How much of a file has been hashed, in order from its start.
#[derive(Default)]
struct Hashed {
    hasher: sha2::Sha256,
    upto: u64,
}

/// A file read once for both its contents and its SHA-256: every byte read
/// in order from the start is hashed as it goes past, so a reader that
/// streams the file (the zip reader streams its entries) hashes nearly all
/// of it, and [`Tally::finish`] reads only what it skipped (a zip's
/// directory at the end).
pub struct HashedFile {
    file: File,
    pos: u64,
    hashed: Rc<RefCell<Hashed>>,
}

/// The hash of a [`HashedFile`], finished once the reading is done.
pub struct Tally(Rc<RefCell<Hashed>>);

impl HashedFile {
    /// Opens `path`, and the tally of its hash.
    pub fn open(path: &Path) -> std::io::Result<(Self, Tally)> {
        let hashed = Rc::new(RefCell::new(Hashed::default()));
        let file = File::open(path)?;
        Ok((
            Self {
                file,
                pos: 0,
                hashed: Rc::clone(&hashed),
            },
            Tally(hashed),
        ))
    }
}

impl Read for HashedFile {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.file.read(buf)?;
        let end = self.pos + n as u64;
        let mut h = self.hashed.borrow_mut();
        if self.pos <= h.upto && h.upto < end {
            let from = usize::try_from(h.upto - self.pos).unwrap_or(n);
            h.hasher.update(&buf[from..n]);
            h.upto = end;
        }
        self.pos = end;
        Ok(n)
    }
}

impl Seek for HashedFile {
    fn seek(&mut self, to: SeekFrom) -> std::io::Result<u64> {
        self.pos = self.file.seek(to)?;
        Ok(self.pos)
    }
}

impl Tally {
    /// The SHA-256 of the file at `path`, lower-case hex, and its length:
    /// what was hashed while it was read, and the rest read now.
    pub fn finish(self, path: &Path) -> std::io::Result<(String, u64)> {
        let Hashed { mut hasher, upto } = std::mem::take(&mut *self.0.borrow_mut());
        let mut f = File::open(path)?;
        f.seek(SeekFrom::Start(upto))?;
        let mut buf = vec![0u8; 1 << 20];
        let mut len = upto;
        loop {
            let n = f.read(&mut buf)?;
            if n == 0 {
                return Ok((hex(&hasher.finalize()), len));
            }
            hasher.update(&buf[..n]);
            len += n as u64;
        }
    }
}

/// Bytes as lower-case hex.
pub fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes.iter().fold(String::with_capacity(64), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_whole_and_hashes_streams() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a/b/c.txt");
        write_atomic(&p, b"one").unwrap();
        write_atomic(&p, b"two").unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), b"two");
        assert_eq!(std::fs::read_dir(p.parent().unwrap()).unwrap().count(), 1);
        assert!(write_atomic(&p.join("under-a-file"), b"x").is_err());
        assert!(write_atomic(dir.path(), b"x").is_err(), "a directory");
        let (sha, len) = sha256_of(&b"abc"[..]).unwrap();
        assert_eq!(
            sha,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(len, 3);
        assert_eq!(sha, crate::ctl::sha256(b"abc"));
    }

    #[test]
    fn a_file_is_hashed_as_it_is_read_and_what_was_skipped_after() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("f.bin");
        let bytes: Vec<u8> = (0..5000u32).map(|i| (i % 251) as u8).collect();
        std::fs::write(&p, &bytes).unwrap();
        let whole = sha256_of(&bytes[..]).unwrap();
        // Read in order: everything is hashed on the way.
        let (mut f, tally) = HashedFile::open(&p).unwrap();
        let mut sink = Vec::new();
        f.read_to_end(&mut sink).unwrap();
        assert_eq!(tally.finish(&p).unwrap(), whole);
        // The end first (as a zip reader starts), then the start, then a
        // jump ahead, a step back over hashed bytes, and stopping early.
        let (mut f, tally) = HashedFile::open(&p).unwrap();
        let mut buf = [0u8; 700];
        f.seek(SeekFrom::End(-100)).unwrap();
        f.read_exact(&mut buf[..100]).unwrap();
        f.seek(SeekFrom::Start(0)).unwrap();
        f.read_exact(&mut buf).unwrap();
        f.seek(SeekFrom::Start(2000)).unwrap();
        f.read_exact(&mut buf).unwrap();
        f.seek(SeekFrom::Start(300)).unwrap();
        f.read_exact(&mut buf).unwrap();
        assert_eq!(tally.finish(&p).unwrap(), whole);
        // Nothing read at all.
        let (_, tally) = HashedFile::open(&p).unwrap();
        assert_eq!(tally.finish(&p).unwrap(), whole);
        assert!(HashedFile::open(&dir.path().join("none")).is_err());
        let (_, tally) = HashedFile::open(&p).unwrap();
        std::fs::remove_file(&p).unwrap();
        assert!(tally.finish(&p).is_err(), "gone before the tail was read");
    }
}
