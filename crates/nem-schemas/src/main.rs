//! `nem-schemas`: fetch a month of AEMO's archive, take its census, take
//! the census of the report folders, record a month's data-model releases,
//! fold every census into each table's history, export the histories as
//! the documents under `nem/`, hold them to a git revision, plan a read of
//! a table, write the histories as Iceberg tables; and, once, write the
//! table declarations from utilake's table files.
//!
//! ```text
//! nem-schemas fetch --month 2024-01 --dir mirror/MMSDM_2024_01 [--interval-ms 1000] [--root .]
//! nem-schemas census --month 2024-01 --dir mirror/MMSDM_2024_01 [--root .]
//! nem-schemas reports --dir mirror/Reports [--mmsdm mirror] [--root .]
//! nem-schemas models --month 2025-10 --dir mirror/MMSDM_2025_10 [--root .]
//! nem-schemas fold [--check] [--root .]
//! nem-schemas export [--check] [--base-url URL] [--root .]
//! nem-schemas compat --base origin/main [--root .]
//! nem-schemas diff --base origin/main [--root .]
//! nem-schemas sql DISPATCHPRICE --files files.json --from 2024-08-01 --to 2024-08-02 [--as-of 0] [--root .]
//! nem-schemas iceberg --out target/iceberg [--root .]
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
    /// Record the data-model create scripts a month's archive carries
    /// (`DOCUMENTATION/MMS Data Model/v5.x/MMSDM_create_v5.x.zip`, from
    /// 2022-10 on) in `{root}/evidence/models/{version}.toml`, one file per
    /// release with the months it shipped in, and `months.toml`. A version
    /// already recorded only widens its months.
    Models {
        /// The month, `YYYY-MM`.
        #[arg(long)]
        month: YearMonth,
        /// The month's directory.
        #[arg(long)]
        dir: PathBuf,
        /// The repository root.
        #[arg(long, default_value = ".")]
        root: PathBuf,
    },
    /// Fold every census into each table's history, kept to the history
    /// `{root}/nem/{TABLE}/table.json` published, and write
    /// `{root}/evidence/histories/{TABLE}.json`.
    Fold {
        /// Write nothing; exit 3 if any history file would change.
        #[arg(long)]
        check: bool,
        /// The repository root, whose `tables/` declares the tables.
        #[arg(long, default_value = ".")]
        root: PathBuf,
    },
    /// Write every history as `{root}/nem/{TABLE}/table.json` and
    /// `schema-{hash}.json`, and `{root}/manifest.json`; remove every other
    /// file under `{root}/nem/`.
    Export {
        /// Write nothing; exit 3 if any file would change, listing each.
        #[arg(long)]
        check: bool,
        /// Where the documents are published.
        #[arg(long, default_value = nem_schemas::export::DEFAULT_BASE_URL)]
        base_url: String,
        /// The repository root.
        #[arg(long, default_value = ".")]
        root: PathBuf,
    },
    /// Hold every table under `{root}/nem/` to what a git revision
    /// published; exit 1 with one line per broken promise.
    Compat {
        /// The revision.
        #[arg(long)]
        base: String,
        /// The repository root.
        #[arg(long, default_value = ".")]
        root: PathBuf,
    },
    /// Summarise what `{root}/nem/` adds to a git revision, per table, as
    /// Markdown.
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
    /// `{out}/{TABLE}`.
    Iceberg {
        /// Where the tables go.
        #[arg(long)]
        out: PathBuf,
        /// The repository root.
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

impl nem_schemas::published::Git for GitCli<'_> {
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
        nem_schemas::published::Published,
        nem_schemas::published::Published,
    ),
    ExitCode,
> {
    use nem_schemas::published::Published;
    let base = Published::at(&GitCli(root), rev).map_err(|e| fail(&e))?;
    let now = Published::read(root).map_err(|e| fail(&e))?;
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
        Cmd::Models { month, dir, root } => {
            let files = nem_schemas::census::files_under(&dir).map_err(|e| fail(&e))?;
            let zips = nem_schemas::models::model_zips(&files);
            let r = nem_schemas::models::record(&root, month, &zips).map_err(|e| fail(&e))?;
            for p in &r.problems {
                eprintln!("error: {p}");
            }
            println!(
                "{month}: {} model zip(s), versions [{}], {} new",
                zips.len(),
                r.versions.join(", "),
                r.new_versions.len()
            );
            if !r.problems.is_empty() {
                return Ok(ExitCode::from(3));
            }
        }
        Cmd::Fold { check, root } => {
            let cat = catalog(&root)?;
            let f = nem_schemas::fold::fold(&root, &cat, check).map_err(|e| fail(&e))?;
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
            use nem_schemas::export;
            let r = export::Export::load(&root, &base_url)
                .map_err(|e| fail(&e))?
                .render();
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
            if old.tables.is_empty() && old.schemas.is_empty() {
                println!("compat: {base} has no nem/; nothing to hold to");
                return Ok(ExitCode::SUCCESS);
            }
            let c = nem_schemas::compat::compat(&base, &old, &now);
            for v in &c.violations {
                println!("{v}");
            }
            if !c.violations.is_empty() {
                return Ok(ExitCode::FAILURE);
            }
            println!(
                "compat: the {} table(s) and {} schema document(s) {base} published hold",
                c.tables, c.schemas
            );
        }
        Cmd::Diff { base, root } => {
            let (old, now) = published(&root, &base)?;
            print!("{}", nem_schemas::diff::diff(&base, &old, &now));
        }
        Cmd::Sql {
            table,
            files,
            from,
            to,
            as_of,
            root,
        } => {
            let q = nem_schemas::sql::sql(&root, &table, &files, (&from, &to), as_of)
                .map_err(|e| fail(&e))?;
            print!("{q}");
        }
        Cmd::Iceberg { out, root } => {
            let n = nem_schemas::iceberg::write_tables(&root, &out).map_err(|e| fail(&e))?;
            println!("{n} table(s) written under {}", out.display());
        }
        Cmd::TablesFromUtilake { from, out } => {
            let cat = nem_schemas::catalog::from_utilake(&from).map_err(|e| fail(&e))?;
            let n = cat.write(&out).map_err(|e| fail(&e))?;
            println!("{n} table declaration(s) written to {}", out.display());
        }
    }
    Ok(ExitCode::SUCCESS)
}
