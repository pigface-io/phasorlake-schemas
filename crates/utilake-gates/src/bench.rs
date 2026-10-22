//! Benchmarks held to `bench/budgets.json`.
//!
//! Micro benchmarks are criterion benches in the crates; their medians are
//! read from `target/criterion/*/new/estimates.json`. A repository with an
//! end-to-end harness names it in its [`Targets`]: a binary that prints
//! JSON measurements, run under `/usr/bin/time` for the peak resident set
//! size, several times, each metric keeping its best run.
//!
//! A budget is `max_*` (lower is better) or `min_*` (higher is better). A
//! measurement outside its budget by more than the tolerance fails. With
//! `--tighten`, budgets move to the measurement when that is tighter, never
//! the other way.

use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

/// `bench/budgets.json`.
#[derive(Debug, Serialize, Deserialize)]
pub struct Budgets {
    /// Which machine the numbers were taken on.
    pub machine: String,
    /// Slack before a breach counts.
    pub tolerance: Tolerance,
    /// Per benchmark: key -> number. Keys start with `max_` or `min_`.
    pub benches: BTreeMap<String, BTreeMap<String, f64>>,
}

/// Slack before a breach counts.
#[derive(Debug, Serialize, Deserialize, Clone, Copy)]
pub struct Tolerance {
    /// Percent slack on time and throughput.
    pub time_pct: f64,
    /// Percent slack on memory.
    pub mem_pct: f64,
}

/// A measurement set: bench -> key -> value, using the same keys as budgets.
pub type Measurements = BTreeMap<String, BTreeMap<String, f64>>;

/// What a repository benchmarks.
#[derive(Debug, Clone, Copy)]
pub struct Targets<'a> {
    /// (package, bench target) of the fast set, run before a push.
    pub fast: &'a [(&'a str, &'a str)],
    /// (package, bench target) of the full set, run in CI.
    pub full: &'a [(&'a str, &'a str)],
    /// The end-to-end harness package, built in release and run under
    /// `/usr/bin/time` in the full set; none when the repository has none.
    pub e2e: Option<&'a str>,
}

fn is_memory_key(key: &str) -> bool {
    key.contains("rss") || key.contains("heap") || key.contains("alloc")
}

/// Compares measurements to budgets; returns breaches and a table of rows.
#[must_use]
pub fn compare(budgets: &Budgets, measured: &Measurements) -> (Vec<String>, Vec<String>) {
    let mut breaches = Vec::new();
    let mut rows = Vec::new();
    for (bench, keys) in measured {
        let Some(budget) = budgets.benches.get(bench) else {
            breaches.push(format!("{bench}: no budget in bench/budgets.json; add one"));
            continue;
        };
        for (key, value) in keys {
            let Some(limit) = budget.get(key) else {
                continue;
            };
            let pct = if is_memory_key(key) {
                budgets.tolerance.mem_pct
            } else {
                budgets.tolerance.time_pct
            };
            let slack = limit * pct / 100.0;
            let ok = if key.starts_with("min_") {
                *value >= limit - slack
            } else {
                *value <= limit + slack
            };
            rows.push(format!(
                "{:<4} {bench:<32} {key:<22} {value:>14.3}  budget {limit:>12.3}",
                if ok { "ok" } else { "FAIL" }
            ));
            if !ok {
                breaches.push(format!(
                    "{bench}.{key} = {value:.3} breaks budget {limit:.3} (tolerance {pct}%)"
                ));
            }
        }
    }
    (breaches, rows)
}

/// Moves each budget to the measurement when that is tighter.
pub fn tighten(budgets: &mut Budgets, measured: &Measurements) -> usize {
    let mut n = 0;
    for (bench, keys) in measured {
        let b = budgets.benches.entry(bench.clone()).or_default();
        for (key, value) in keys {
            let slack = if is_memory_key(key) { 1.10 } else { 1.15 };
            let proposed = if key.starts_with("min_") {
                value / slack
            } else {
                value * slack
            };
            let proposed = round3(proposed);
            match b.get(key) {
                Some(cur)
                    if (key.starts_with("min_") && proposed <= *cur)
                        || (!key.starts_with("min_") && proposed >= *cur) => {}
                _ => {
                    b.insert(key.clone(), proposed);
                    n += 1;
                }
            }
        }
    }
    n
}

fn round3(v: f64) -> f64 {
    (v * 1000.0).round() / 1000.0
}

