//! What an export published: every table's history, read from its table
//! metadata, and every schema document under each market's directory
//! (`nem/`, `wem/`, `gas/`), from a git revision or from a directory, so
//! [`crate::compat`] and [`crate::diff`] can hold one to the other. A
//! table is keyed by its directory, `{market}/{TABLE}`. A revision from
//! before the metadata carried the history (format 2) is not read: the
//! format change has been crossed and every published table carries its
//! history.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use phasor_iceberg::table::TableMetadata;
use phasor_iceberg::{History, from_metadata};

use crate::export::{METADATA, owned};
use crate::fold::Baseline;
use phasor_iceberg::Market;

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
    #[error("git could not read {path} at {rev}")]
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
    #[error("the published file {path} cannot be read")]
    Read {
        /// The file or directory.
        path: String,
        /// Cause.
        #[source]
        source: std::io::Error,
    },
    /// A table document that is not a published table.
    #[error("the table document {path} ({at}) holds no history: {message}")]
    Table {
        /// The file.
        path: String,
        /// The revision, or `working tree`.
        at: String,
        /// What is wrong.
        message: String,
    },
}

/// The published histories and schema documents, and each market's data
/// version as `manifest.json` names it (the first, when it names none).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Published {
    /// Each table's history, by its directory `{market}/{TABLE}`.
    pub tables: BTreeMap<String, History>,
    /// Each `{market}/{TABLE}/schema-*.json`, by path.
    pub schemas: BTreeMap<String, Vec<u8>>,
    /// Each market's data version, and why it began; a market absent is
    /// at the first.
    pub baselines: BTreeMap<Market, Baseline>,
    /// Markets whose tables and documents were not read: re-based since,
    /// so nothing of theirs is held ([`Published::held_to`]).
    pub unread: BTreeSet<Market>,
}

impl Published {
    /// The data version of `market`.
    pub fn baseline(&self, market: Market) -> Baseline {
        self.baselines
            .get(&market)
            .cloned()
            .unwrap_or(Baseline::FIRST)
    }

    /// The markets with a table, read or not.
    pub fn markets(&self) -> impl Iterator<Item = Market> + '_ {
        Market::ALL.into_iter().filter(|m| {
            self.unread.contains(m) || self.tables.keys().any(|k| k.starts_with(&format!("{m}/")))
        })
    }
}

/// The baselines `manifest.json` names, per market: under `markets`, or,
/// in a manifest from before there were other markets, at the top for
/// the NEM. The first when it names none.
fn baseline_of(manifest: &[u8], at: &str) -> Result<BTreeMap<Market, Baseline>, PublishedError> {
    let bad = |message: String| PublishedError::Table {
        path: MANIFEST.to_owned(),
        at: at.to_owned(),
        message,
    };
    let doc: serde_json::Value =
        serde_json::from_slice(manifest).map_err(|e| bad(e.to_string()))?;
    let mut out = BTreeMap::new();
    if let Some(markets) = doc.get("markets").and_then(|m| m.as_object()) {
        for (id, entry) in markets {
            let market = Market::parse(id).ok_or_else(|| bad(format!("{id:?} is not a market")))?;
            if let Some(b) = entry.get("baseline") {
                let b = serde_json::from_value(b.clone()).map_err(|e| bad(e.to_string()))?;
                out.insert(market, b);
            }
        }
    } else if let Some(b) = doc.get("baseline") {
        let b = serde_json::from_value(b.clone()).map_err(|e| bad(e.to_string()))?;
        out.insert(Market::Nem, b);
    }
    Ok(out)
}

/// Where a path under a market's directory falls: a table's metadata, a
/// schema document, or neither.
enum Kind {
    Metadata,
    Schema,
    Other,
}

fn kind(path: &str) -> Kind {
    let parts: Vec<&str> = path.split('/').collect();
    match parts.as_slice() {
        [m, _, "metadata", "v1.metadata.json"] if Market::parse(m).is_some() => Kind::Metadata,
        [m, _, file]
            if Market::parse(m).is_some()
                && file.starts_with("schema-")
                && file.ends_with(".json") =>
        {
            Kind::Schema
        }
        _ => Kind::Other,
    }
}

