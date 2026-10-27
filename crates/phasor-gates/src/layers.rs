//! Every dependency between a workspace's crates is one `layers.json`
//! declares.
//!
//! The gate reads the workspace from `cargo metadata` and fails on an edge
//! to a crate of the family (by default every `phasor-*` crate) that the
//! file does not list, so a crate cannot start leaning on another without
//! the layering saying so; and on a listed edge that is gone, so the file
//! never claims more than the code does.
//!
//! `layers.json` holds two maps, crate to the family crates it depends on:
//!
//! ```json
//! {
//!   "normal": {
//!     "phasor-decode": [],
//!     "phasor-plain-decode": ["phasor-decode", "phasor-provenance"],
//!     "xtask": ["phasor-gates"]
//!   },
//!   "dev": {
//!     "phasor-schemas": ["phasor-zip"]
//!   }
//! }
//! ```
//!
//! - `normal`: the edges a crate builds with, its build-dependencies
//!   included. Every crate of the workspace is a key, with `[]` for none,
//!   so a new crate is a decision too.
//! - `dev`: the edges only its tests, examples and benchmarks build with.
//!   A crate with none is left out; `dev` itself may be left out when no
//!   crate has any.
//!
//! A crate that is both a normal and a dev dependency is listed in both.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// The file, at the repository root.
pub const FILE: &str = "layers.json";

/// Each crate's family dependencies.
pub type Graph = BTreeMap<String, BTreeSet<String>>;

/// `layers.json`, or the edges a workspace has.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Layers {
    /// Normal and build edges; every workspace crate is a key.
    pub normal: Graph,
    /// Dev edges; only crates that have one are keys.
    #[serde(default)]
    pub dev: Graph,
}

/// Which crates the gate watches edges to: any whose name starts with one
/// of `prefixes`, and any named in `names`.
#[derive(Debug, Clone, Copy)]
pub struct Family<'a> {
    /// Name prefixes, such as `phasor-`.
    pub prefixes: &'a [&'a str],
    /// Further crates by name, such as `zonetrace`.
    pub names: &'a [&'a str],
}

impl Family<'_> {
    /// Every `phasor-*` crate.
    pub const PHASOR: Family<'static> = Family {
        prefixes: &["phasor-"],
        names: &[],
    };

    /// Whether `name` is of the family.
    #[must_use]
    pub fn contains(&self, name: &str) -> bool {
        self.prefixes.iter().any(|p| name.starts_with(p)) || self.names.contains(&name)
    }
}

/// The part of `cargo metadata --no-deps` the gate reads.
#[derive(Deserialize)]
struct Metadata {
    packages: Vec<Package>,
}

#[derive(Deserialize)]
struct Package {
    name: String,
    dependencies: Vec<Dependency>,
}

#[derive(Deserialize)]
struct Dependency {
    /// The package's name, whatever the manifest renames it to.
    name: String,
    /// `null` for normal, `"dev"` or `"build"`.
    kind: Option<String>,
}

/// The family edges of the workspace `cargo metadata --format-version 1
/// --no-deps` printed as `json`: build edges count as normal.
///
/// # Errors
///
/// When `json` is not such output.
pub fn edges(json: &[u8], family: &Family<'_>) -> Result<Layers> {
    let meta: Metadata = serde_json::from_slice(json).context("reading cargo metadata")?;
    let mut layers = Layers::default();
    for p in meta.packages {
        let normal = layers.normal.entry(p.name.clone()).or_default();
        let mut dev = BTreeSet::new();
        for d in p
            .dependencies
            .into_iter()
            .filter(|d| family.contains(&d.name))
        {
            if d.kind.as_deref() == Some("dev") {
                dev.insert(d.name);
            } else {
                normal.insert(d.name);
            }
        }
        if !dev.is_empty() {
            layers.dev.insert(p.name, dev);
        }
    }
    Ok(layers)
}

