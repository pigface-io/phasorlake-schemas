//! `phasor-schemas`: fetch a month of AEMO's archive, take its census, list
//! the files every census read from a mirror archive, take
//! the census of the report folders, record a month's data-model releases,
//! fold every census into each table's history, export the histories as
//! the documents under `nem/`, hold them to a git revision, plan a read of
//! a table, write the histories as Iceberg tables; and, once, write the
//! table declarations from phasor's table files.
//!
//! ```text
//! phasor-schemas fetch --month 2024-01 --dir mirror/MMSDM_2024_01 [--interval-ms 1000] [--root .]
//! phasor-schemas census --month 2024-01 --dir mirror/MMSDM_2024_01 [--root .]
//! phasor-schemas sources --archive /data/mirror/archive [--check] [--root .]
//! phasor-schemas reports --dir mirror/archive/www.nemweb.com.au [--market gas] [--feed gbb] [--mmsdm mirror] [--mirror ~/.phasor-mirror] [--root .]
//! phasor-schemas models --month 2025-10 --dir mirror/MMSDM_2025_10 [--root .]
//! phasor-schemas fold [--check] [--rebaseline REASON] [--root .]
//! phasor-schemas export [--check] [--base-url URL] [--root .]
//! phasor-schemas compat --base origin/main [--root .]
//! phasor-schemas diff --base origin/main [--root .]
//! phasor-schemas sql DISPATCHPRICE --files files.json --from 2024-08-01 --to 2024-08-02 [--as-of 0] [--root .]
//! phasor-schemas iceberg --out target/iceberg [--root .]
//! ```

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use clap::{Parser, Subcommand};
use phasor_iceberg::Market;
use phasor_schemas::catalog::{Catalog, tables_dir};
use phasor_schemas::fetch::{DEFAULT_INTERVAL_MS, Http, Response};
use phasor_schemas::month::YearMonth;

