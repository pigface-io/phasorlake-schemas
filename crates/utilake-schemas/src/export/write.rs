//! Writing the rendered files under a root, or checking a root against
//! them.

use std::path::{Path, PathBuf};

use super::{ExportError, OWNED, Rendered};

/// What one write did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Written {
    /// Files new or with other bytes.
    pub written: usize,
    /// Files already as rendered.
    pub kept: usize,
    /// Files under `nem/` the render did not produce, removed.
    pub removed: usize,
}

/// A file a check found not as rendered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stale {
    /// Its path from the root.
    pub path: String,
    /// `missing`, `differs` or `extra`.
    pub why: &'static str,
}

impl std::fmt::Display for Stale {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} {}", self.path, self.why)
    }
}

/// Writes every rendered file under `root`, each whole through a temporary
/// file renamed into place, and removes every other file under `nem/` and
/// the directories it leaves empty.
pub fn write(root: &Path, r: &Rendered) -> Result<Written, ExportError> {
    let mut done = Written::default();
    for (rel, bytes) in &r.files {
        let path = root.join(rel);
        if std::fs::read(&path).ok().as_deref() == Some(bytes.as_slice()) {
            done.kept += 1;
            continue;
        }
        crate::files::write_atomic(&path, bytes).map_err(|e| ExportError::io(&path, e))?;
        done.written += 1;
    }
    for path in extras(root, r)? {
        std::fs::remove_file(&path).map_err(|e| ExportError::io(&path, e))?;
        done.removed += 1;
        prune(&path, &root.join(OWNED));
    }
    Ok(done)
}

/// Every file under `root` not as rendered, by path: rendered and
/// `missing` or with other bytes (`differs`), or under `nem/` and not
/// rendered (`extra`). Writes nothing.
pub fn check(root: &Path, r: &Rendered) -> Result<Vec<Stale>, ExportError> {
    let mut stale = Vec::new();
    for (rel, bytes) in &r.files {
        let path = root.join(rel);
        let why = match std::fs::read(&path) {
            Ok(now) if now == *bytes => continue,
            Ok(_) => "differs",
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => "missing",
            Err(e) => return Err(ExportError::io(&path, e)),
        };
        stale.push(Stale {
            path: rel.clone(),
            why,
        });
    }
    for path in extras(root, r)? {
        stale.push(Stale {
            path: relative(root, &path),
            why: "extra",
        });
    }
    stale.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(stale)
}

fn relative(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

/// The files under `root/nem/` the render did not produce, sorted.
fn extras(root: &Path, r: &Rendered) -> Result<Vec<PathBuf>, ExportError> {
    let mut found = Vec::new();
    walk(&root.join(OWNED), &mut found)?;
    found.retain(|p| !r.files.contains_key(&relative(root, p)));
    found.sort();
    Ok(found)
}

fn walk(dir: &Path, into: &mut Vec<PathBuf>) -> Result<(), ExportError> {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(ExportError::io(dir, e)),
    };
    for entry in entries {
        let path = entry.map_err(|e| ExportError::io(dir, e))?.path();
        if path.is_dir() {
            walk(&path, into)?;
        } else {
            into.push(path);
        }
    }
    Ok(())
}

/// Removes the directories above `file` that its removal left empty, up
/// to `stop`, which stays.
fn prune(file: &Path, stop: &Path) {
    let mut dir = file.parent();
    while let Some(d) = dir.filter(|d| *d != stop) {
        if std::fs::remove_dir(d).is_err() {
            return;
        }
        dir = d.parent();
    }
}
