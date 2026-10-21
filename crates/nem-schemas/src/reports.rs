//! The census of AEMO's report folders: every block of every file under
//! `Reports/ARCHIVE` and `Reports/CURRENT`, nested zips included.
//!
//! The MMSDM archive publishes a table a month at a time; the report
//! folders publish the same tables (and others) as they are produced, so a
//! layout AEMO has just changed shows there first. Each CSV's month comes
//! from the first date in its name (`PUBLIC_DISPATCHIS_202610200245_….CSV`
//! is 2026-10), else its zip's. Its blocks are typed by the control files
//! of that MMSDM month, or the last month before it that has them; a table
//! no control file loads has columns of unknown type.
//!
//! A folder holds many thousands of five-minute files, so the census of a
//! folder is a list of runs, not of files: CSVs in name order (their names
//! carry their time) that held a table in one shape and version, with the
//! first and last CSV, how many, and the block record of them all (rows,
//! time span, and each column's declared class and observed profile,
//! merged). A run ends at any change: another shape or version, or a
//! column whose values are of a wider kind or scale than the run's so far,
//! so the CSV where anything changed is kept. It is written to
//! `{root}/evidence/census/reports/{ARCHIVE|CURRENT}/{folder}.toml`.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::catalog::Catalog;
use crate::census::{
    BlockRecord, BlockShape, CensusError, Ctls, Span, Unread, base, block_shape, census_dir,
    files_under, read_blocks, read_text, write_text,
};
use crate::month::YearMonth;
use crate::shape::TypeClass;
use crate::zip::{ArchiveError, WalkError, walk_csv_entries};

/// The report roots under `Reports/`.
pub const ROOTS: [&str; 2] = ["ARCHIVE", "CURRENT"];

/// How far back a report month looks for an MMSDM month's control files.
const CTL_LOOKBACK_MONTHS: usize = 24;

/// The kind of a file of a report root: `reports/archive` or
/// `reports/current`.
pub fn kind(root: &str) -> String {
    format!("reports/{}", root.to_ascii_lowercase())
}

/// CSVs in a row that held one table in one shape and version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tally {
    /// The month of its first CSV.
    pub month: YearMonth,
    /// CSVs that held a block of it.
    pub csvs: u64,
    /// The first CSV, by name.
    pub first_csv: String,
    /// The last CSV, by name.
    pub last_csv: String,
    /// The file AEMO published the first CSV in (the CSV, or its zip).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub first_file: String,
    /// Its SHA-256.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub first_sha256: String,
    /// The file the last CSV was published in.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub last_file: String,
    /// Its SHA-256.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub last_sha256: String,
    /// Their blocks as one: rows summed, span and profiles merged.
    #[serde(flatten)]
    pub block: BlockRecord,
}

/// The census of one folder, as written.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FolderCensus {
    /// Files read.
    pub files: u64,
    /// CSVs read out of them.
    pub csvs: u64,
    /// Files that are neither zip nor CSV (a PDF, a web page).
    pub other: u64,
    /// Files or entries that could not be read.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unread: Vec<Unread>,
    /// Every run, in the order its first CSV was read.
    #[serde(default, rename = "block")]
    pub blocks: Vec<Tally>,
}

/// The first month a name gives: the first run of at least six digits
/// that starts with a year from 1990 to 2099 and a month 01 to 12.
#[must_use]
pub fn name_month(name: &str) -> Option<YearMonth> {
    let b = name.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if !b[i].is_ascii_digit() {
            i += 1;
            continue;
        }
        let start = i;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
        if i - start >= 6 {
            let year: i32 = name[start..start + 4].parse().ok()?;
            let month: u8 = name[start + 4..start + 6].parse().ok()?;
            if (1990..2100).contains(&year)
                && let Some(m) = YearMonth::new(year, month)
            {
                return Some(m);
            }
        }
    }
    None
}

