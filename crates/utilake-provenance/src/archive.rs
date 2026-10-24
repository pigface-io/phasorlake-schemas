//! The archive: every held file at a path named for its URL, with its
//! provenance beside it as `{name}.provenance.json`.
//!
//! A file is written once and never moved or rewritten by the code that
//! holds it. A later version AEMO serves under the same name is kept beside
//! the first as `{name}@{at}`, with its own record. Only a record changes,
//! and only by being written whole again: to a temporary file, synced, then
//! renamed over the old one.
//!
//! Records written before format 2 sit at `{name}.json`. They are read
//! where no `{name}.provenance.json` is, and a record written again at the
//! new name takes the old one's place.

use std::path::{Path, PathBuf};

use crate::layout;
use crate::provenance::{Provenance, ReadError};
use crate::stamp::Stamp;

/// What a record's name ends with.
pub const RECORD: &str = ".provenance.json";

/// What a format-1 record's name ends with.
pub const OLD_RECORD: &str = ".json";

/// A held file and its provenance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Held {
    /// The bytes.
    pub path: PathBuf,
    /// Where they came from.
    pub provenance: Provenance,
}

/// Why the archive could not be read or written.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The file system refused.
    #[error("{path}: {source}")]
    Io {
        /// The path.
        path: String,
        /// Why.
        source: std::io::Error,
    },
    /// A record that does not read.
    #[error("{path}: {source}")]
    Record {
        /// The record's path.
        path: String,
        /// Why.
        source: ReadError,
    },
}

fn io(path: &Path) -> impl Fn(std::io::Error) -> Error + '_ {
    move |source| Error::Io {
        path: path.display().to_string(),
        source,
    }
}

/// Whether a name in the archive is a provenance record.
#[must_use]
pub fn is_record(name: &str) -> bool {
    name.ends_with(RECORD)
}

/// Whether a name in the archive is neither a held file nor a record: a
/// download or record in progress (`.part`, `.part.resume`, `.part.json`,
/// `.tmp`), or the AppleDouble file macOS writes beside a file on a volume
/// without extended attributes (`._name`).
#[must_use]
pub fn is_noise(name: &str) -> bool {
    name.starts_with("._")
        || [".part", ".part.resume", ".part.json", ".tmp"]
            .iter()
            .any(|s| name.ends_with(s))
}

/// `path` with `suffix` appended to its name.
fn beside(path: &Path, suffix: &str) -> PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push(suffix);
    PathBuf::from(s)
}

/// The archive under `root`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Archive {
    root: PathBuf,
}

impl Archive {
    /// The archive at `root`.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// Where the archive is.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Where the bytes of `url` are kept ([`layout::local_segments`]).
    #[must_use]
    pub fn path_for(&self, url: &str) -> PathBuf {
        let mut path = self.root.clone();
        path.extend(layout::local_segments(url));
        path
    }

    /// `url` as a path with nothing dropped ([`layout::url_segments`]).
    #[must_use]
    pub fn url_path(&self, url: &str) -> PathBuf {
        let mut path = self.root.clone();
        path.extend(layout::url_segments(url));
        path
    }

    /// The record of the file at `path`.
    #[must_use]
    pub fn record_path(path: &Path) -> PathBuf {
        beside(path, RECORD)
    }

    /// Where a later version of the file at `path` is kept: beside it, as
    /// `{name}@{at}`, `at` the time AEMO listed that version with
    /// (`20261021T115300+1000`). The first version keeps the plain name.
    #[must_use]
    pub fn version_path(path: &Path, at: Stamp) -> PathBuf {
        beside(path, &format!("@{}", at.compact()))
    }

    /// Where a download of the file at `path` is written until it is whole.
    #[must_use]
    pub fn part_path(path: &Path) -> PathBuf {
        beside(path, ".part")
    }

