//! Times utilake-zip on real zips in a directory, in zones:
//!
//! ```text
//! cargo run --release -p utilake-zip --features zones --example zipbench -- {dir} [{out}]
//! ```
//!
//! For each zip, four passes, each run once to warm up and then three times
//! (the median is kept):
//!
//! - `dir`: the central directory alone, by `zipdir::read_from`.
//! - `walk8k` and `walk256k`: every `.CSV` entry walked and read to its end
//!   in reads of 8 KiB and 256 KiB, the bytes thrown away, so the time is
//!   the walker's and not a parser's.
//! - `ranged`: what a ranged reader would fetch, by `DirectoryReader`, for
//!   the directory and then the largest entry: requests and bytes.
//!
//! Each pass prints its zone tree, the tracer's cost taken out. With `out`,
//! a summary line per pass and the zone rows go to NDJSON files there.

use std::fmt::Write as _;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::Instant;

use peak_alloc::PeakAlloc;
use utilake_zip::zipdir::{self, DirectoryReader, LOCAL_HEADER, Step};

#[global_allocator]
static PEAK: PeakAlloc = PeakAlloc;

/// Runs kept after the warm-up.
const RUNS: usize = 3;

/// The slack a ranged reader fetches past an entry's name for its local
/// extra field, as utilake's fetcher does.
const SLACK: u64 = 256;

struct Pass {
    ms: f64,
    peak: f64,
    entries: usize,
    bytes: u64,
    report: zonetrace::Report,
    note: String,
}