#[derive(Parser)]
#[command(
    name = "phasor-schemas",
    version,
    about = "The census of AEMO's MMS archive."
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Download every file of a month from nemweb into a directory, one
    /// request at a time, and record each in `{root}/sources/{YYYY-MM}.tsv`.
    Fetch {
        /// The month, `YYYY-MM`.
        #[arg(long)]
        month: YearMonth,
        /// Where the month's files go.
        #[arg(long)]
        dir: PathBuf,
        /// The least time between two requests, in milliseconds.
        #[arg(long, default_value_t = DEFAULT_INTERVAL_MS)]
        interval_ms: u64,
        /// The repository root.
        #[arg(long, default_value = ".")]
        root: PathBuf,
    },
    /// Record every block of every file of a month's directory in
    /// `{root}/evidence/census/{market}/{YYYY-MM}.toml`.
    Census {
        /// The month, `YYYY-MM`.
        #[arg(long)]
        month: YearMonth,
        /// The month's directory.
        #[arg(long)]
        dir: PathBuf,
        /// The market: `nem`, `wem` or `gas`.
        #[arg(long, default_value = "nem")]
        market: Market,
        /// The repository root, whose `tables/{market}/` declares the tables.
        #[arg(long, default_value = ".")]
        root: PathBuf,
        /// Files read at once: one per core by default. The census is bound
        /// by the CPU; give 6 while something else writes the same disk.
        #[arg(long, default_value_t = phasor_schemas::pool::default_jobs())]
        jobs: usize,
    },
    /// List every file each census read in
    /// `{root}/sources/{market}/{YYYY-MM}.tsv`, from the sidecars of a
    /// mirror archive; exit 1 naming each file whose sidecar is missing or
    /// disagrees with the census, writing nothing.
    Sources {
        /// The mirror archive: the directory holding `www.nemweb.com.au/`,
        /// or that directory itself.
        #[arg(long)]
        archive: PathBuf,
        /// The market: `nem`, `wem` or `gas`.
        #[arg(long, default_value = "nem")]
        market: Market,
        /// Write nothing; exit 3 if any sources file would change, listing
        /// each.
        #[arg(long)]
        check: bool,
        /// The repository root.
        #[arg(long, default_value = ".")]
        root: PathBuf,
        /// Months and folders read at once: one per core by default.
        #[arg(long, default_value_t = phasor_schemas::pool::default_jobs())]
        jobs: usize,
    },
    /// Record every folder of every root of a market's feeds in
    /// `{root}/evidence/census/{market}/{feed}/{folder}.toml` (the NEM's
    /// report folders under `reports/`), a folder's listings as one.
    Reports {
        /// The directory of the host in the mirror archive:
        /// `…/archive/www.nemweb.com.au`, holding `Reports/`.
        #[arg(long)]
        dir: PathBuf,
        /// The market: `nem`, `wem` or `gas`.
        #[arg(long, default_value = "nem")]
        market: Market,
        /// One feed of the market (`gbb`, `gsh`, `sttm`, `dwgm`,
        /// `datafiles`, `wemde`); every feed when not given.
        #[arg(long)]
        feed: Option<String>,
        /// The directory of MMSDM months (`MMSDM_{YYYY}_{MM}/CTL/`) whose
        /// control files type the blocks; without it nothing is typed.
        #[arg(long)]
        mmsdm: Option<PathBuf>,
        /// The phasor-mirror home whose folder events
        /// (`state/folders/{host}/{folder}/events.ndjson`) note each
        /// folder's listings, counted for the files rewritten in place.
        #[arg(long, env = "PHASOR_MIRROR_HOME")]
        mirror: Option<PathBuf>,
        /// The repository root, whose `tables/` declares the tables.
        #[arg(long, default_value = ".")]
        root: PathBuf,
        /// Folders read at once: one per core by default; give 6 while
        /// something else writes the same disk.
        #[arg(long, default_value_t = phasor_schemas::pool::default_jobs())]
        jobs: usize,
    },
    /// Record the data-model create scripts a month's archive carries
    /// (`DOCUMENTATION/MMS Data Model/v5.x/MMSDM_create_v5.x.zip`, from
    /// 2022-10 on) in `{root}/evidence/models/{market}/{version}.toml`, one
    /// file per release with the months it shipped in, and `months.toml`. A
    /// version already recorded only widens its months. For `--market gas`,
    /// `--dir` is AEMO's Gas Data Model Report as the mirror holds it
    /// (`Reports/CURRENT/MMSDataModelReport/Gas`), read from its pages.
    Models {
        /// The month, `YYYY-MM`: the archive month, or the month the
        /// report was listed.
        #[arg(long)]
        month: YearMonth,
        /// The month's directory, or the report's.
        #[arg(long)]
        dir: PathBuf,
        /// The market: `nem` or `gas` (the WEM publishes no data model;
        /// its tables are declared by hand).
        #[arg(long, default_value = "nem")]
        market: Market,
        /// The report's version, when its pages do not say.
        #[arg(long)]
        version: Option<String>,
        /// The repository root.
        #[arg(long, default_value = ".")]
        root: PathBuf,
    },
    /// Fold every census of a market into each table's history, kept to
    /// the history `{root}/{market}/{TABLE}/metadata/v1.metadata.json`
    /// published, and write `{root}/evidence/histories/{market}/{TABLE}.json`.
    Fold {
        /// Write nothing; exit 3 if any history file would change.
        #[arg(long)]
        check: bool,
        /// Keep no published history, once: mint every id anew and record
        /// the next data version with this reason.
        #[arg(long, value_name = "REASON")]
        rebaseline: Option<String>,
        /// The market: `nem`, `wem` or `gas`.
        #[arg(long, default_value = "nem")]
        market: Market,
        /// The repository root, whose `tables/{market}/` declares the tables.
        #[arg(long, default_value = ".")]
        root: PathBuf,
    },
    /// Write every table of every market as
    /// `{root}/{market}/{TABLE}/metadata/` (Iceberg table metadata and its
    /// manifest lists) and `schema-{id}.json`, with
    /// `{root}/{market}/headers.json` and `{root}/manifest.json`; remove
    /// every other file under the markets' directories.
    Export {
        /// Write nothing; exit 3 if any file would change, listing each.
        #[arg(long)]
        check: bool,
        /// Where the documents are published.
        #[arg(long, default_value = phasor_schemas::export::DEFAULT_BASE_URL)]
        base_url: String,
        /// The repository root.
        #[arg(long, default_value = ".")]
        root: PathBuf,
    },
    /// Hold every table under the markets' directories to what a git
    /// revision published; exit 1 with one line per broken promise.
    Compat {
        /// The revision.
        #[arg(long)]
        base: String,
        /// The repository root.
        #[arg(long, default_value = ".")]
        root: PathBuf,
    },
    /// Summarise what the markets' directories add to a git revision, per
    /// table, as Markdown.
    Diff {
        /// The revision.
        #[arg(long)]
        base: String,
        /// The repository root.
        #[arg(long, default_value = ".")]
        root: PathBuf,
    },
    /// Print the DuckDB query that reads an interval of a table's files
    /// across its schemas.
    Sql {
        /// The table.
        table: String,
        /// The market: `nem`, `wem` or `gas`.
        #[arg(long, default_value = "nem")]
        market: Market,
        /// The data files, `[{path, schema_id, min_us, max_us}]`.
        #[arg(long)]
        files: PathBuf,
        /// The interval's start: RFC 3339, or a day in market time.
        #[arg(long)]
        from: String,
        /// Its end, not included.
        #[arg(long)]
        to: String,
        /// The schema to read as; the latest when not given.
        #[arg(long)]
        as_of: Option<u32>,
        /// The repository root.
        #[arg(long, default_value = ".")]
        root: PathBuf,
    },
    /// Write every history as an Iceberg table without data under
    /// `{out}/{market}/{TABLE}`.
    Iceberg {
        /// Where the tables go.
        #[arg(long)]
        out: PathBuf,
        /// The repository root.
        #[arg(long, default_value = ".")]
        root: PathBuf,
    },
}

