//! Files the census writes and reads whole.

use std::io::{Read, Write as _};
use std::path::Path;

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
    use sha2::Digest as _;
    let mut h = sha2::Sha256::new();
    let mut buf = vec![0u8; 1 << 16];
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
}
