//! Budgets only tighten and floors only rise. Against the base ref's copy of
//! `bench/budgets.json` and `coverage.json`, any loosened value fails unless
//! a commit since the base explains it with a `bench-budget:` or
//! `coverage-floor:` line. The message-style allowlist only shrinks: an
//! entry the base's list did not have fails.

use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result};

/// A `git` command run in `root`, clear of the variables a running git
/// hook sets (`GIT_DIR`, `GIT_INDEX_FILE`, ...), which would otherwise
/// point it at the hook's repository, or its worktree, instead of `root`.
pub fn git(root: &Path) -> Command {
    let mut cmd = Command::new("git");
    cmd.current_dir(root);
    for (key, _) in std::env::vars_os() {
        if key.to_string_lossy().starts_with("GIT_") {
            cmd.env_remove(key);
        }
    }
    cmd
}

fn git_show(root: &Path, base: &str, path: &str) -> Result<Option<String>> {
    let out = git(root)
        .args(["show", &format!("{base}:{path}")])
        .output()
        .context("running git show")?;
    if out.status.success() {
        Ok(Some(String::from_utf8_lossy(&out.stdout).into_owned()))
    } else {
        Ok(None)
    }
}

fn commit_messages_since(root: &Path, base: &str) -> Result<String> {
    let out = git(root)
        .args(["log", "--format=%B", &format!("{base}..HEAD")])
        .output()
        .context("running git log")?;
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Budget keys where a *higher* value is tighter.
fn higher_is_tighter(key: &str) -> bool {
    key.starts_with("min_")
}

/// Compares two budget documents; returns every loosening.
#[must_use]
pub fn budget_loosenings(old: &serde_json::Value, new: &serde_json::Value) -> Vec<String> {
    let mut out = Vec::new();
    let (Some(o), Some(n)) = (
        old.get("benches").and_then(|b| b.as_object()),
        new.get("benches").and_then(|b| b.as_object()),
    ) else {
        return out;
    };
    for (bench, ob) in o {
        let Some(nb) = n.get(bench).and_then(|b| b.as_object()) else {
            out.push(format!("bench/budgets.json: bench {bench} was removed"));
            continue;
        };
        let Some(ob) = ob.as_object() else { continue };
        for (key, ov) in ob {
            let (Some(ov), Some(nv)) =
                (ov.as_f64(), nb.get(key).and_then(serde_json::Value::as_f64))
            else {
                continue;
            };
            let loosened = if higher_is_tighter(key) {
                nv < ov
            } else {
                nv > ov
            };
            if loosened {
                out.push(format!(
                    "bench/budgets.json: {bench}.{key} loosened from {ov} to {nv}"
                ));
            }
        }
    }
    out
}

/// Compares two coverage documents; returns every dropped floor.
#[must_use]
pub fn floor_drops(old: &serde_json::Value, new: &serde_json::Value) -> Vec<String> {
    let mut out = Vec::new();
    let (Some(o), Some(n)) = (
        old.get("floors").and_then(|f| f.as_object()),
        new.get("floors").and_then(|f| f.as_object()),
    ) else {
        return out;
    };
    for (prefix, ov) in o {
        match n.get(prefix).and_then(serde_json::Value::as_f64) {
            None => out.push(format!("coverage.json: floor for {prefix} was removed")),
            Some(nv) if nv < ov.as_f64().unwrap_or(0.0) => out.push(format!(
                "coverage.json: floor for {prefix} dropped from {ov} to {nv}"
            )),
            _ => {}
        }
    }
    out
}

/// The gate: the working tree's budgets and floors against `base`'s.
pub fn run(root: &Path, base: &str) -> Result<bool> {
    let mut problems = Vec::new();
    let messages = commit_messages_since(root, base).unwrap_or_default();
    // A loosening or a drop stands when a commit since the base explains
    // it with the trailer named.
    let explained = |trailer: &str| {
        messages
            .lines()
            .any(|l| l.trim_start().starts_with(trailer))
    };
    if let Some(old) = git_show(root, base, "bench/budgets.json")? {
        let old: serde_json::Value = serde_json::from_str(&old).unwrap_or_default();
        let new: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(root.join("bench/budgets.json"))?)?;
        problems.extend(
            budget_loosenings(&old, &new)
                .into_iter()
                .filter(|_| !explained("bench-budget:"))
                .map(|m| {
                    format!("{m}; explain it with a `bench-budget:` line in a commit message")
                }),
        );
    }
    if let Some(old) = git_show(root, base, "coverage.json")? {
        let old: serde_json::Value = serde_json::from_str(&old).unwrap_or_default();
        let new: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(root.join("coverage.json"))?)?;
        problems.extend(
            floor_drops(&old, &new)
                .into_iter()
                .filter(|_| !explained("coverage-floor:"))
                .map(|m| {
                    format!("{m}; explain it with a `coverage-floor:` line in a commit message")
                }),
        );
    }
    if let Some(old) = git_show(root, base, crate::msg_style::ALLOW)? {
        let new = std::fs::read_to_string(root.join(crate::msg_style::ALLOW)).unwrap_or_default();
        problems.extend(crate::msg_style::growth(&old, &new).into_iter().map(|e| {
            format!(
                "{}: {e:?} is new; the allowlist only shrinks",
                crate::msg_style::ALLOW
            )
        }));
    }
    for p in &problems {
        eprintln!("{p}");
    }
    if problems.is_empty() {
        println!("ratchet: no budget loosened and no floor dropped against {base}");
    }
    Ok(problems.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_loosening_and_drops() {
        let old = serde_json::json!({ "benches": { "parse/x": { "min_mb_per_s": 100, "max_ms": 10 }, "gone": { "max_ms": 1 } } });
        let new =
            serde_json::json!({ "benches": { "parse/x": { "min_mb_per_s": 90, "max_ms": 12 } } });
        let l = budget_loosenings(&old, &new);
        assert_eq!(l.len(), 3, "{l:?}");
        let tighter = serde_json::json!({ "benches": { "parse/x": { "min_mb_per_s": 200, "max_ms": 5 }, "gone": { "max_ms": 1 } } });
        assert!(budget_loosenings(&old, &tighter).is_empty());
        let oc = serde_json::json!({ "floors": { "a": 90, "b": 80 } });
        let nc = serde_json::json!({ "floors": { "a": 85 } });
        let d = floor_drops(&oc, &nc);
        assert_eq!(d.len(), 2, "{d:?}");
        assert!(
            floor_drops(&oc, &serde_json::json!({ "floors": { "a": 95, "b": 80 } })).is_empty()
        );
        assert!(budget_loosenings(&serde_json::json!({}), &new).is_empty());
        // A bench that is not an object is passed over; documents without
        // floors have none to drop.
        let odd = serde_json::json!({ "benches": { "parse/x": 7 } });
        assert!(budget_loosenings(&odd, &new).is_empty());
        let words = serde_json::json!({ "benches": { "parse/x": { "max_ms": "ten" } } });
        assert!(budget_loosenings(&words, &new).is_empty());
        assert!(floor_drops(&serde_json::json!({}), &nc).is_empty());
    }

    fn git(dir: &Path, args: &[&str]) {
        let st = super::git(dir)
            .args(args)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t")
            .status()
            .unwrap();
        assert!(st.success(), "git {args:?}");
    }

    #[test]
    fn runs_against_a_base_commit() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        git(root, &["init", "-q", "-b", "main"]);
        git(root, &["commit", "-q", "--allow-empty", "-m", "empty"]);
        git(root, &["tag", "empty"]);
        std::fs::create_dir_all(root.join("bench")).unwrap();
        std::fs::write(
            root.join("bench/budgets.json"),
            r#"{"benches": {"x": {"max_ms": 10}}}"#,
        )
        .unwrap();
        std::fs::write(
            root.join("coverage.json"),
            r#"{"floors": {"crates/a": 90}}"#,
        )
        .unwrap();
        git(root, &["add", "."]);
        git(root, &["commit", "-q", "-m", "base"]);
        git(root, &["tag", "base"]);
        assert!(run(root, "base").unwrap());
        // A loosening without an explanation fails; one explained passes.
        std::fs::write(
            root.join("bench/budgets.json"),
            r#"{"benches": {"x": {"max_ms": 20}}}"#,
        )
        .unwrap();
        std::fs::write(
            root.join("coverage.json"),
            r#"{"floors": {"crates/a": 80}}"#,
        )
        .unwrap();
        assert!(!run(root, "base").unwrap());
        git(root, &["add", "."]);
        git(
            root,
            &[
                "commit",
                "-q",
                "-m",
                "loosen\n\nbench-budget: a slower machine\ncoverage-floor: moved",
            ],
        );
        assert!(run(root, "base").unwrap());
        // A base without the files has nothing to compare.
        assert!(run(root, "HEAD").unwrap());
        assert!(run(root, "empty").unwrap());
        // The message-style allowlist shrinks, and may go; it never grows.
        std::fs::write(root.join("msg_style.allow"), "a.rs:1\nb.rs:2\n").unwrap();
        git(root, &["add", "."]);
        git(root, &["commit", "-q", "-m", "allow"]);
        std::fs::write(root.join("msg_style.allow"), "a.rs:1\n").unwrap();
        assert!(run(root, "HEAD").unwrap());
        std::fs::remove_file(root.join("msg_style.allow")).unwrap();
        assert!(run(root, "HEAD").unwrap());
        std::fs::write(root.join("msg_style.allow"), "a.rs:1\nc.rs:3\n").unwrap();
        assert!(!run(root, "HEAD").unwrap());
    }
}