/// The HTTP client of `fetch`.
struct Ureq(ureq::Agent);

impl Http for Ureq {
    fn get(&mut self, url: &str, user_agent: &str) -> std::io::Result<Response> {
        let r = self
            .0
            .get(url)
            .header("User-Agent", user_agent)
            .call()
            .map_err(std::io::Error::other)?;
        let last_modified = r
            .headers()
            .get("last-modified")
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned);
        Ok(Response {
            status: r.status().as_u16(),
            last_modified,
            body: Box::new(r.into_body().into_reader()),
        })
    }
}

/// The git command line, run in the repository root.
struct GitCli<'a>(&'a Path);

impl GitCli<'_> {
    fn run(&self, args: &[&str]) -> std::io::Result<Vec<u8>> {
        // The repository of the root, not one a git hook running us names.
        let out = std::process::Command::new("git")
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE")
            .arg("-C")
            .arg(self.0)
            .args(args)
            .output()?;
        if !out.status.success() {
            let err = String::from_utf8_lossy(&out.stderr);
            return Err(std::io::Error::other(err.trim().to_owned()));
        }
        Ok(out.stdout)
    }
}

impl phasor_schemas::published::Git for GitCli<'_> {
    fn list(&self, rev: &str, dir: &str) -> std::io::Result<Vec<String>> {
        // Paths relative to the root, as the root's own files are named.
        let out = self.run(&["ls-tree", "-r", "--name-only", "-z", rev, "--", dir])?;
        Ok(out
            .split(|b| *b == 0)
            .filter(|p| !p.is_empty())
            .map(|p| String::from_utf8_lossy(p).into_owned())
            .collect())
    }

    fn show(&self, rev: &str, path: &str) -> std::io::Result<Vec<u8>> {
        self.run(&["show", &format!("{rev}:./{path}")])
    }
}

/// What `rev` published and what the working tree holds.
fn published(
    root: &Path,
    rev: &str,
) -> Result<
    (
        phasor_schemas::published::Published,
        phasor_schemas::published::Published,
    ),
    ExitCode,
> {
    use phasor_schemas::published::Published;
    let now = Published::read(root).map_err(|e| fail(&e))?;
    let base = Published::held_to(&GitCli(root), rev, &now).map_err(|e| fail(&e))?;
    Ok((base, now))
}

/// An error and its causes, as one line each.
fn fail(e: &dyn std::error::Error) -> ExitCode {
    eprintln!("error: {e}");
    let mut cause = e.source();
    while let Some(c) = cause {
        eprintln!("  caused by: {c}");
        cause = c.source();
    }
    ExitCode::FAILURE
}

fn scratch() -> Result<tempfile::TempDir, ExitCode> {
    tempfile::tempdir().map_err(|e| fail(&e))
}

/// The market's declarations; none yet when it has no `tables/{market}/`.
fn catalog(root: &Path, market: Market) -> Result<Catalog, ExitCode> {
    let dir = tables_dir(root, market);
    if !dir.exists() {
        return Catalog::new(Vec::new()).map_err(|e| fail(&e));
    }
    Catalog::read(&dir).map_err(|e| fail(&e))
}