/// What is wrong with `actual` against `declared`, one line each.
#[must_use]
pub fn problems(declared: &Layers, actual: &Layers) -> Vec<String> {
    let mut out = Vec::new();
    let empty = BTreeSet::new();
    for (krate, deps) in &actual.normal {
        let Some(allowed) = declared.normal.get(krate) else {
            out.push(format!("{krate}: not in {FILE}'s normal map"));
            continue;
        };
        diff(&mut out, "", krate, allowed, deps);
    }
    for (krate, deps) in &actual.dev {
        diff(
            &mut out,
            " (dev)",
            krate,
            declared.dev.get(krate).unwrap_or(&empty),
            deps,
        );
    }
    for (krate, allowed) in &declared.dev {
        if actual.normal.contains_key(krate) && !actual.dev.contains_key(krate) {
            diff(&mut out, " (dev)", krate, allowed, &empty);
        }
    }
    let workspace = |k: &&String| !actual.normal.contains_key(*k);
    let gone: BTreeSet<&String> = declared
        .normal
        .keys()
        .chain(declared.dev.keys())
        .filter(workspace)
        .collect();
    for krate in gone {
        out.push(format!("{krate}: in {FILE} but not a workspace crate"));
    }
    out
}

/// The edges of one crate of one kind: each not allowed, then each gone.
fn diff(
    out: &mut Vec<String>,
    kind: &str,
    krate: &str,
    allowed: &BTreeSet<String>,
    deps: &BTreeSet<String>,
) {
    for d in deps.difference(allowed) {
        out.push(format!("{krate} -> {d}{kind}: not allowed by {FILE}"));
    }
    for d in allowed.difference(deps) {
        out.push(format!(
            "{krate} -> {d}{kind}: listed in {FILE} but gone; remove it"
        ));
    }
}

/// The family edges of the workspace at `root`, from `cargo metadata`.
///
/// # Errors
///
/// When cargo cannot be run or fails.
pub fn workspace(root: &Path, family: &Family<'_>) -> Result<Layers> {
    let out = Command::new("cargo")
        .current_dir(root)
        .args([
            "metadata",
            "--format-version",
            "1",
            "--no-deps",
            "--offline",
        ])
        .output()
        .context("running cargo metadata")?;
    anyhow::ensure!(
        out.status.success(),
        "cargo metadata: {}",
        String::from_utf8_lossy(&out.stderr).trim()
    );
    edges(&out.stdout, family)
}