/// Control files per report month, read once from a directory of MMSDM
/// months (`{dir}/MMSDM_{YYYY}_{MM}/`, as `fetch` writes them).
pub struct CtlSource {
    dir: Option<PathBuf>,
    by_month: HashMap<YearMonth, Ctls>,
    read: HashMap<YearMonth, Option<Vec<(String, String)>>>,
}

impl CtlSource {
    /// Control files of the MMSDM months under `dir`; none without one.
    #[must_use]
    pub fn new(dir: Option<&Path>) -> Self {
        Self {
            dir: dir.map(Path::to_path_buf),
            by_month: HashMap::new(),
            read: HashMap::new(),
        }
    }

    /// The control files of `month`, else of the last MMSDM month before
    /// it that has them.
    pub fn for_month(&mut self, month: YearMonth) -> &Ctls {
        if !self.by_month.contains_key(&month) {
            let mut texts = Vec::new();
            let mut m = month;
            for _ in 0..CTL_LOOKBACK_MONTHS {
                if let Some(t) = self.month_texts(m) {
                    texts = t;
                    break;
                }
                m = m.prev();
            }
            let ctls = Ctls::parse(texts.iter().map(|(n, t)| (n.as_str(), t.as_str())));
            self.by_month.insert(month, ctls);
        }
        &self.by_month[&month]
    }

    /// The loose control files of a month directory: its `CTL/`, as
    /// `fetch` lays it out or as nemweb does.
    fn month_texts(&mut self, month: YearMonth) -> Option<Vec<(String, String)>> {
        if !self.read.contains_key(&month) {
            let name = format!("MMSDM_{}_{:02}", month.year(), month.month());
            let texts = self.dir.as_ref().and_then(|d| {
                let m = d.join(name);
                [m.join("CTL"), m.join("MMSDM_Historical_Data_SQLLoader/CTL")]
                    .iter()
                    .find_map(|d| ctl_texts(d))
            });
            self.read.insert(month, texts);
        }
        self.read[&month].clone()
    }
}

/// The control files directly in `dir`, by name; `None` for none.
fn ctl_texts(dir: &Path) -> Option<Vec<(String, String)>> {
    let mut t: Vec<(String, String)> = std::fs::read_dir(dir)
        .ok()?
        .filter_map(Result::ok)
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let wanted = !name.starts_with("._") && crate::zip::has_extension(&name, ".ctl");
            let bytes = wanted.then(|| std::fs::read(e.path()).ok()).flatten()?;
            Some((name, String::from_utf8_lossy(&bytes).into_owned()))
        })
        .collect();
    t.sort();
    (!t.is_empty()).then_some(t)
}

/// One run, gathering.
struct Acc {
    month: YearMonth,
    csvs: u64,
    first_csv: String,
    last_csv: String,
    /// The files the first and last CSV were read out of.
    first_path: PathBuf,
    last_path: PathBuf,
    merged: BlockShape,
}

/// What a folder's census gathers before it is written.
#[derive(Default)]
struct Gather {
    census: FolderCensus,
    runs: Vec<Acc>,
    /// The open run of each report pair.
    open: HashMap<(String, String), usize>,
    undated: u64,
}

/// Whether `b` continues run `a`: the same shape, version and declared
/// classes, and no column of a wider kind or scale than the run has seen.
fn continues(a: &Acc, b: &BlockShape) -> bool {
    let m = &a.merged;
    m.shape.id() == b.shape.id()
        && m.version == b.version
        && m.ctl == b.ctl
        && m.profile
            .iter()
            .zip(&b.profile)
            .all(|(p, q)| p.kind.join(q.kind) == p.kind && q.scale <= p.scale)
}