fn main() {
    let mut args = std::env::args().skip(1);
    let dir = PathBuf::from(args.next().expect("a directory of zips"));
    let out = args.next().map(PathBuf::from);
    let mut zips: Vec<PathBuf> = std::fs::read_dir(&dir)
        .expect("the directory")
        .map(|e| e.expect("an entry").path())
        .filter(|p| utilake_zip::has_extension(&p.to_string_lossy(), ".zip"))
        .collect();
    zips.sort();
    let overhead = zonetrace::overhead();
    println!("tracer: {overhead:.1} ns a zone, taken out of each parent\n");
    let spool = std::env::temp_dir().join("zipbench-spool");
    let mut summary = String::new();
    let mut rows = String::new();
    for zip in &zips {
        let size = std::fs::metadata(zip).expect("size").len();
        let name = zip
            .file_name()
            .expect("a name")
            .to_string_lossy()
            .into_owned();
        println!("== {name} ({:.1} MB)", size as f64 / 1e6);
        let passes: [(&str, &dyn Fn() -> Pass); 4] = [
            ("dir", &|| directory(zip, size)),
            ("walk8k", &|| walk(zip, &spool, 8 << 10)),
            ("walk256k", &|| walk(zip, &spool, 256 << 10)),
            ("ranged", &|| ranged(zip, size)),
        ];
        for (pass, run) in passes {
            let p = median(run);
            let mb_s = size as f64 / 1e6 / (p.ms / 1e3);
            println!(
                "-- {pass}: {:.3} ms, {mb_s:.0} MB/s of zip, peak heap {:.1} MB, {} entries, {:.1} MB out{}",
                p.ms,
                p.peak,
                p.entries,
                p.bytes as f64 / 1e6,
                p.note
            );
            print!("{}", p.report.text(overhead));
            let _ = writeln!(
                summary,
                r#"{{"file":"{name}","pass":"{pass}","size":{size},"ms":{:.3},"mb_s":{mb_s:.1},"peak_heap_mb":{:.2},"entries":{},"bytes":{},"overhead_ns":{overhead:.2}{}}}"#,
                p.ms,
                p.peak,
                p.entries,
                p.bytes,
                if p.note.is_empty() {
                    String::new()
                } else {
                    format!(r#","note":"{}""#, p.note.trim())
                }
            );
            for line in p.report.ndjson(overhead).lines() {
                let _ = writeln!(rows, r#"{{"file":"{name}","pass":"{pass}","row":{line}}}"#);
            }
        }
        println!();
    }
    if let Some(out) = out {
        std::fs::create_dir_all(&out).expect("the output directory");
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time")
            .as_secs();
        let rev = std::process::Command::new("git")
            .args(["rev-parse", "--short", "HEAD"])
            .output()
            .ok()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
            .unwrap_or_default();
        let base = out.join(format!("{rev}-{stamp}"));
        std::fs::write(base.with_extension("summary.ndjson"), summary).expect("summary");
        std::fs::write(base.with_extension("zones.ndjson"), rows).expect("zones");
        println!("wrote {}.{{summary,zones}}.ndjson", base.display());
    }
}

/// One warm-up, then the median of [`RUNS`] by time.
fn median(run: &dyn Fn() -> Pass) -> Pass {
    let _ = run();
    let mut runs: Vec<Pass> = (0..RUNS).map(|_| run()).collect();
    runs.sort_by(|a, b| a.ms.total_cmp(&b.ms));
    runs.swap_remove(RUNS / 2)
}

/// Starts a timed pass: a clean tree and a clean peak.
fn start() -> Instant {
    let _ = zonetrace::take();
    PEAK.reset_peak_usage();
    Instant::now()
}

fn finish(t: Instant, entries: usize, bytes: u64, note: String) -> Pass {
    let ms = t.elapsed().as_secs_f64() * 1e3;
    Pass {
        ms,
        peak: f64::from(PEAK.peak_usage_as_mb()),
        entries,
        bytes,
        report: zonetrace::take(),
        note,
    }
}

fn directory(zip: &Path, size: u64) -> Pass {
    let t = start();
    let mut f = File::open(zip).expect("open");
    let entries = zipdir::read_from(&mut f, size).expect("a directory");
    let bytes = entries.iter().map(|e| e.size).sum();
    finish(t, entries.len(), bytes, String::new())
}

fn walk(zip: &Path, spool: &Path, buf: usize) -> Pass {
    let mut scratch = vec![0u8; buf];
    let mut bytes = 0u64;
    let t = start();
    let walked = utilake_zip::walk_csv_entries(
        zip,
        spool,
        &mut |e| {
            loop {
                let n = e.reader.read(&mut scratch)?;
                if n == 0 {
                    return Ok::<(), std::io::Error>(());
                }
                bytes += n as u64;
            }
        },
        &mut |_| {},
    )
    .expect("a walk");
    let note = if walked.failed.is_empty() {
        String::new()
    } else {
        format!(", {} failed", walked.failed.len())
    };
    finish(t, walked.entries, bytes, note)
}

/// Reads `start..end` of `f`, as one ranged request would.
fn span(f: &mut File, start: u64, end: u64) -> Vec<u8> {
    let mut buf = vec![0u8; usize::try_from(end - start).expect("a span")];
    f.seek(SeekFrom::Start(start)).expect("seek");
    f.read_exact(&mut buf).expect("read");
    buf
}

fn ranged(zip: &Path, size: u64) -> Pass {
    let t = start();
    let mut f = File::open(zip).expect("open");
    let mut reader = DirectoryReader::new(size);
    let mut step = reader.next();
    let (mut requests, mut fetched) = (0u32, 0u64);
    let entries = loop {
        match step {
            Step::Read { start, end } => {
                requests += 1;
                fetched += end - start;
                step = reader.feed(span(&mut f, start, end)).expect("a directory");
            }
            Step::Done(entries) => break entries,
        }
    };
    let dir = (requests, fetched);
    // The largest entry, as a fetcher picks one: its header, name, slack
    // and data in one span, and one more when the local extra is longer.
    let largest = entries
        .iter()
        .filter(|e| !e.is_dir())
        .max_by_key(|e| e.compressed)
        .expect("an entry");
    let at = largest.local_header_offset;
    let want = LOCAL_HEADER as u64 + largest.name.len() as u64 + SLACK + largest.compressed;
    let end = (at + want).min(size);
    let got = span(&mut f, at, end);
    requests += 1;
    fetched += end - at;
    let head = zipdir::local_header_len(&got).expect("a local header") as u64;
    if head + largest.compressed > end - at {
        requests += 1;
        fetched += head + largest.compressed - (end - at);
    }
    let note = format!(
        ", directory {} req {} B ({:.4}% of zip), + largest entry {} = {} req {:.1} MB",
        dir.0,
        dir.1,
        dir.1 as f64 / size as f64 * 100.0,
        largest.name,
        requests,
        fetched as f64 / 1e6
    );
    finish(t, entries.len(), largest.size, note)
}