#[derive(Deserialize)]
struct Estimates {
    median: Estimate,
}
#[derive(Deserialize)]
struct Estimate {
    point_estimate: f64,
}
#[derive(Deserialize)]
struct BenchmarkMeta {
    #[serde(default)]
    throughput: Option<Throughput>,
}
#[derive(Deserialize)]
struct Throughput {
    #[serde(rename = "Bytes", default)]
    bytes: Option<u64>,
}

/// Reads criterion's output for every bench under `target/criterion`.
pub fn read_criterion(target: &Path) -> Result<Measurements> {
    let mut out: Measurements = BTreeMap::new();
    let root = target.join("criterion");
    let mut stack = vec![root.clone()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in rd.filter_map(Result::ok) {
            let p = e.path();
            if !p.is_dir() {
                continue;
            }
            let new = p.join("new");
            if new.is_dir() {
                let est: Estimates =
                    serde_json::from_str(&std::fs::read_to_string(new.join("estimates.json"))?)?;
                let meta: BenchmarkMeta =
                    serde_json::from_str(&std::fs::read_to_string(new.join("benchmark.json"))?)?;
                let name = p
                    .strip_prefix(&root)
                    .unwrap_or(&p)
                    .to_string_lossy()
                    .replace('\\', "/");
                let ns = est.median.point_estimate;
                let keys = out.entry(name).or_default();
                if let Some(bytes) = meta.throughput.and_then(|t| t.bytes) {
                    keys.insert(
                        "min_mb_per_s".into(),
                        round3(bytes as f64 / 1e6 / (ns / 1e9)),
                    );
                } else if ns > 1e6 {
                    keys.insert("max_ms".into(), round3(ns / 1e6));
                } else {
                    keys.insert("max_ns".into(), round3(ns));
                }
            } else {
                stack.push(p);
            }
        }
    }
    Ok(out)
}

/// Runs of the end-to-end benchmark, each in its own process; each metric
/// keeps its best value, so one run slowed by the machine (a benchmark
/// just before it, a cold page cache) does not fail the gate.
const E2E_RUNS: usize = 3;

fn run_e2e(root: &Path, package: &str) -> Result<Measurements> {
    let status = Command::new("cargo")
        .args(["build", "--release", "-q", "-p", package])
        .current_dir(root)
        .status()?;
    if !status.success() {
        bail!("building {package} failed");
    }
    measure_e2e(root, &root.join("target/release").join(package), package)
}

/// Runs the built harness `bin` three times; each metric keeps its
/// best value.
pub fn measure_e2e(root: &Path, bin: &Path, package: &str) -> Result<Measurements> {
    let mut best: Measurements = BTreeMap::new();
    for _ in 0..E2E_RUNS {
        for (bench, keys) in run_e2e_once(root, bin, package)? {
            let into = best.entry(bench).or_default();
            for (key, value) in keys {
                let better = |old: f64| {
                    if key.starts_with("min_") {
                        value.max(old)
                    } else {
                        value.min(old)
                    }
                };
                let v = into.get(&key).map_or(value, |&old| better(old));
                into.insert(key, v);
            }
        }
    }
    Ok(best)
}