impl Gather {
    fn add(&mut self, month: YearMonth, csv: &str, path: &Path, b: BlockShape) {
        let key = (b.shape.report_type.clone(), b.shape.report_subtype.clone());
        let open = self.open.get(&key).copied();
        let Some(i) = open.filter(|i| continues(&self.runs[*i], &b)) else {
            self.open.insert(key, self.runs.len());
            self.runs.push(Acc {
                month,
                csvs: 1,
                first_csv: csv.to_owned(),
                last_csv: csv.to_owned(),
                first_path: path.to_path_buf(),
                last_path: path.to_path_buf(),
                merged: b,
            });
            return;
        };
        let a = &mut self.runs[i];
        a.csvs += 1;
        if csv < a.first_csv.as_str() {
            csv.clone_into(&mut a.first_csv);
            path.clone_into(&mut a.first_path);
        }
        if csv > a.last_csv.as_str() {
            csv.clone_into(&mut a.last_csv);
            path.clone_into(&mut a.last_path);
        }
        let m = &mut a.merged;
        m.rows += b.rows;
        m.span = match (m.span.take(), b.span) {
            (Some(x), Some(y)) => Some(Span {
                first: x.first.min(y.first),
                last: x.last.max(y.last),
                column: x.column,
            }),
            (x, y) => x.or(y),
        };
        for (p, q) in m.profile.iter_mut().zip(&b.profile) {
            p.merge(q);
        }
        for (c, d) in m.ctl.iter_mut().zip(b.ctl) {
            if d != TypeClass::Unknown {
                *c = d;
            }
        }
    }
}

/// The census of the `files` of one folder.
pub fn census_folder(
    catalog: &Catalog,
    ctls: &mut CtlSource,
    files: &[PathBuf],
    tmp: &Path,
) -> FolderCensus {
    let mut g = Gather::default();
    let mut scan_buf = Vec::new();
    for path in files {
        let name = base(&path.to_string_lossy()).to_owned();
        let outer_month = name_month(&name);
        g.census.files += 1;
        let mut one_csv =
            |csv: &str, r: &mut dyn std::io::Read, g: &mut Gather| -> Result<(), String> {
                let blocks = read_blocks(r, &mut scan_buf).map_err(|e| e.to_string())?;
                g.census.csvs += 1;
                let month = name_month(base(csv)).or(outer_month);
                let Some(month) = month else {
                    g.undated += 1;
                    return Ok(());
                };
                let c = ctls.for_month(month);
                for raw in blocks {
                    let b = block_shape(catalog, c, None, raw);
                    g.add(month, base(csv), path, b);
                }
                Ok(())
            };
        let is = |ext: &str| crate::zip::has_extension(&name, ext);
        if is(".csv") {
            let res = std::fs::File::open(path)
                .map_err(|e| e.to_string())
                .and_then(|mut f| one_csv(&name, &mut f, &mut g));
            if let Err(error) = res {
                g.census.unread.push(Unread { file: name, error });
            }
        } else if is(".zip") {
            let mut entry_err: Option<Unread> = None;
            let walked = walk_csv_entries::<String>(
                path,
                tmp,
                &mut |e| {
                    one_csv(&e.name, e.reader, &mut g).inspect_err(|error| {
                        entry_err = Some(Unread {
                            file: format!("{name}!{}", e.name),
                            error: error.clone(),
                        });
                    })
                },
                &mut |_| {},
            );
            match walked {
                Ok(_) | Err(WalkError::Archive(ArchiveError::NoCsv { .. })) => {}
                Err(WalkError::Archive(a)) => {
                    g.census.unread.push(Unread {
                        file: name,
                        error: a.to_string(),
                    });
                }
                Err(WalkError::Callback(_)) => g.census.unread.extend(entry_err),
            }
        } else {
            g.census.other += 1;
        }
    }
    if g.undated > 0 {
        g.census.unread.push(Unread {
            file: String::new(),
            error: format!("{} CSV(s) whose month no name gives", g.undated),
        });
    }
    // A run's first and last files are hashed, for the marks that name
    // them; the files between are counted, not hashed.
    let hash = |p: &Path| {
        std::fs::File::open(p)
            .and_then(crate::files::sha256_of)
            .map(|(sha, _)| sha)
            .unwrap_or_default()
    };
    for a in g.runs {
        g.census.blocks.push(Tally {
            month: a.month,
            csvs: a.csvs,
            first_file: base(&a.first_path.to_string_lossy()).to_owned(),
            first_sha256: hash(&a.first_path),
            last_file: base(&a.last_path.to_string_lossy()).to_owned(),
            last_sha256: hash(&a.last_path),
            first_csv: a.first_csv,
            last_csv: a.last_csv,
            block: BlockRecord::of(&a.merged),
        });
    }
    g.census
}