/// The directory of the table a path names: `{market}/{TABLE}`.
fn dir_of(path: &str) -> String {
    path.splitn(3, '/').take(2).collect::<Vec<_>>().join("/")
}

impl Published {
    /// From each file's path under the root and its bytes; `at` names
    /// where they came from, for an error.
    /// A market `skip` names is left unread: its files are noted, not
    /// parsed.
    fn from_files(
        files: Vec<(String, Vec<u8>)>,
        at: &str,
        skip: impl Fn(&BTreeMap<Market, Baseline>, Market) -> bool,
    ) -> Result<Self, PublishedError> {
        let mut out = Self::default();
        if let Some((_, bytes)) = files.iter().find(|(p, _)| p == MANIFEST) {
            out.baselines = baseline_of(bytes, at)?;
        }
        let mut metadata: BTreeMap<String, (String, TableMetadata)> = BTreeMap::new();
        for (path, bytes) in files {
            let market = path.split('/').next().and_then(Market::parse);
            if let Some(m) = market.filter(|m| skip(&out.baselines, *m))
                && !matches!(kind(&path), Kind::Other)
            {
                out.unread.insert(m);
                continue;
            }
            match kind(&path) {
                Kind::Metadata => {
                    let m: TableMetadata =
                        serde_json::from_slice(&bytes).map_err(|e| PublishedError::Table {
                            path: path.clone(),
                            at: at.to_owned(),
                            message: e.to_string(),
                        })?;
                    metadata.insert(dir_of(&path), (path, m));
                }
                Kind::Schema => {
                    out.schemas.insert(path, bytes);
                }
                Kind::Other => {}
            }
        }
        for (table, (path, m)) in metadata {
            let bad = |path: &str, message: String| PublishedError::Table {
                path: path.to_owned(),
                at: at.to_owned(),
                message,
            };
            // Metadata from before the format carried the history is not
            // read: the format change has been crossed.
            if !m.properties.contains_key("phasor.format") {
                return Err(bad(
                    &path,
                    "no phasor.format: published before the metadata carried the history".into(),
                ));
            }
            let p = from_metadata(&m).map_err(|e| bad(&path, e.to_string()))?;
            out.tables.insert(table, p.history);
        }
        Ok(out)
    }

    /// What `rev` published; empty when it has no market directory.
    pub fn at(git: &dyn Git, rev: &str) -> Result<Self, PublishedError> {
        Self::at_skipping(git, rev, |_, _| false)
    }

    /// What `rev` published, less the tables of each market whose data
    /// version `now` has re-based: those are held to nothing, so they are
    /// not read, and may be in a form `now` no longer reads.
    pub fn held_to(git: &dyn Git, rev: &str, now: &Published) -> Result<Self, PublishedError> {
        Self::at_skipping(git, rev, |base, m| {
            base.get(&m).cloned().unwrap_or(Baseline::FIRST).version != now.baseline(m).version
        })
    }

    fn at_skipping(
        git: &dyn Git,
        rev: &str,
        skip: impl Fn(&BTreeMap<Market, Baseline>, Market) -> bool,
    ) -> Result<Self, PublishedError> {
        let failed = |path: &str, source| PublishedError::Git {
            rev: rev.to_owned(),
            path: path.to_owned(),
            source,
        };
        let mut files = Vec::new();
        for dir in owned() {
            for path in git.list(rev, dir).map_err(|e| failed(dir, e))? {
                if matches!(kind(&path), Kind::Other) {
                    continue;
                }
                let bytes = git.show(rev, &path).map_err(|e| failed(&path, e))?;
                files.push((path, bytes));
            }
        }
        // A revision before the manifest named a version is of the first.
        if let Ok(bytes) = git.show(rev, MANIFEST) {
            files.push((MANIFEST.to_owned(), bytes));
        }
        Self::from_files(files, rev, skip)
    }

