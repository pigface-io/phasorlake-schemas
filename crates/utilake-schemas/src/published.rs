//! What an export published: every table's history and every schema
//! document under `nem/`, read from a git revision or from a directory, so
//! [`crate::compat`] and [`crate::diff`] can hold one to the other.

use std::collections::BTreeMap;
use std::path::Path;

use serde::Deserialize as _;
use utilake_iceberg::History;

use crate::export::OWNED;
use crate::fold::Baseline;

/// The manifest, read for its data version.
const MANIFEST: &str = "manifest.json";

/// The git a revision is read from.
pub trait Git {
    /// Every file under `dir` at `rev`, by path from the repository root;
    /// none when the revision has no such directory. An error when the
    /// revision cannot be read.
    fn list(&self, rev: &str, dir: &str) -> std::io::Result<Vec<String>>;

    /// The bytes of `path` at `rev`.
    fn show(&self, rev: &str, path: &str) -> std::io::Result<Vec<u8>>;
}

/// Why a published set could not be read.
#[derive(Debug, thiserror::Error)]
pub enum PublishedError {
    /// Git could not read the revision or a file of it.
    #[error("git could not read {path} at {rev}.")]
    Git {
        /// The revision.
        rev: String,
        /// The directory or file.
        path: String,
        /// Cause.
        #[source]
        source: std::io::Error,
    },
    /// A file of the working tree that cannot be read.
    #[error("The published file {path} cannot be read.")]
    Read {
        /// The file or directory.
        path: String,
        /// Cause.
        #[source]
        source: std::io::Error,
    },
    /// A table document without a history in it.
    #[error("The table document {path} ({at}) holds no history: {message}")]
    Table {
        /// The file.
        path: String,
        /// The revision, or `working tree`.
        at: String,
        /// What is wrong.
        message: String,
    },
}

/// The published histories and schema documents, and the data version
/// `manifest.json` names (the first, when it names none).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Published {
    /// Each `nem/{TABLE}/table.json`'s history, by table.
    pub tables: BTreeMap<String, History>,
    /// Each `nem/{TABLE}/schema-*.json`, by path.
    pub schemas: BTreeMap<String, Vec<u8>>,
    /// The data version, and why it began.
    pub baseline: Baseline,
}

impl Default for Published {
    fn default() -> Self {
        Self {
            tables: BTreeMap::new(),
            schemas: BTreeMap::new(),
            baseline: Baseline::FIRST,
        }
    }
}

/// The baseline `manifest.json` names; the first when it names none.
fn baseline_of(manifest: &[u8], at: &str) -> Result<Baseline, PublishedError> {
    let bad = |message: String| PublishedError::Table {
        path: MANIFEST.to_owned(),
        at: at.to_owned(),
        message,
    };
    let doc: serde_json::Value =
        serde_json::from_slice(manifest).map_err(|e| bad(e.to_string()))?;
    match doc.get("baseline") {
        None => Ok(Baseline::FIRST),
        Some(b) => serde_json::from_value(b.clone()).map_err(|e| bad(e.to_string())),
    }
}

/// Where a path under `nem/` falls: a table document, a schema document,
/// or neither.
enum Kind<'a> {
    Table(&'a str),
    Schema,
    Other,
}

fn kind(path: &str) -> Kind<'_> {
    let parts: Vec<&str> = path.split('/').collect();
    match parts.as_slice() {
        [OWNED, table, "table.json"] => Kind::Table(table),
        [OWNED, _, file] if file.starts_with("schema-") && file.ends_with(".json") => Kind::Schema,
        _ => Kind::Other,
    }
}

impl Published {
    /// From each file's path under the root and its bytes; `at` names
    /// where they came from, for an error.
    fn from_files(
        files: impl IntoIterator<Item = (String, Vec<u8>)>,
        at: &str,
    ) -> Result<Self, PublishedError> {
        let mut out = Self::default();
        for (path, bytes) in files {
            if path == MANIFEST {
                out.baseline = baseline_of(&bytes, at)?;
                continue;
            }
            match kind(&path) {
                Kind::Table(table) => {
                    let bad = |message: String| PublishedError::Table {
                        path: path.clone(),
                        at: at.to_owned(),
                        message,
                    };
                    let doc: serde_json::Value =
                        serde_json::from_slice(&bytes).map_err(|e| bad(e.to_string()))?;
                    let iceberg = doc.get("iceberg").ok_or_else(|| bad("no iceberg".into()))?;
                    let h = History::deserialize(iceberg).map_err(|e| bad(e.to_string()))?;
                    out.tables.insert(table.to_owned(), h);
                }
                Kind::Schema => {
                    out.schemas.insert(path, bytes);
                }
                Kind::Other => {}
            }
        }
        Ok(out)
    }

