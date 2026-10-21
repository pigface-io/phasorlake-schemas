//! Coverage floors only rise. Against the base ref's copy of
//! `coverage.json`, a dropped or removed floor fails unless a commit since
//! the base explains it with a `coverage-floor:` line.

use std::process::Command;

use anyhow::{Context, Result};

fn git(args: &[&str]) -> Result<Option<String>> {
    let out = Command::new("git")
        .args(args)
        .current_dir(crate::repo_root())
        .output()
        .context("running git")?;
    Ok(out
        .status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned()))
}

/// Compares two coverage documents; returns every dropped floor.
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

pub fn run(base: &str) -> Result<bool> {
    let root = crate::repo_root();
    let Some(old) = git(&["show", &format!("{base}:coverage.json")])? else {
        println!("ratchet: {base} has no coverage.json");
        return Ok(true);
    };
    let old: serde_json::Value = serde_json::from_str(&old).unwrap_or_default();
    let new: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(root.join("coverage.json"))?)?;
    let drops = floor_drops(&old, &new);
    let messages = git(&["log", "--format=%B", &format!("{base}..HEAD")])?.unwrap_or_default();
    let explained = messages
        .lines()
        .any(|l| l.trim_start().starts_with("coverage-floor:"));
    if drops.is_empty() || explained {
        println!("ratchet: no floor dropped against {base}");
        return Ok(true);
    }
    for d in &drops {
        eprintln!("{d}; explain it with a `coverage-floor:` line in a commit message");
    }
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_drops() {
        let oc = serde_json::json!({ "floors": { "a": 90, "b": 80 } });
        let nc = serde_json::json!({ "floors": { "a": 85 } });
        assert_eq!(floor_drops(&oc, &nc).len(), 2);
        let up = serde_json::json!({ "floors": { "a": 95, "b": 80 } });
        assert!(floor_drops(&oc, &up).is_empty());
        assert!(floor_drops(&serde_json::json!({}), &nc).is_empty());
    }
}