    /// What the working tree under `root` holds.
    pub fn read(root: &Path) -> Result<Self, PublishedError> {
        let read = |path: &Path, source| PublishedError::Read {
            path: path.display().to_string(),
            source,
        };
        let mut files = Vec::new();
        let mut tables = Vec::new();
        for m in owned() {
            let dir = root.join(m);
            match std::fs::read_dir(&dir) {
                Ok(rd) => tables.extend(
                    rd.map(|e| e.map(|e| e.path()))
                        .collect::<std::io::Result<Vec<_>>>()
                        .map_err(|e| read(&dir, e))?,
                ),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(read(&dir, e)),
            }
        }
        for t in tables.iter().filter(|t| t.is_dir()) {
            let mut paths = Vec::new();
            for entry in std::fs::read_dir(t).map_err(|e| read(t, e))? {
                let path = entry.map_err(|e| read(t, e))?.path();
                if !path.is_dir() {
                    paths.push(path);
                }
            }
            paths.push(t.join(METADATA));
            for path in paths.into_iter().filter(|p| !p.is_dir()) {
                let rel = path
                    .strip_prefix(root)
                    .unwrap_or(&path)
                    .to_string_lossy()
                    .replace('\\', "/");
                let bytes = match std::fs::read(&path) {
                    Ok(b) => b,
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                    Err(e) => return Err(read(&path, e)),
                };
                files.push((rel, bytes));
            }
        }
        let manifest = root.join(MANIFEST);
        match std::fs::read(&manifest) {
            Ok(bytes) => files.push((MANIFEST.to_owned(), bytes)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(read(&manifest, e)),
        }
        Self::from_files(files, "working tree", |_, _| false)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::cell::RefCell;

    use super::*;

    #[test]
    fn a_base_market_re_based_since_is_noted_not_read() {
        let manifest = |v: u32| {
            serde_json::json!({ "markets": { "nem": { "baseline": { "version": v, "reason": "r" } } } })
                .to_string()
                .into_bytes()
        };
        let mut git = MemGit::default();
        let old = BTreeMap::from([
            (MANIFEST.to_owned(), manifest(4)),
            // A table document in a form no longer read.
            ("nem/T/metadata/v1.metadata.json".to_owned(), b"{}".to_vec()),
            ("nem/T/schema-0.json".to_owned(), b"{}".to_vec()),
            ("nem/T/other.txt".to_owned(), b"".to_vec()),
        ]);
        git.revs.insert("base".into(), old);
        let now = |v: u32| Published {
            baselines: baseline_of(&manifest(v), "now").unwrap(),
            ..Published::default()
        };
        let base = Published::held_to(&git, "base", &now(5)).unwrap();
        assert!(base.tables.is_empty() && base.schemas.is_empty());
        assert_eq!(base.unread, BTreeSet::from([Market::Nem]));
        assert_eq!(base.markets().collect::<Vec<_>>(), [Market::Nem]);
        // At the same version it is read, and must be readable.
        assert!(Published::held_to(&git, "base", &now(4)).is_err());
    }

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
            for p in
                owned().flat_map(|m| crate::census::files_under(&root.join(m)).unwrap_or_default())
            {
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

    const TABLE: &str = "nem/DISPATCH_UNIT_SCADA/metadata/v1.metadata.json";

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
        assert_eq!(
            now.tables["nem/DISPATCH_UNIT_SCADA"],
            crate::export::tests::sample()
        );
        assert_eq!(now.schemas.len(), 2);
        assert_eq!(now.baseline(Market::Nem), Baseline::FIRST);
        assert_eq!(Published::at(&git, "base").unwrap(), now);
        // The manifest names the data version; one that names none, or
        // is gone, is of the first; one that cannot be read is an error.
        let manifest = root.join(MANIFEST);
        std::fs::write(
            &manifest,
            r#"{"baseline": {"version": 2, "reason": "why"}}"#,
        )
        .unwrap();
        assert_eq!(
            Published::read(root).unwrap().baseline(Market::Nem).version,
            2
        );
        std::fs::write(&manifest, "{}").unwrap();
        assert_eq!(
            Published::read(root).unwrap().baseline(Market::Nem).version,
            1
        );
        // A manifest of many markets names each one's; a market it does not
        // name is at the first; a market that is not one is an error.
        std::fs::write(
            &manifest,
            r#"{"markets": {"gas": {"baseline": {"version": 3, "reason": "g"}}, "wem": {}}}"#,
        )
        .unwrap();
        let many = Published::read(root).unwrap();
        assert_eq!(many.baseline(Market::Gas).version, 3);
        assert_eq!(many.baseline(Market::Wem).version, 1);
        assert_eq!(many.baseline(Market::Nem).version, 1);
        assert_eq!(many.markets().collect::<Vec<_>>(), [Market::Nem]);
        std::fs::write(&manifest, r#"{"markets": {"mars": {}}}"#).unwrap();
        assert!(Published::read(root).is_err());
        std::fs::write(&manifest, r#"{"baseline": 3}"#).unwrap();
        assert!(Published::read(root).is_err());
        std::fs::write(&manifest, "[").unwrap();
        assert!(Published::read(root).is_err());
        std::fs::remove_file(&manifest).unwrap();
        assert_eq!(
            Published::read(root).unwrap().baseline(Market::Nem).version,
            1
        );
        git.broken.borrow_mut().push(MANIFEST.into());
        assert_eq!(
            Published::at(&git, "base")
                .unwrap()
                .baseline(Market::Nem)
                .version,
            1
        );
        git.broken.borrow_mut().clear();
        std::fs::create_dir(&manifest).unwrap();
        assert!(Published::read(root).is_err());
        std::fs::remove_dir(&manifest).unwrap();
        crate::export::write(
            root,
            &crate::export::Export::load(root, crate::export::DEFAULT_BASE_URL)
                .unwrap()
                .render()
                .unwrap(),
        )
        .unwrap();

        let e = Published::at(&git, "nope").unwrap_err();
        assert_eq!(e.to_string(), "git could not read nem at nope");
        git.broken.borrow_mut().push(TABLE.into());
        let e = Published::at(&git, "base").unwrap_err();
        assert!(e.to_string().contains("v1.metadata.json at base"), "{e}");

        // Metadata that is not metadata, or holds no history.
        let table = root.join(TABLE);
        let original = std::fs::read(&table).unwrap();
        std::fs::write(&table, "{}").unwrap();
        let e = Published::read(root).unwrap_err();
        assert!(
            e.to_string().contains("(working tree) holds no history: "),
            "{e}"
        );
        std::fs::write(&table, "[").unwrap();
        assert!(Published::read(root).is_err());
        let mut m: serde_json::Value = serde_json::from_slice(&original).unwrap();
        m["properties"]["phasor.declared"] = serde_json::json!("maybe");
        std::fs::write(&table, m.to_string()).unwrap();
        let e = Published::read(root).unwrap_err();
        assert!(e.to_string().contains("phasor.declared"), "{e}");

        // Metadata from before the format carried the history (format 2)
        // is not read any more.
        m["properties"] = serde_json::json!({});
        std::fs::write(&table, m.to_string()).unwrap();
        let e = Published::read(root).unwrap_err();
        assert!(e.to_string().contains("no phasor.format"), "{e}");
        std::fs::write(&table, &original).unwrap();

        // A directory where the metadata is holds nothing; a document that
        // cannot be read, or a file where nem/ is, is an error.
        std::fs::remove_file(&table).unwrap();
        std::fs::create_dir(&table).unwrap();
        assert!(Published::read(root).unwrap().tables.is_empty());
        std::fs::remove_dir(&table).unwrap();
        {
            use std::os::unix::fs::PermissionsExt as _;
            let doc = root.join("nem/DISPATCH_UNIT_SCADA/schema-0.json");
            std::fs::set_permissions(&doc, std::fs::Permissions::from_mode(0o000)).unwrap();
            let e = Published::read(root).unwrap_err();
            assert!(e.to_string().starts_with("the published file "), "{e}");
            std::fs::set_permissions(&doc, std::fs::Permissions::from_mode(0o644)).unwrap();
        }
        let flat = tempfile::tempdir().unwrap();
        std::fs::write(flat.path().join("nem"), "a file").unwrap();
        assert!(Published::read(flat.path()).is_err());
        let empty = tempfile::tempdir().unwrap();
        assert_eq!(Published::read(empty.path()).unwrap(), Published::default());
    }
}