    /// Notes in the record of the file at `path` that the listing no
    /// longer names each of `urls`. Whether the record changed; a file with
    /// no record changes nothing.
    pub fn gone<'a>(
        path: &Path,
        urls: impl IntoIterator<Item = &'a str>,
        at: Stamp,
    ) -> Result<bool, Error> {
        let Some(mut p) = Self::read(path)? else {
            return Ok(false);
        };
        let mut changed = false;
        for u in urls {
            changed |= p.gone(u, at);
        }
        if changed {
            Self::write(path, &p)?;
        }
        Ok(changed)
    }

    /// Reads the file a record describes again and holds it to the record.
    pub fn verify(held: &Held) -> Result<bool, Error> {
        let c = crate::content::Content::of_file(&held.path).map_err(io(&held.path))?;
        Ok(c.same(&held.provenance.content))
    }

    /// The record of the file at `path` among the names its folder holds,
    /// as `has` says: `{name}.provenance.json`, else a format-1
    /// `{name}.json`. No name is looked for on disk, so a walk that has read
    /// a folder finds each record without the look for an absent name that
    /// on exFAT reads the whole folder.
    pub fn record_where(path: &Path, has: impl Fn(&str) -> bool) -> Option<PathBuf> {
        let name = path.file_name()?.to_string_lossy();
        [RECORD, OLD_RECORD]
            .iter()
            .map(|s| format!("{name}{s}"))
            .find(|n| has(n))
            .map(|n| path.with_file_name(n))
    }

    /// The record at `record`, a path a walk or [`Archive::record_where`]
    /// found.
    pub fn read_record(record: &Path) -> Result<Provenance, Error> {
        let bytes = std::fs::read(record).map_err(io(record))?;
        Provenance::from_json(&bytes).map_err(|source| Error::Record {
            path: record.display().to_string(),
            source,
        })
    }

    /// The provenance of the file at `path`: its record, else its format-1
    /// record; none when it has neither.
    pub fn read(path: &Path) -> Result<Option<Provenance>, Error> {
        for record in [beside(path, RECORD), beside(path, OLD_RECORD)] {
            match std::fs::read(&record) {
                Ok(bytes) => {
                    return Provenance::from_json(&bytes).map(Some).map_err(|source| {
                        Error::Record {
                            path: record.display().to_string(),
                            source,
                        }
                    });
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(io(&record)(e)),
            }
        }
        Ok(None)
    }

    /// Writes the record of the file at `path` whole: to `{record}.tmp`,
    /// synced, renamed over the record. A format-1 record beside it that
    /// reads as one is then removed: the new record holds all it held.
    pub fn write(path: &Path, provenance: &Provenance) -> Result<(), Error> {
        use std::io::Write as _;
        let record = beside(path, RECORD);
        let tmp = beside(&record, ".tmp");
        let mut f = std::fs::File::create(&tmp).map_err(io(&tmp))?;
        f.write_all(provenance.to_json().as_bytes())
            .map_err(io(&tmp))?;
        rustix::fs::fsync(&f)
            .map_err(std::io::Error::from)
            .map_err(io(&tmp))?;
        drop(f);
        std::fs::rename(&tmp, &record).map_err(io(&record))?;
        let old = beside(path, OLD_RECORD);
        if std::fs::read(&old)
            .ok()
            .is_some_and(|b| Provenance::from_json(&b).is_ok())
        {
            std::fs::remove_file(&old).map_err(io(&old))?;
        }
        Ok(())
    }

    /// Every file held at `path`: the file there, then each `{name}@{at}`
    /// beside it, in name order, so in the order AEMO listed them. Two URLs
    /// can share a path (both listings of a report folder are one folder);
    /// their files never replace each other. A file is held only with a
    /// record that reads and gives its size: a record cut short, or bytes
    /// cut short, are not held, so they are fetched again.
    pub fn held_at(path: &Path) -> Result<Vec<Held>, Error> {
        let mut out = Vec::new();
        // A version is kept only beside a first file: with none there, the
        // folder (on exFAT, a scan of it) is not read.
        if std::fs::symlink_metadata(path).is_err() {
            return Ok(out);
        }
        out.extend(Self::held_file(path)?);
        let dir = path.parent().unwrap_or(Path::new("."));
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        let prefix = format!("{name}@");
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .map_err(io(dir))?
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with(&prefix) && !n.ends_with(OLD_RECORD) && !is_noise(n))
            .collect();
        names.sort();
        for n in names {
            out.extend(Self::held_file(&dir.join(n))?);
        }
        Ok(out)
    }

    /// The file at `path` with its provenance, when it has a record that
    /// reads and gives the file's size.
    fn held_file(path: &Path) -> Result<Option<Held>, Error> {
        let Ok(meta) = std::fs::metadata(path) else {
            return Ok(None);
        };
        match Self::read(path) {
            Ok(Some(provenance)) if provenance.content.size == meta.len() => Ok(Some(Held {
                path: path.to_path_buf(),
                provenance,
            })),
            Ok(_) | Err(Error::Record { .. }) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// The file held for `url`: the first at its path whose provenance
    /// names it.
    pub fn lookup(&self, url: &str) -> Result<Option<Held>, Error> {
        Ok(Self::held_at(&self.path_for(url))?
            .into_iter()
            .find(|h| h.provenance.names(url)))
    }

    /// Every held file under `dir`, at any depth: a file with a record
    /// beside it (`{name}.provenance.json`, or a format-1 `{name}.json`
    /// where its `{name}` is there too). Paths in name order; no record is
    /// read.
    pub fn walk(dir: &Path) -> Result<Vec<PathBuf>, Error> {
        Ok(Self::walk_records(dir)?
            .into_iter()
            .map(|(p, _)| p)
            .collect())
    }

    /// The same, each file with the path of its record.
    pub fn walk_records(dir: &Path) -> Result<Vec<(PathBuf, PathBuf)>, Error> {
        let mut out = Vec::new();
        let mut stack = vec![dir.to_path_buf()];
        while let Some(d) = stack.pop() {
            let mut files = std::collections::BTreeSet::new();
            for e in std::fs::read_dir(&d).map_err(io(&d))?.flatten() {
                let name = e.file_name().to_string_lossy().into_owned();
                if e.file_type().is_ok_and(|t| t.is_dir()) {
                    stack.push(e.path());
                } else if !is_noise(&name) {
                    files.insert(name);
                }
            }
            for name in &files {
                // A record, or a format-1 record of a file beside it.
                let old_record = name
                    .strip_suffix(OLD_RECORD)
                    .is_some_and(|f| files.contains(f));
                if is_record(name) || old_record {
                    continue;
                }
                let path = d.join(name);
                if let Some(record) = Self::record_where(&path, |n| files.contains(n)) {
                    out.push((path, record));
                }
            }
        }
        out.sort();
        Ok(out)
    }
}

#[cfg(test)]
mod tests;
