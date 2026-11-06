//! Files that exist whole or not at all. Bytes go to a `.part` sibling,
//! which is renamed over the target once written (and synced, when asked),
//! so a reader sees the old file or the new one, never part of either; the
//! sibling is removed if anything fails. Missing directories are made.
//! Every tool writes its records and documents this way; the mirror's
//! downloads, which resume, keep their own `.part` protocol.

use std::io::Write as _;
use std::path::{Path, PathBuf};

/// Whether a write waits for the disk before the rename.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Durability {
    /// `fsync` the bytes before the rename: the file survives a crash.
    Synced,
    /// No sync: a batch the caller flushes afterwards, or a cache that may
    /// be rebuilt.
    Unsynced,
}

/// The `.part` sibling of `path`.
fn part_of(path: &Path) -> std::io::Result<PathBuf> {
    let name = path.file_name().and_then(|n| n.to_str()).ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("{} names no file", path.display()),
        )
    })?;
    Ok(path.with_file_name(format!("{name}.part")))
}

/// Writes `bytes` to `path` by way of `{path}.part` and a rename, creating
/// the parent directories; synced to disk first when `durability` says so.
pub fn write_atomic(path: &Path, bytes: &[u8], durability: Durability) -> std::io::Result<()> {
    let part = part_of(path)?;
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)?;
    }
    let write = (|| -> std::io::Result<()> {
        let mut f = std::fs::File::create(&part)?;
        f.write_all(bytes)?;
        if durability == Durability::Synced {
            rustix::fs::fsync(&f).map_err(std::io::Error::from)?;
        }
        drop(f);
        std::fs::rename(&part, path)
    })();
    if write.is_err() {
        let _ = std::fs::remove_file(&part);
    }
    write
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_whole_in_new_directories_either_way_and_leaves_no_part() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a/b/c.json");
        write_atomic(&path, b"one", Durability::Synced).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"one");
        write_atomic(&path, b"two", Durability::Unsynced).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"two");
        assert!(!dir.path().join("a/b/c.json.part").exists());
        // A bare name writes in the working directory's terms: no parent to make.
        assert_eq!(
            part_of(Path::new("c.json")).unwrap(),
            Path::new("c.json.part")
        );
    }

    #[test]
    fn a_path_naming_no_file_is_refused_and_a_failed_rename_leaves_no_part() {
        let dir = tempfile::tempdir().unwrap();
        let e = write_atomic(Path::new("/"), b"x", Durability::Unsynced).unwrap_err();
        assert_eq!(e.kind(), std::io::ErrorKind::InvalidInput);
        // The target is a directory: the rename fails and the part is gone.
        let target = dir.path().join("taken");
        std::fs::create_dir_all(target.join("inside")).unwrap();
        assert!(write_atomic(&target, b"x", Durability::Synced).is_err());
        assert!(!dir.path().join("taken.part").exists());
        assert!(target.join("inside").is_dir());
    }
}