fn main() -> ExitCode {
    match run(Cli::parse().cmd) {
        Ok(code) | Err(code) => code,
    }
}

fn run(cmd: Cmd) -> Result<ExitCode, ExitCode> {
    match cmd {
        Cmd::Fetch {
            month,
            dir,
            interval_ms,
            root,
        } => {
            let agent = ureq::Agent::config_builder()
                .http_status_as_error(false)
                .timeout_connect(Some(Duration::from_secs(30)))
                .build()
                .into();
            let interval = Duration::from_millis(interval_ms);
            let done = phasor_schemas::fetch::fetch_month(
                &mut Ureq(agent),
                phasor_schemas::fetch::BASE,
                month,
                &dir,
                &root,
                interval,
            )
            .map_err(|e| fail(&e))?;
            println!(
                "{month}: {} file(s) listed, {} fetched ({} bytes), {} already there, {} request(s)",
                done.listed, done.fetched, done.bytes, done.skipped, done.requests
            );
        }
        Cmd::Census {
            month,
            dir,
            market,
            root,
            jobs,
        } => {
            let cat = catalog(&root, market)?;
            let files = phasor_schemas::census::month_files(&dir).map_err(|e| fail(&e))?;
            let tmp = scratch()?;
            let census =
                phasor_schemas::census::census_month(&cat, month, &files, tmp.path(), jobs);
            for p in &census.problems {
                eprintln!("error: {}: {}", p.file, p.error);
            }
            let blocks =
                phasor_schemas::census::record(&root, market, &census).map_err(|e| fail(&e))?;
            println!(
                "{month}: {} file(s), {blocks} block(s), {} unread",
                census.files.len(),
                census.problems.len()
            );
            if !census.problems.is_empty() {
                return Ok(ExitCode::from(3));
            }
        }
        Cmd::Sources {
            archive,
            market,
            check,
            root,
            jobs,
        } => {
            use phasor_schemas::sources;
            let s = sources::collect(&root, market, &archive, jobs).map_err(|e| fail(&e))?;
            for m in &s.mismatches {
                eprintln!("error: {m}");
            }
            let counts = format!(
                "{} month(s), {} file(s) listed, {} mismatch(es)",
                s.months.len(),
                s.files(),
                s.mismatches.len()
            );
            if !s.mismatches.is_empty() {
                println!("{counts}; nothing written");
                return Ok(ExitCode::FAILURE);
            }
            if check {
                let stale = sources::stale(&root, market, &s);
                for p in &stale {
                    eprintln!("stale: {}", p.display());
                }
                println!("{counts}; {} file(s) would change", stale.len());
                if !stale.is_empty() {
                    return Ok(ExitCode::from(3));
                }
            } else {
                let n = sources::write(&root, market, &s).map_err(|e| fail(&e))?;
                println!("{counts}; {n} file(s) written");
            }
        }
        Cmd::Reports {
            dir,
            market,
            feed,
            mmsdm,
            mirror,
            root,
            jobs,
        } => {
            use phasor_schemas::reports;
            if let Some(f) = &feed
                && !reports::feeds_of(market.id()).any(|x| x.id == f)
            {
                eprintln!("error: {market} has no feed {f:?}");
                return Ok(ExitCode::FAILURE);
            }
            let cat = catalog(&root, market)?;
            let tmp = scratch()?;

            let taken = reports::census_reports(
                &cat,
                &dir,
                (mmsdm.as_deref(), mirror.as_deref()),
                (&root, market),
                tmp.path(),
                jobs,
                feed.as_deref(),
            )
            .map_err(|e| fail(&e))?;
            let mut unread = 0;
            for t in &taken {
                for u in &t.census.unread {
                    eprintln!("error: {}: {} {}", t.folder, u.file, u.error);
                }
                unread += t.census.unread.len();
            }
            let runs: usize = taken.iter().map(|t| t.census.blocks.len()).sum();
            println!("{} folder(s), {runs} run(s), {unread} unread", taken.len());
            if unread > 0 {
                return Ok(ExitCode::from(3));
            }
        }
        Cmd::Models {
            month,
            dir,
            market,
            version,
            root,
        } => {
            let (r, read) = match market {
                Market::Nem => {
                    let files = phasor_schemas::census::files_under(&dir).map_err(|e| fail(&e))?;
                    let zips = phasor_schemas::models::model_zips(&files);
                    let r = phasor_schemas::models::record(&root, market, month, &zips)
                        .map_err(|e| fail(&e))?;
                    (r, format!("{} model zip(s)", zips.len()))
                }
                Market::Gas => {
                    let r = phasor_schemas::report_model::record(
                        &root,
                        market,
                        month,
                        &dir,
                        version.as_deref(),
                    )
                    .map_err(|e| fail(&e))?;
                    (r, "the data model report".to_owned())
                }
                Market::Wem => {
                    eprintln!(
                        "error: the WEM publishes no data model; declare its tables by hand under tables/wem/"
                    );
                    return Ok(ExitCode::FAILURE);
                }
            };
            for p in &r.problems {
                eprintln!("error: {p}");
            }
            println!(
                "{month}: {read}, versions [{}], {} new",
                r.versions.join(", "),
                r.new_versions.len()
            );
            if !r.problems.is_empty() {
                return Ok(ExitCode::from(3));
            }
        }
        Cmd::Fold {
            check,
            rebaseline,
            market,
            root,
        } => {
            let cat = catalog(&root, market)?;
            let f = phasor_schemas::fold::fold(&root, market, &cat, check, rebaseline.as_deref())
                .map_err(|e| fail(&e))?;
            for note in &f.notes {
                eprintln!("note: {note}");
            }
            println!(
                "{} table(s), {} schema(s), {} run(s), {} break(s); {} file(s) {}",
                f.tables,
                f.schemas,
                f.runs,
                f.breaks,
                f.changed,
                if check { "would change" } else { "changed" }
            );
            if check && f.changed > 0 {
                return Ok(ExitCode::from(3));
            }
        }
        Cmd::Export {
            check,
            base_url,
            root,
        } => {
            use phasor_schemas::export;
            let r = export::Export::load(&root, &base_url)
                .map_err(|e| fail(&e))?
                .render()
                .map_err(|e| fail(&e))?;
            if check {
                let stale = export::check(&root, &r).map_err(|e| fail(&e))?;
                for s in &stale {
                    eprintln!("stale: {s}");
                }
                println!(
                    "{} table(s), {} schema(s); {} file(s) would change",
                    r.tables,
                    r.schemas,
                    stale.len()
                );
                if !stale.is_empty() {
                    return Ok(ExitCode::from(3));
                }
            } else {
                let w = export::write(&root, &r).map_err(|e| fail(&e))?;
                println!(
                    "{} table(s), {} schema(s); {} file(s) written, {} kept, {} removed",
                    r.tables, r.schemas, w.written, w.kept, w.removed
                );
            }
        }
        Cmd::Compat { base, root } => {
            let (old, now) = published(&root, &base)?;
            if old.tables.is_empty() && old.schemas.is_empty() && old.unread.is_empty() {
                println!("compat: {base} published no table; nothing to hold to");
                return Ok(ExitCode::SUCCESS);
            }
            let c = phasor_schemas::compat::compat(&base, &old, &now);
            for v in &c.violations {
                println!("{v}");
            }
            if !c.violations.is_empty() {
                return Ok(ExitCode::FAILURE);
            }
            for (market, (was, is)) in &c.rebased {
                println!(
                    "compat: {market}'s data version {was} at {base} was re-based to {is}: {}; nothing of it held",
                    now.baseline(*market).reason
                );
            }
            if c.rebased.is_empty() {
                println!(
                    "compat: the {} table(s) and {} schema document(s) {base} published hold",
                    c.tables, c.schemas
                );
            }
        }
        Cmd::Diff { base, root } => {
            let (old, now) = published(&root, &base)?;
            print!("{}", phasor_schemas::diff::diff(&base, &old, &now));
        }
        Cmd::Sql {
            table,
            market,
            files,
            from,
            to,
            as_of,
            root,
        } => {
            let q = phasor_schemas::sql::sql(&root, market, &table, &files, (&from, &to), as_of)
                .map_err(|e| fail(&e))?;
            print!("{q}");
        }
        Cmd::Iceberg { out, root } => {
            let n = phasor_schemas::iceberg::write_tables(&root, &out).map_err(|e| fail(&e))?;
            println!("{n} table(s) written under {}", out.display());
        }
    }
    Ok(ExitCode::SUCCESS)
}
