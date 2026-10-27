//! `llm/index.md` is the map an agent reads when it needs to find where
//! something lives; `CLAUDE.md` points at it without loading it, so it must
//! stay short enough to read on demand, and every pointer in it has to be
//! true: a path that has moved sends an agent to the wrong place with
//! confidence.

use std::collections::BTreeSet;
use std::path::Path;
use std::process::Command;

use anyhow::Result;

/// The entry file every session loads.
pub const ENTRY: &str = "CLAUDE.md";
/// The index the entry file points at.
pub const INDEX: &str = "llm/index.md";
/// (lines, bytes) the entry file may hold.
pub const ENTRY_BUDGET: (usize, usize) = (20, 1024);
/// (lines, bytes) the index may hold.
pub const INDEX_BUDGET: (usize, usize) = (90, 9 * 1024);

/// Code spans that look like paths: no spaces, a slash or an extension.
#[must_use]
pub fn path_spans(markdown: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let mut rest = markdown;
    while let Some(start) = rest.find('`') {
        let after = &rest[start + 1..];
        let Some(end) = after.find('`') else { break };
        let span = &after[..end];
        if !span.contains(' ')
            && !span.contains('\n')
            && (span.contains('/')
                || span.rsplit_once('.').is_some_and(|(_, ext)| {
                    ext.chars().all(char::is_alphanumeric) && !ext.is_empty()
                }))
        {
            out.insert(span.to_owned());
        }
        rest = &after[end + 1..];
    }
    out
}

/// `just <recipe>` spans.
#[must_use]
pub fn recipe_spans(markdown: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let mut rest = markdown;
    while let Some(start) = rest.find('`') {
        let after = &rest[start + 1..];
        let Some(end) = after.find('`') else { break };
        let span = &after[..end];
        if let Some(r) = span.strip_prefix("just ") {
            let recipe: String = r
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '-' || *c == '_')
                .collect();
            if !recipe.is_empty() {
                out.insert(recipe);
            }
        }
        rest = &after[end + 1..];
    }
    out
}

/// The gate: the entry file and the index under `root`, against their
/// budgets, with every path and recipe they name checked.
pub fn run(root: &Path) -> Result<bool> {
    let mut failures = Vec::new();
    let read = |name: &str, budget: (usize, usize), failures: &mut Vec<String>| -> String {
        let Ok(text) = std::fs::read_to_string(root.join(name)) else {
            failures.push(format!("{name} is missing"));
            return String::new();
        };
        let lines = text.trim_end().lines().count();
        if lines > budget.0 {
            failures.push(format!(
                "{name} has {lines} lines; the budget is {}",
                budget.0
            ));
        }
        if text.len() > budget.1 {
            failures.push(format!(
                "{name} has {} bytes; the budget is {}",
                text.len(),
                budget.1
            ));
        }
        text
    };
    let entry = read(ENTRY, ENTRY_BUDGET, &mut failures);
    if !entry.is_empty() && !entry.contains(INDEX) {
        failures.push(format!(
            "{ENTRY} does not point at the index: it must name {INDEX}"
        ));
    }
    if entry.lines().any(|l| l.trim() == format!("@{INDEX}")) {
        failures.push(format!("{ENTRY} imports the index with @{INDEX}; it must only point at it, so the index is read when needed, not on every session"));
    }
    let index = read(INDEX, INDEX_BUDGET, &mut failures);
    for p in path_spans(&index) {
        let path = root.join(&p);
        let ignored = crate::ratchet::git(root)
            .args(["check-ignore", "-q", &p])
            .status()
            .is_ok_and(|s| s.success());
        if !path.exists() && !ignored {
            failures.push(format!("{INDEX} names `{p}`, which does not exist"));
        }
    }
    let summary = Command::new("just")
        .arg("--summary")
        .current_dir(root)
        .output();
    match summary {
        Ok(out) if out.status.success() => {
            let recipes: BTreeSet<String> = String::from_utf8_lossy(&out.stdout)
                .split_whitespace()
                .map(str::to_owned)
                .collect();
            for r in recipe_spans(&index) {
                if !recipes.contains(&r) {
                    failures.push(format!("{INDEX} names `just {r}`, which is not a recipe"));
                }
            }
        }
        _ => failures.push(
            "`just --summary` failed, so the recipes named in the index could not be checked"
                .into(),
        ),
    }
    for f in &failures {
        eprintln!("{f}");
    }
    if failures.is_empty() {
        println!(
            "{INDEX}: {} lines, {} bytes, every path and recipe exists",
            index.trim_end().lines().count(),
            index.len()
        );
    }
    Ok(failures.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spans() {
        let md = "Run `just cover` and read `crates/a/src/x.rs`, `README.md`, `WatchFleet` and `cargo test`.";
        let p = path_spans(md);
        assert!(p.contains("crates/a/src/x.rs") && p.contains("README.md"));
        assert!(!p.contains("WatchFleet") && !p.contains("cargo test"));
        assert_eq!(recipe_spans(md).into_iter().collect::<Vec<_>>(), ["cover"]);
    }

    #[test]
    fn runs_against_a_repository() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("llm")).unwrap();
        std::fs::write(root.join("justfile"), "cover:\n    echo\n").unwrap();
        std::fs::write(root.join("README.md"), "x").unwrap();
        std::fs::write(root.join(ENTRY), "Read `llm/index.md` when needed.\n").unwrap();
        std::fs::write(root.join(INDEX), "See `README.md`; run `just cover`.\n").unwrap();
        assert!(run(root).unwrap());
        std::fs::write(
            root.join(INDEX),
            "See `gone.md`; run `just nothing`.\n".to_owned() + &"x".repeat(INDEX_BUDGET.1),
        )
        .unwrap();
        assert!(!run(root).unwrap());
        std::fs::write(root.join(ENTRY), "@llm/index.md\n").unwrap();
        assert!(!run(root).unwrap());
        // An entry that does not point at the index; an index of too many
        // lines; no justfile, so no recipe can be checked.
        std::fs::write(root.join(ENTRY), "Nothing to see.\n").unwrap();
        std::fs::write(root.join(INDEX), "x\n".repeat(INDEX_BUDGET.0 + 1)).unwrap();
        std::fs::remove_file(root.join("justfile")).unwrap();
        assert!(!run(root).unwrap());
        std::fs::remove_file(root.join(ENTRY)).unwrap();
        assert!(!run(root).unwrap());
    }
}
