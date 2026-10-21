//! `nem-schemas`: fetch a month of AEMO's archive, take its census, take
//! the census of the report folders; and, once, write the table
//! declarations from utilake's table files.
//!
//! ```text
//! nem-schemas fetch --month 2024-01 --dir mirror/MMSDM_2024_01 [--interval-ms 1000] [--root .]
//! nem-schemas census --month 2024-01 --dir mirror/MMSDM_2024_01 [--root .]
//! nem-schemas reports --dir mirror/Reports [--mmsdm mirror] [--root .]
//! nem-schemas tables-from-utilake --from ../utilake/schemas/nem --out tables
//! ```

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use clap::{Parser, Subcommand};
use nem_schemas::catalog::Catalog;
use nem_schemas::fetch::{DEFAULT_INTERVAL_MS, Http, Response};
use nem_schemas::month::YearMonth;

#[derive(Parser)]
#[command(
    name = "nem-schemas",
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
    /// `{root}/evidence/census/{YYYY-MM}.toml`.
    Census {
        /// The month, `YYYY-MM`.
        #[arg(long)]
        month: YearMonth,
        /// The month's directory.
        #[arg(long)]
        dir: PathBuf,
        /// The repository root, whose `tables/` declares the tables.
        #[arg(long, default_value = ".")]
        root: PathBuf,
    },
    /// Record every folder of the report roots in
    /// `{root}/evidence/census/reports/{ROOT}/{folder}.toml`.
    Reports {
        /// The directory holding `ARCHIVE/` and `CURRENT/`.
        #[arg(long)]
        dir: PathBuf,
        /// The directory of MMSDM months (`MMSDM_{YYYY}_{MM}/CTL/`) whose
        /// control files type the blocks; without it nothing is typed.
        #[arg(long)]
        mmsdm: Option<PathBuf>,
        /// The repository root, whose `tables/` declares the tables.
        #[arg(long, default_value = ".")]
        root: PathBuf,
    },
    /// Write `tables/*.toml` from utilake's table files, once.
    TablesFromUtilake {
        /// utilake's `schemas/nem`.
        #[arg(long)]
        from: PathBuf,
        /// Where the declarations go.
        #[arg(long)]
        out: PathBuf,
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

fn catalog(root: &Path) -> Result<Catalog, ExitCode> {
    Catalog::read(&root.join("tables")).map_err(|e| fail(&e))
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
            let done = nem_schemas::fetch::fetch_month(
                &mut Ureq(agent),
                nem_schemas::fetch::BASE,
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
        Cmd::Census { month, dir, root } => {
            let cat = catalog(&root)?;
            let files = nem_schemas::census::month_files(&dir).map_err(|e| fail(&e))?;
            let tmp = scratch()?;
            let census = nem_schemas::census::census_month(&cat, month, &files, tmp.path());
            for p in &census.problems {
                eprintln!("error: {}: {}", p.file, p.error);
            }
            let blocks = nem_schemas::census::record(&root, &census).map_err(|e| fail(&e))?;
            println!(
                "{month}: {} file(s), {blocks} block(s), {} unread",
                census.files.len(),
                census.problems.len()
            );
            if !census.problems.is_empty() {
                return Ok(ExitCode::from(3));
            }
        }
        Cmd::Reports { dir, mmsdm, root } => {
            let cat = catalog(&root)?;
            let tmp = scratch()?;
            let taken = nem_schemas::reports::census_reports(
                &cat,
                &dir,
                mmsdm.as_deref(),
                &root,
                tmp.path(),
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
        Cmd::TablesFromUtilake { from, out } => {
            let cat = nem_schemas::catalog::from_utilake(&from).map_err(|e| fail(&e))?;
            let n = cat.write(&out).map_err(|e| fail(&e))?;
            println!("{n} table declaration(s) written to {}", out.display());
        }
    }
    Ok(ExitCode::SUCCESS)
}