    /// What `rev` published; empty when it has no `nem/`.
    pub fn at(git: &dyn Git, rev: &str) -> Result<Self, PublishedError> {
        let failed = |path: &str, source| PublishedError::Git {
            rev: rev.to_owned(),
            path: path.to_owned(),
            source,
        };
        let mut files = Vec::new();
        for path in git.list(rev, OWNED).map_err(|e| failed(OWNED, e))? {
            if matches!(kind(&path), Kind::Other) {
                continue;
            }
            let bytes = git.show(rev, &path).map_err(|e| failed(&path, e))?;
            files.push((path, bytes));
        }
        // A revision before the manifest named a version is of the first.
        if let Ok(bytes) = git.show(rev, MANIFEST) {
            files.push((MANIFEST.to_owned(), bytes));
        }
        Self::from_files(files, rev)
    }

    /// What the working tree under `root` holds.
    pub fn read(root: &Path) -> Result<Self, PublishedError> {
        let dir = root.join(OWNED);
        let read = |path: &Path, source| PublishedError::Read {
            path: path.display().to_string(),
            source,
        };
        let mut files = Vec::new();
        let tables = match std::fs::read_dir(&dir) {
            Ok(rd) => rd
                .map(|e| e.map(|e| e.path()))
                .collect::<std::io::Result<Vec<_>>>()
                .map_err(|e| read(&dir, e))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(e) => return Err(read(&dir, e)),
        };
        for t in tables.iter().filter(|t| t.is_dir()) {
            for entry in std::fs::read_dir(t).map_err(|e| read(t, e))? {
                let path = entry.map_err(|e| read(t, e))?.path();
                // The table's Iceberg metadata is a directory of its own.
                if path.is_dir() {
                    continue;
                }
                let rel = path
                    .strip_prefix(root)
                    .unwrap_or(&path)
                    .to_string_lossy()
                    .replace('\\', "/");
                let bytes = std::fs::read(&path).map_err(|e| read(&path, e))?;
                files.push((rel, bytes));
            }
        }
        let manifest = root.join(MANIFEST);
        match std::fs::read(&manifest) {
            Ok(bytes) => files.push((MANIFEST.to_owned(), bytes)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(read(&manifest, e)),
        }
        Self::from_files(files, "working tree")
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::cell::RefCell;

    use super::*;

    /// A git of revisions held in memory: each revision's files by path.
    #[derive(Default)]
    pub(crate) struct MemGit {
        pub revs: BTreeMap<String, BTreeMap<String, Vec<u8>>>,
        /// Paths whose `show` fails.
        pub broken: RefCell<Vec<String>>,
    }

    impl MemGit {
        /// A revision holding every file under `root`'s `nem/`.
        pub fn commit(&mut self, rev: &str, root: &Path) {
            let mut files = BTreeMap::new();
            let manifest = root.join(MANIFEST);
            if let Ok(bytes) = std::fs::read(&manifest) {
                files.insert(MANIFEST.to_owned(), bytes);
            }
            for p in crate::census::files_under(&root.join(OWNED)).unwrap_or_default() {
                let rel = p
                    .strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/");
                files.insert(rel, std::fs::read(&p).unwrap());
            }
            self.revs.insert(rev.to_owned(), files);
        }
    }

    impl Git for MemGit {
        fn list(&self, rev: &str, dir: &str) -> std::io::Result<Vec<String>> {
            let files = self
                .revs
                .get(rev)
                .ok_or_else(|| std::io::Error::other(format!("unknown revision {rev}")))?;
            let prefix = format!("{dir}/");
            Ok(files
                .keys()
                .filter(|p| p.starts_with(&prefix))
                .cloned()
                .collect())
        }

        fn show(&self, rev: &str, path: &str) -> std::io::Result<Vec<u8>> {
            if self.broken.borrow().iter().any(|b| b == path) {
                return Err(std::io::Error::other("broken"));
            }
            Ok(self.revs[rev][path].clone())
        }
    }

    #[test]
    fn reads_a_revision_and_the_working_tree_alike() {
        let dir = crate::export::tests::exported();
        let root = dir.path();
        std::fs::write(root.join("nem/notes.txt"), "not a table").unwrap();
        std::fs::write(root.join("nem/DISPATCH_UNIT_SCADA/README.md"), "x").unwrap();
        let mut git = MemGit::default();
        git.commit("base", root);
        let now = Published::read(root).unwrap();
        assert_eq!(now.tables.len(), 1);
        assert_eq!(now.schemas.len(), 2);
        assert_eq!(now.baseline, Baseline::FIRST);
        assert_eq!(Published::at(&git, "base").unwrap(), now);
        // The manifest names the data version; one that names none, or
        // is gone, is of the first; one that cannot be read is an error.
        let manifest = root.join(MANIFEST);
        std::fs::write(
            &manifest,
            r#"{"baseline": {"version": 2, "reason": "why"}}"#,
        )
        .unwrap();
        assert_eq!(Published::read(root).unwrap().baseline.version, 2);
        std::fs::write(&manifest, "{}").unwrap();
        assert_eq!(Published::read(root).unwrap().baseline.version, 1);
        std::fs::write(&manifest, r#"{"baseline": 3}"#).unwrap();
        assert!(Published::read(root).is_err());
        std::fs::write(&manifest, "[").unwrap();
        assert!(Published::read(root).is_err());
        std::fs::remove_file(&manifest).unwrap();
        assert_eq!(Published::read(root).unwrap().baseline.version, 1);
        git.broken.borrow_mut().push(MANIFEST.into());
        assert_eq!(Published::at(&git, "base").unwrap().baseline.version, 1);
        git.broken.borrow_mut().clear();
        std::fs::create_dir(&manifest).unwrap();
        assert!(Published::read(root).is_err());
        std::fs::remove_dir(&manifest).unwrap();
        crate::export::write(
            root,
            &crate::export::Export::load(root, crate::export::DEFAULT_BASE_URL)
                .unwrap()
                .render(),
        )
        .unwrap();

        let e = Published::at(&git, "nope").unwrap_err();
        assert_eq!(e.to_string(), "git could not read nem at nope.");
        git.broken
            .borrow_mut()
            .push("nem/DISPATCH_UNIT_SCADA/table.json".into());
        let e = Published::at(&git, "base").unwrap_err();
        assert!(e.to_string().contains("table.json at base"), "{e}");

        // A table document without its history, or not JSON.
        let table = root.join("nem/DISPATCH_UNIT_SCADA/table.json");
        std::fs::write(&table, "{}").unwrap();
        let e = Published::read(root).unwrap_err();
        assert!(
            e.to_string()
                .ends_with("(working tree) holds no history: no iceberg"),
            "{e}"
        );
        std::fs::write(&table, "[").unwrap();
        assert!(Published::read(root).is_err());
        std::fs::write(&table, r#"{"iceberg": {}}"#).unwrap();
        assert!(Published::read(root).is_err());

        // A directory where a document is holds nothing (the metadata
        // directory is one); a document that cannot be read, or a file
        // where nem/ is, is an error.
        std::fs::remove_file(&table).unwrap();
        std::fs::create_dir(&table).unwrap();
        assert!(Published::read(root).unwrap().tables.is_empty());
        std::fs::remove_dir(&table).unwrap();
        {
            use std::os::unix::fs::PermissionsExt as _;
            let doc = root.join("nem/DISPATCH_UNIT_SCADA/schema-2e75294ff617bace.json");
            std::fs::set_permissions(&doc, std::fs::Permissions::from_mode(0o000)).unwrap();
            let e = Published::read(root).unwrap_err();
            assert!(e.to_string().starts_with("The published file "), "{e}");
            std::fs::set_permissions(&doc, std::fs::Permissions::from_mode(0o644)).unwrap();
        }
        let flat = tempfile::tempdir().unwrap();
        std::fs::write(flat.path().join("nem"), "a file").unwrap();
        assert!(Published::read(flat.path()).is_err());
        let empty = tempfile::tempdir().unwrap();
        assert_eq!(Published::read(empty.path()).unwrap(), Published::default());
    }
}