fn run_e2e_once(root: &Path, bin: &Path, package: &str) -> Result<Measurements> {
    let flag = if cfg!(target_os = "macos") {
        "-l"
    } else {
        "-v"
    };
    let out = Command::new("/usr/bin/time")
        .arg(flag)
        .arg(bin)
        .current_dir(root)
        .output()?;
    if !out.status.success() {
        bail!("{package} failed: {}", String::from_utf8_lossy(&out.stderr));
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    let mut measured: Measurements = serde_json::from_str(stdout.trim())
        .with_context(|| format!("{package} did not print JSON measurements"))?;
    if let Some(rss) = parse_rss_mb(&stderr) {
        for keys in measured.values_mut() {
            keys.insert("max_rss_mb".into(), round3(rss));
        }
    }
    Ok(measured)
}

/// Peak resident set size in MB from `/usr/bin/time -l` (macOS, bytes) or `-v` (GNU, KiB).
#[must_use]
pub fn parse_rss_mb(time_output: &str) -> Option<f64> {
    for line in time_output.lines() {
        let l = line.trim();
        if let Some(rest) = l.strip_suffix("maximum resident set size") {
            return rest.trim().parse::<f64>().ok().map(|b| b / 1e6);
        }
        if let Some(rest) = l.strip_prefix("Maximum resident set size (kbytes):") {
            return rest.trim().parse::<f64>().ok().map(|k| k * 1024.0 / 1e6);
        }
    }
    None
}

/// The gate: runs `set` (`fast` or `full`) of `targets` under `root`,
/// holds the measurements to `bench/budgets.json`, and with `do_tighten`
/// moves the budgets to the measurements where tighter.
pub fn run(root: &Path, targets: Targets<'_>, set: &str, do_tighten: bool) -> Result<bool> {
    let budgets_path = root.join("bench/budgets.json");
    let mut budgets: Budgets = serde_json::from_str(
        &std::fs::read_to_string(&budgets_path).context("reading bench/budgets.json")?,
    )?;
    // (package, bench target). Named explicitly: `--benches` would also run the
    // library's unit tests as a benchmark harness, which rejects criterion flags.
    let list = match set {
        "fast" => targets.fast,
        "full" => targets.full,
        other => bail!("unknown set {other}; use fast or full"),
    };
    let target_dir = root.join("target");
    let _ = std::fs::remove_dir_all(target_dir.join("criterion"));
    for (p, target) in list {
        let status = Command::new("cargo")
            .args([
                "bench", "-q", "-p", p, "--bench", target, "--", "--quick", "--noplot",
            ])
            .current_dir(root)
            .status()?;
        if !status.success() {
            bail!("cargo bench -p {p} failed");
        }
    }
    let mut measured = read_criterion(&target_dir)?;
    if set == "full"
        && let Some(e2e) = targets.e2e
    {
        measured.extend(run_e2e(root, e2e)?);
    }
    if do_tighten {
        let n = tighten(&mut budgets, &measured);
        std::fs::write(
            &budgets_path,
            serde_json::to_string_pretty(&budgets)? + "\n",
        )?;
        println!("tightened {n} budget(s)");
    }
    let (breaches, rows) = compare(&budgets, &measured);
    for r in &rows {
        println!("{r}");
    }
    for b in &breaches {
        eprintln!("{b}");
    }
    Ok(breaches.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn budgets() -> Budgets {
        let mut benches = BTreeMap::new();
        benches.insert(
            "parse/x".to_owned(),
            [
                ("min_mb_per_s".to_owned(), 100.0),
                ("max_ns".to_owned(), 10.0),
            ]
            .into_iter()
            .collect(),
        );
        Budgets {
            machine: "test".into(),
            tolerance: Tolerance {
                time_pct: 10.0,
                mem_pct: 5.0,
            },
            benches,
        }
    }

    #[test]
    fn compares_with_tolerance() {
        let b = budgets();
        let mut m: Measurements = BTreeMap::new();
        m.insert(
            "parse/x".into(),
            [
                ("min_mb_per_s".to_owned(), 91.0),
                ("max_ns".to_owned(), 10.9),
            ]
            .into_iter()
            .collect(),
        );
        let (breaches, rows) = compare(&b, &m);
        assert!(breaches.is_empty(), "{breaches:?}");
        assert_eq!(rows.len(), 2);
        m.get_mut("parse/x")
            .unwrap()
            .insert("min_mb_per_s".into(), 80.0);
        m.get_mut("parse/x").unwrap().insert("max_ns".into(), 12.0);
        m.insert(
            "new/bench".into(),
            [("max_ms".to_owned(), 1.0)].into_iter().collect(),
        );
        let (breaches, _) = compare(&b, &m);
        assert_eq!(breaches.len(), 3, "{breaches:?}");
    }

    #[test]
    fn tightens_only_tighter() {
        let mut b = budgets();
        let mut m: Measurements = BTreeMap::new();
        m.insert(
            "parse/x".into(),
            [
                ("min_mb_per_s".to_owned(), 200.0),
                ("max_ns".to_owned(), 20.0),
                ("max_rss_mb".to_owned(), 50.0),
            ]
            .into_iter()
            .collect(),
        );
        let n = tighten(&mut b, &m);
        assert_eq!(n, 2, "{b:?}");
        assert!((b.benches["parse/x"]["min_mb_per_s"] - 173.913).abs() < 0.01);
        assert_eq!(
            b.benches["parse/x"]["max_ns"], 10.0,
            "a looser measurement leaves the budget alone"
        );
        assert_eq!(b.benches["parse/x"]["max_rss_mb"], 55.0);
    }

    #[test]
    fn parses_time_output() {
        assert_eq!(
            parse_rss_mb("  1.00 real\n   123456789  maximum resident set size\n"),
            Some(123.456_789)
        );
        assert_eq!(
            parse_rss_mb("\tMaximum resident set size (kbytes): 2048\n"),
            Some(2.097_152)
        );
        assert_eq!(parse_rss_mb("nothing"), None);
    }

    #[test]
    fn reads_criterion_output_and_runs_an_empty_set() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let new = root.join("target/criterion/parse/x/new");
        std::fs::create_dir_all(&new).unwrap();
        std::fs::write(
            new.join("estimates.json"),
            r#"{"median": {"point_estimate": 2000000.0}}"#,
        )
        .unwrap();
        std::fs::write(
            new.join("benchmark.json"),
            r#"{"throughput": {"Bytes": 1000000}}"#,
        )
        .unwrap();
        let slow = root.join("target/criterion/slow/new");
        std::fs::create_dir_all(&slow).unwrap();
        std::fs::write(
            slow.join("estimates.json"),
            r#"{"median": {"point_estimate": 2e6}}"#,
        )
        .unwrap();
        std::fs::write(slow.join("benchmark.json"), "{}").unwrap();
        let quick = root.join("target/criterion/quick/new");
        std::fs::create_dir_all(&quick).unwrap();
        std::fs::write(
            quick.join("estimates.json"),
            r#"{"median": {"point_estimate": 500.0}}"#,
        )
        .unwrap();
        std::fs::write(quick.join("benchmark.json"), "{}").unwrap();
        let m = read_criterion(&root.join("target")).unwrap();
        assert_eq!(m["parse/x"]["min_mb_per_s"], 500.0);
        assert_eq!(m["slow"]["max_ms"], 2.0);
        assert_eq!(m["quick"]["max_ns"], 500.0);
        // The gate over no targets reads nothing and passes; an unknown set fails.
        std::fs::create_dir_all(root.join("bench")).unwrap();
        std::fs::write(
            root.join("bench/budgets.json"),
            r#"{"machine": "t", "tolerance": {"time_pct": 10, "mem_pct": 5}, "benches": {}}"#,
        )
        .unwrap();
        let none = Targets {
            fast: &[],
            full: &[],
            e2e: None,
        };
        assert!(run(root, none, "fast", false).unwrap());
        assert!(run(root, none, "full", true).unwrap());
        assert!(run(root, none, "other", false).is_err());
        // A bench target that is not there fails the gate before measuring.
        let missing = Targets {
            fast: &[("no-such-package", "none")],
            full: &[],
            e2e: Some("no-such-package"),
        };
        assert!(run(root, missing, "fast", false).is_err());
        assert!(run(root, missing, "full", false).is_err());
        let tight: Budgets = serde_json::from_str(
            &std::fs::read_to_string(root.join("bench/budgets.json")).unwrap(),
        )
        .unwrap();
        assert!(tight.benches.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn measures_a_harness_and_tightens_to_it() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let bin = root.join("harness");
        std::fs::write(
            &bin,
            "#!/bin/sh\necho '{\"e2e/run\": {\"max_ms\": 12.5, \"min_rows_per_s\": 1000}}'\n",
        )
        .unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        let m = measure_e2e(root, &bin, "harness").unwrap();
        assert_eq!(m["e2e/run"]["max_ms"], 12.5);
        assert_eq!(m["e2e/run"]["min_rows_per_s"], 1000.0);
        assert!(m["e2e/run"].contains_key("max_rss_mb"), "{m:?}");
        // The budgets tighten to what was measured, then hold it.
        std::fs::create_dir_all(root.join("bench")).unwrap();
        std::fs::write(
            root.join("bench/budgets.json"),
            r#"{"machine": "t", "tolerance": {"time_pct": 10, "mem_pct": 5}, "benches": {}}"#,
        )
        .unwrap();
        let mut budgets: Budgets = serde_json::from_str(
            &std::fs::read_to_string(root.join("bench/budgets.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(tighten(&mut budgets, &m), 3);
        let (breaches, rows) = compare(&budgets, &m);
        assert!(breaches.is_empty(), "{breaches:?}");
        assert_eq!(rows.len(), 3);
        // A harness that fails, or prints no JSON, is an error.
        std::fs::write(&bin, "#!/bin/sh\nexit 3\n").unwrap();
        assert!(measure_e2e(root, &bin, "harness").is_err());
        std::fs::write(&bin, "#!/bin/sh\necho nope\n").unwrap();
        assert!(measure_e2e(root, &bin, "harness").is_err());
    }
}