/// The folders directly under `dir`, in name order (hidden ones, macOS
/// `._` files among them, left out).
#[must_use]
pub fn top_folders(dir: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|rd| {
            rd.filter_map(Result::ok)
                .map(|e| e.path())
                .filter(|p| p.is_dir())
                .filter(|p| {
                    p.file_name()
                        .is_some_and(|n| !n.to_string_lossy().starts_with('.'))
                })
                .collect()
        })
        .unwrap_or_default();
    out.sort();
    out
}

/// Every folder at or under `under` that holds files, with its files in
/// name order, keyed by its path below `dir` (`/`-separated).
pub fn folders(dir: &Path, under: &Path) -> Result<BTreeMap<String, Vec<PathBuf>>, CensusError> {
    let mut out: BTreeMap<String, Vec<PathBuf>> = BTreeMap::new();
    for path in files_under(under)? {
        let parent = path.parent().unwrap_or(dir);
        let rel = parent.strip_prefix(dir).unwrap_or(parent);
        let key = rel
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("/");
        out.entry(key).or_default().push(path);
    }
    Ok(out)
}

/// Where the census of folder `rel` of report root `root` is written.
#[must_use]
pub fn folder_path(repo: &Path, root: &str, rel: &str) -> PathBuf {
    let name = if rel.is_empty() { "_root" } else { rel };
    census_dir(repo)
        .join("reports")
        .join(root)
        .join(format!("{name}.toml"))
}

/// Writes the census of a folder.
pub fn record_folder(
    repo: &Path,
    root: &str,
    rel: &str,
    census: &FolderCensus,
) -> Result<(), CensusError> {
    let text = format!(
        "# Generated by `nem-schemas reports`: every shape of Reports/{root}/{rel}, per month, and what each column held.\n{}",
        toml::to_string(census).expect("a folder census serialises")
    );
    write_text(&folder_path(repo, root, rel), &text)
}

/// The census of a folder as written, if it was.
pub fn read_folder(
    repo: &Path,
    root: &str,
    rel: &str,
) -> Result<Option<FolderCensus>, CensusError> {
    let path = folder_path(repo, root, rel);
    let Some(text) = read_text(&path)? else {
        return Ok(None);
    };
    toml::from_str(&text)
        .map(Some)
        .map_err(|e| CensusError::Read {
            path: path.display().to_string(),
            message: e.to_string(),
        })
}

/// One folder's census as taken by [`census_reports`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Taken {
    /// `Reports/{ROOT}/{folder}`.
    pub folder: String,
    /// Its census.
    pub census: FolderCensus,
}