/// The gate: holds the workspace at `root` to its `layers.json`, prints
/// every problem, and returns whether there was none.
///
/// # Errors
///
/// When `layers.json` cannot be read or parsed, or cargo fails.
pub fn run(root: &Path, family: &Family<'_>) -> Result<bool> {
    let path = root.join(FILE);
    let text = std::fs::read_to_string(&path).with_context(|| format!("reading {FILE}"))?;
    let declared: Layers =
        serde_json::from_str(&text).with_context(|| format!("parsing {FILE}"))?;
    let found = problems(&declared, &workspace(root, family)?);
    for p in &found {
        eprintln!("layers: {p}");
    }
    Ok(found.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn graph(edges: &[(&str, &[&str])]) -> Graph {
        edges
            .iter()
            .map(|(k, ds)| {
                (
                    (*k).to_owned(),
                    ds.iter().map(|d| (*d).to_owned()).collect(),
                )
            })
            .collect()
    }

    #[test]
    fn the_family_is_its_prefixes_and_names() {
        let f = Family {
            prefixes: &["phasor-"],
            names: &["zonetrace"],
        };
        assert!(f.contains("phasor-zip") && f.contains("zonetrace"));
        assert!(!f.contains("serde") && !Family::PHASOR.contains("zonetrace"));
    }

    #[test]
    fn an_edge_not_declared_or_declared_and_gone_fails() {
        let declared = Layers {
            normal: graph(&[("a", &["core"]), ("core", &[]), ("old", &[]), ("t", &[])]),
            dev: graph(&[("a", &["x"]), ("core", &["fake"]), ("gone", &["x"])]),
        };
        let actual = Layers {
            normal: graph(&[("a", &["kit"]), ("core", &[]), ("new", &[]), ("t", &[])]),
            dev: graph(&[("a", &["x", "y"]), ("t", &["core"])]),
        };
        assert_eq!(
            problems(&declared, &actual),
            [
                "a -> kit: not allowed by layers.json",
                "a -> core: listed in layers.json but gone; remove it",
                "new: not in layers.json's normal map",
                "a -> y (dev): not allowed by layers.json",
                "t -> core (dev): not allowed by layers.json",
                "core -> fake (dev): listed in layers.json but gone; remove it",
                "gone: in layers.json but not a workspace crate",
                "old: in layers.json but not a workspace crate",
            ]
        );
        assert!(problems(&actual, &actual).is_empty());
    }

    #[test]
    fn build_edges_are_normal_and_dev_edges_their_own() {
        let json = br#"{"packages": [
            {"name": "phasor-a", "dependencies": [
                {"name": "phasor-b", "kind": null},
                {"name": "phasor-c", "kind": "build"},
                {"name": "phasor-d", "kind": "dev"},
                {"name": "phasor-b", "kind": "dev"},
                {"name": "serde", "kind": null}
            ]},
            {"name": "phasor-b", "dependencies": []}
        ]}"#;
        let got = edges(json, &Family::PHASOR).unwrap();
        assert_eq!(
            got,
            Layers {
                normal: graph(&[("phasor-a", &["phasor-b", "phasor-c"]), ("phasor-b", &[])]),
                dev: graph(&[("phasor-a", &["phasor-b", "phasor-d"])]),
            }
        );
        assert!(edges(b"{}", &Family::PHASOR).is_err());
        // The file reads back as written, and `dev` may be left out.
        let text = serde_json::to_string(&got).unwrap();
        assert_eq!(serde_json::from_str::<Layers>(&text).unwrap(), got);
        let bare: Layers = serde_json::from_str(r#"{"normal": {"a": []}}"#).unwrap();
        assert!(bare.dev.is_empty());
        assert!(serde_json::from_str::<Layers>(r#"{"normal": {}, "odd": {}}"#).is_err());
    }

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    #[test]
    fn runs_against_a_workspace() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(
            &root.join("Cargo.toml"),
            "[workspace]\nresolver = \"3\"\nmembers = [\"a\", \"b\"]\n",
        );
        write(
            &root.join("a/Cargo.toml"),
            "[package]\nname = \"phasor-a\"\nedition = \"2024\"\n\n[dependencies]\nphasor-b = { path = \"../b\" }\n\n[dev-dependencies]\nhelper = { package = \"phasor-b\", path = \"../b\" }\n",
        );
        write(&root.join("a/src/lib.rs"), "");
        write(
            &root.join("b/Cargo.toml"),
            "[package]\nname = \"phasor-b\"\nedition = \"2024\"\n",
        );
        write(&root.join("b/src/lib.rs"), "");
        assert!(run(root, &Family::PHASOR).is_err(), "no layers.json");
        write(&root.join(FILE), "{not json");
        assert!(run(root, &Family::PHASOR).is_err());
        write(
            &root.join(FILE),
            r#"{"normal": {"phasor-a": ["phasor-b"], "phasor-b": []}, "dev": {"phasor-a": ["phasor-b"]}}"#,
        );
        assert!(run(root, &Family::PHASOR).unwrap());
        write(
            &root.join(FILE),
            r#"{"normal": {"phasor-a": [], "phasor-b": []}}"#,
        );
        assert!(!run(root, &Family::PHASOR).unwrap());
        std::fs::remove_file(root.join("Cargo.toml")).unwrap();
        let err = run(root, &Family::PHASOR).unwrap_err().to_string();
        assert!(err.starts_with("cargo metadata: "), "{err}");
        assert!(run(&root.join("nowhere"), &Family::PHASOR).is_err());
    }
}