/// The census of every folder of both report roots under `reports` (the
/// directory holding `ARCHIVE/` and `CURRENT/`), each recorded under
/// `repo` as it is taken. `mmsdm` holds the MMSDM months whose control
/// files type the blocks; `tmp` takes nested zips while they are read.
pub fn census_reports(
    catalog: &Catalog,
    reports: &Path,
    mmsdm: Option<&Path>,
    repo: &Path,
    tmp: &Path,
    jobs: usize,
) -> Result<Vec<Taken>, CensusError> {
    let mut work = Vec::new();
    for root in ROOTS {
        let dir = reports.join(root);
        for top in top_folders(&dir) {
            for (rel, files) in folders(&dir, &top)? {
                work.push((root, rel, files));
            }
        }
    }
    // Folders on `jobs` threads, each thread with its own control files
    // and spool directory; taken in the order of the folders.
    let ctls: Vec<std::sync::Mutex<CtlSource>> = (0..jobs.max(1))
        .map(|_| std::sync::Mutex::new(CtlSource::new(mmsdm)))
        .collect();
    let taken = crate::pool::map(jobs, &work, |w, (root, rel, files)| {
        let spool = tmp.join(format!("thread-{w}"));
        let _ = std::fs::create_dir_all(&spool);
        let mut c = ctls[w]
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let census = census_folder(catalog, &mut c, files, &spool);
        record_folder(repo, root, rel, &census)?;
        let folder = format!("Reports/{root}/{rel}");
        Ok(Taken { folder, census })
    });
    taken.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::tests::scada;
    use crate::census::tests::{SCADA_CTL, put, three_tables, zip_of};

    #[test]
    fn a_name_gives_its_first_month() {
        let m = |s: &str| name_month(s).map(|m| m.to_string());
        assert_eq!(
            m("PUBLIC_DISPATCHIS_202610200245_0000000543803367.zip").as_deref(),
            Some("2026-10")
        );
        assert_eq!(m("PUBLIC_BILLING_20250902.zip").as_deref(), Some("2025-09"));
        assert_eq!(
            m("FILE01_123_202509.CSV").as_deref(),
            Some("2025-09"),
            "short runs are skipped"
        );
        assert_eq!(m("X_000000123456_201213.CSV"), None, "year 0000, month 13");
        assert_eq!(
            m("X_209913_200001.CSV").as_deref(),
            Some("2000-01"),
            "month 13 is not a month"
        );
        assert_eq!(m("PUBLIC_TER_DAILY.zip"), None);
        assert_eq!(kind("ARCHIVE"), "reports/archive");
        assert_eq!(kind("CURRENT"), "reports/current");
    }

    #[test]
    fn a_folder_is_a_list_of_runs_and_recorded() {
        let dir = tempfile::tempdir().unwrap();
        let cat = scada();
        // The control file of 2026-08 types the SCADA block of 2026-08 and,
        // looking back, of 2026-10; nothing types 2026-07. 2026-09 has a
        // CTL folder of no control file, so it is looked past.
        let mmsdm = dir.path().join("mmsdm");
        put(
            &mmsdm,
            "MMSDM_2026_08/MMSDM_Historical_Data_SQLLoader/CTL/PUBLIC_ARCHIVE#DISPATCH_UNIT_SCADA#FILE01#202608.ctl",
            SCADA_CTL.as_bytes(),
        );
        put(
            &mmsdm,
            "MMSDM_2026_08/MMSDM_Historical_Data_SQLLoader/CTL/._x.ctl",
            b"x",
        );
        put(&mmsdm, "MMSDM_2026_09/CTL/README.txt", b"x");
        let reports = dir.path().join("Reports");
        let folder = "ARCHIVE/Dispatch_SCADA";
        // A day's zip of five-minute zips, each a CSV of three blocks.
        let inner = |stamp: &str, rows: usize| {
            zip_of(&[(
                &format!("PUBLIC_DISPATCHSCADA_{stamp}_0000000001.CSV"),
                &three_tables(rows),
            )])
        };
        // Out of name order: the run's first CSV is the least by name.
        let day = zip_of(&[
            (
                "PUBLIC_DISPATCHSCADA_202608010010.zip",
                &inner("202608010010", 3),
            ),
            (
                "PUBLIC_DISPATCHSCADA_202608010005.zip",
                &inner("202608010005", 2),
            ),
        ]);
        put(
            &reports,
            &format!("{folder}/PUBLIC_DISPATCHSCADA_20260801.zip"),
            &day,
        );
        put(
            &reports,
            &format!("{folder}/PUBLIC_DISPATCHSCADA_202610200005_1.CSV"),
            &three_tables(4),
        );
        put(
            &reports,
            &format!("{folder}/PUBLIC_DISPATCHSCADA_202607310005_1.CSV"),
            &three_tables(1),
        );
        // A value with a second decimal place: a wider scale starts a run.
        let finer = String::from_utf8(three_tables(1))
            .unwrap()
            .replace(",0.5\r", ",0.25\r");
        put(
            &reports,
            &format!("{folder}/PUBLIC_DISPATCHSCADA_202610200010_1.CSV"),
            finer.as_bytes(),
        );
        // A zip named for its month, of a CSV that is not.
        put(
            &reports,
            &format!("{folder}/PUBLIC_DISPATCHSCADA_20261021.zip"),
            &zip_of(&[("PUBLIC_UNDATED.CSV", &three_tables(1))]),
        );
        // No date in its name: no month, so not counted in a run.
        put(
            &reports,
            &format!("{folder}/PUBLIC_UNDATED.CSV"),
            &three_tables(1),
        );
        put(&reports, &format!("{folder}/notes.pdf"), b"%PDF");
        put(
            &reports,
            &format!("{folder}/PUBLIC_BAD_20260801.zip"),
            b"not a zip",
        );
        put(
            &reports,
            &format!("{folder}/PUBLIC_BAD_20260802.CSV"),
            b"D,A,B,1,x\n",
        );
        put(
            &reports,
            &format!("{folder}/PUBLIC_BADENTRY_20260803.zip"),
            &zip_of(&[("PUBLIC_BADENTRY_20260803.CSV", b"D,A,B,1,x\n")]),
        );
        put(
            &reports,
            &format!("{folder}/PUBLIC_NOCSV_20260803.zip"),
            &zip_of(&[("README.txt", b"x")]),
        );
        put(
            &reports,
            "ARCHIVE/GSH/Benchmark_Price/PUBLIC_GSH_20260801.CSV",
            &three_tables(1),
        );
        put(&reports, "CURRENT/._Dispatch_SCADA/x.CSV", &three_tables(1));

        let root_dir = reports.join("ARCHIVE");
        let tops = top_folders(&root_dir);
        assert_eq!(tops.len(), 2);
        assert!(top_folders(&dir.path().join("none")).is_empty());
        let all = folders(&root_dir, &tops[0]).unwrap();
        assert_eq!(all.keys().collect::<Vec<_>>(), ["Dispatch_SCADA"]);
        let gsh = folders(&root_dir, &tops[1]).unwrap();
        assert_eq!(gsh.keys().collect::<Vec<_>>(), ["GSH/Benchmark_Price"]);
        assert!(folders(&root_dir, &root_dir.join("none")).is_err());
        assert!(folder_path(Path::new("s"), "ARCHIVE", "").ends_with("reports/ARCHIVE/_root.toml"));

        let tmp = tempfile::tempdir().unwrap();
        let mut ctls = CtlSource::new(Some(&mmsdm));
        assert!(
            ctls.for_month(YearMonth::new(2026, 7).unwrap())
                .for_table(&["DISPATCH_UNIT_SCADA"])
                .is_none()
        );
        let census = census_folder(&cat, &mut ctls, &all["Dispatch_SCADA"], tmp.path());
        assert_eq!((census.files, census.other), (11, 1));
        assert_eq!(census.csvs, 7, "three nested, four loose");
        let unread: Vec<&str> = census.unread.iter().map(|u| u.file.as_str()).collect();
        assert_eq!(
            unread,
            [
                "PUBLIC_BADENTRY_20260803.zip!PUBLIC_BADENTRY_20260803.CSV",
                "PUBLIC_BAD_20260801.zip",
                "PUBLIC_BAD_20260802.CSV",
                ""
            ],
            "{:?}",
            census.unread
        );
        assert_eq!(census.unread[3].error, "1 CSV(s) whose month no name gives");
        let scada: Vec<&Tally> = census
            .blocks
            .iter()
            .filter(|t| t.block.report == ("DISPATCH".into(), "UNIT_SCADA".into()))
            .collect();
        // Runs in name order: July untyped; August's two nested CSVs and
        // October's first typed alike (October looks back to August's
        // control file); October's finer value and the CSV of the zip
        // named for October 21st after it.
        let months: Vec<String> = scada.iter().map(|t| t.month.to_string()).collect();
        assert_eq!(months, ["2026-07", "2026-08", "2026-10"]);
        let aug = scada[1];
        assert_eq!((aug.csvs, aug.block.rows), (3, 9));
        assert_eq!(
            aug.first_csv,
            "PUBLIC_DISPATCHSCADA_202608010005_0000000001.CSV"
        );
        assert_eq!(aug.last_csv, "PUBLIC_DISPATCHSCADA_202610200005_1.CSV");
        assert_eq!(aug.first_sha256.len(), 64, "the run's first file is hashed");
        assert_eq!(aug.last_sha256.len(), 64);
        assert!(!aug.first_file.is_empty() && !aug.last_file.is_empty());
        // The CSVs' blocks are one record: rows summed, values merged.
        assert_eq!(aug.block.nulls, [0, 0, 0]);
        assert_eq!(aug.block.max[2], "3.5");
        assert_eq!(aug.block.first.as_deref(), Some("2026/08/01 00:05:00"));
        assert_eq!(
            scada[2].first_csv,
            "PUBLIC_DISPATCHSCADA_202610200010_1.CSV"
        );
        assert_eq!(scada[2].last_csv, "PUBLIC_UNDATED.CSV");
        assert_eq!(scada[0].block.ctl_of(0), TypeClass::Unknown);
        assert_eq!(aug.block.ctl_of(0), TypeClass::Timestamp);
        // Blocks without a span merge to none.
        let extra: Vec<&Tally> = census
            .blocks
            .iter()
            .filter(|t| t.block.report.1 == "EXTRA")
            .collect();
        assert!(extra.iter().all(|t| t.block.time.is_none()));

        // Recorded, and read back as it was.
        let repo = dir.path().join("repo");
        record_folder(&repo, "ARCHIVE", "Dispatch_SCADA", &census).unwrap();
        assert_eq!(
            read_folder(&repo, "ARCHIVE", "Dispatch_SCADA")
                .unwrap()
                .unwrap(),
            census
        );
        assert_eq!(read_folder(&repo, "ARCHIVE", "None").unwrap(), None);
        std::fs::write(folder_path(&repo, "ARCHIVE", "Broken"), "files = [").unwrap();
        let e = read_folder(&repo, "ARCHIVE", "Broken")
            .unwrap_err()
            .to_string();
        assert!(e.contains("Broken.toml cannot be read"), "{e}");
        std::fs::create_dir_all(folder_path(&repo, "ARCHIVE", "Dir")).unwrap();
        assert!(read_folder(&repo, "ARCHIVE", "Dir").is_err());

        // Every folder of both roots at once, each recorded.
        let taken = census_reports(&cat, &reports, Some(&mmsdm), &repo, tmp.path(), 2).unwrap();
        let names: Vec<&str> = taken.iter().map(|t| t.folder.as_str()).collect();
        assert_eq!(
            names,
            [
                "Reports/ARCHIVE/Dispatch_SCADA",
                "Reports/ARCHIVE/GSH/Benchmark_Price"
            ]
        );
        assert_eq!(taken[0].census, census);
        assert!(folder_path(&repo, "ARCHIVE", "GSH/Benchmark_Price").is_file());
        let blocked = dir.path().join("blocked");
        std::fs::write(&blocked, "a file").unwrap();
        assert!(census_reports(&cat, &reports, None, &blocked, tmp.path(), 2).is_err());
        // Without MMSDM months nothing is typed.
        let mut none = CtlSource::new(None);
        assert!(
            none.for_month(YearMonth::new(2026, 8).unwrap())
                .for_table(&["DISPATCH_UNIT_SCADA"])
                .is_none()
        );
    }
}
