use std::io::Write as _;

use utilake_iceberg::Kind;

use super::*;
use crate::catalog::tests::scada;

pub(crate) const SCADA_CTL: &str = r#"LOAD DATA INFILE "PUBLIC_ARCHIVE#DISPATCH_UNIT_SCADA#FILE01#202608010000.CSV" APPEND INTO TABLE DISPATCH_UNIT_SCADA
FIELDS TERMINATED BY ',' OPTIONALLY ENCLOSED BY '"' TRAILING NULLCOLS
(row_type FILLER, report_type FILLER, report_subtype FILLER, report_version FILLER,
SETTLEMENTDATE DATE "yyyy/mm/dd hh24:mi:ss", DUID CHAR(20), SCADAVALUE FLOAT EXTERNAL)
"#;

pub(crate) fn zip_of(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut cursor = std::io::Cursor::new(Vec::new());
    {
        let mut w = zip::ZipWriter::new(&mut cursor);
        for (name, bytes) in entries {
            w.start_file(*name, zip::write::SimpleFileOptions::default())
                .unwrap();
            w.write_all(bytes).unwrap();
        }
        w.finish().unwrap();
    }
    cursor.into_inner()
}

/// A file of three tables, the second and third starting after more
/// than 4 MiB of the first: what a head-only census never saw.
pub(crate) fn three_tables(scada_rows: usize) -> Vec<u8> {
    let mut csv = b"C,NEMP.WORLD,DVD,AEMO,PUBLIC,2026/09/01,00:00:00,,,\r\nI,DISPATCH,UNIT_SCADA,1,SETTLEMENTDATE,DUID,SCADAVALUE\r\n".to_vec();
    for i in 0..scada_rows {
        csv.extend_from_slice(
            format!("D,DISPATCH,UNIT_SCADA,1,\"2026/08/01 00:05:00\",UNIT{i:08},{i}.5\r\n")
                .as_bytes(),
        );
    }
    csv.extend_from_slice(b"I,UTILAKE_TEST,EXTRA,2,A,B\r\nD,UTILAKE_TEST,EXTRA,2,x,y\r\nD,UTILAKE_TEST,EXTRA,2,x,z\r\n");
    csv.extend_from_slice(b"I,UTILAKE_TEST,LAST,1,C\r\nD,UTILAKE_TEST,LAST,1,q\r\n");
    csv.extend_from_slice(format!("C,\"END OF REPORT\",{}\r\n", scada_rows + 8).as_bytes());
    csv
}

/// Writes `bytes` to `rel` under `dir`, directories and all.
pub(crate) fn put(dir: &Path, rel: &str, bytes: &[u8]) {
    let path = dir.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, bytes).unwrap();
}

/// The span of a block is the least and greatest value of its time
/// column over every row, whatever order the rows come in; the column
/// is the table's declared one, else the first time-naming one, else
/// the first dated one.
#[test]
fn a_block_spans_its_rows_by_the_time_column() {
    let cat = scada();
    let csv = b"C,x,y\r\n\
I,DISPATCH,UNIT_SCADA,1,SETTLEMENTDATE,DUID,SCADAVALUE,LASTCHANGED\r\n\
D,DISPATCH,UNIT_SCADA,1,\"2026/08/02 00:05:00\",U1,1.5,\"2026/09/01 10:00:00\"\r\n\
D,DISPATCH,UNIT_SCADA,1,\"2026/08/01 00:05:00\",U2,2.5,\"2026/09/02 10:00:00\"\r\n\
D,DISPATCH,UNIT_SCADA,1,\"2026/08/31 23:55:00\",U3,3.5,\"2026/08/31 10:00:00\"\r\n\
I,UTILAKE_TEST,BYVOCAB,1,PRICE,TRADINGDATE,EFFECTIVEDATE\r\n\
D,UTILAKE_TEST,BYVOCAB,1,1,2026/08/03,2020/01/01\r\n\
D,UTILAKE_TEST,BYVOCAB,1,2,2026/08/01,2021/01/01\r\n\
I,UTILAKE_TEST,FIRSTDATED,1,A,WHEN,B\r\n\
D,UTILAKE_TEST,FIRSTDATED,1,x,2026/08/05,2026/08/09\r\n\
D,UTILAKE_TEST,FIRSTDATED,1,y,2026/08/04,2026/08/08\r\n\
D,UTILAKE_TEST,FIRSTDATED,1,z\r\n\
I,UTILAKE_TEST,UNDATED,1,A,B\r\n\
D,UTILAKE_TEST,UNDATED,1,1,2\r\n\
I,UTILAKE_TEST,LATE,1,A,WHEN\r\n\
D,UTILAKE_TEST,LATE,1,x,\r\n\
D,UTILAKE_TEST,LATE,1,y,2026/08/06\r\n\
D,UTILAKE_TEST,LATE,1,z,N/A\r\n\
D,UTILAKE_TEST,LATE,1,w,2026/08/02\r\n\
D,UTILAKE_TEST,LATE,1,v,\r\n\
X,not a row\r\n\
C,\"END OF REPORT\",15\r\n";
    let mut buf = Vec::new();
    let (blocks, stats) = read_blocks(&csv[..], &mut buf).unwrap();
    assert_eq!(blocks.len(), 5);
    assert_eq!(stats.bad_rows, 1, "X is no row");
    assert!(stats.footer_mismatch(), "the footer says 15 lines");
    assert_eq!(blocks[0].rows, 3);
    assert_eq!(
        blocks[0].bounds.len(),
        2,
        "SETTLEMENTDATE and LASTCHANGED are dated"
    );
    // Every column is profiled from its rows, quoted or not.
    let kinds: Vec<_> = blocks[0].profile.iter().map(|p| p.kind).collect();
    assert_eq!(
        kinds,
        [Kind::Timestamp, Kind::Text, Kind::Decimal, Kind::Timestamp]
    );
    assert_eq!(blocks[1].profile[1].kind, Kind::Date);
    assert_eq!(blocks[3].profile[0].nulls, 0);
    let ctls = Ctls::parse([(
        "PUBLIC_ARCHIVE#DISPATCH_UNIT_SCADA#FILE01#202608.ctl",
        SCADA_CTL,
    )]);
    let spans: Vec<Option<Span>> = blocks
        .into_iter()
        .map(|b| block_shape(&cat, &ctls, None, b).span)
        .collect();
    assert_eq!(
        spans[0],
        Some(Span {
            first: "2026/08/01 00:05:00".into(),
            last: "2026/08/31 23:55:00".into(),
            column: "SETTLEMENTDATE".into()
        }),
        "the declared time column, min and max over unordered rows"
    );
    assert_eq!(
        spans[1],
        Some(Span {
            first: "2026/08/01".into(),
            last: "2026/08/03".into(),
            column: "TRADINGDATE".into()
        }),
        "no declaration: TRADINGDATE outranks EFFECTIVEDATE in the vocabulary"
    );
    assert_eq!(
        spans[2],
        Some(Span {
            first: "2026/08/04".into(),
            last: "2026/08/05".into(),
            column: "WHEN".into()
        }),
        "no known name: the first dated column; a short row bounds nothing"
    );
    assert_eq!(spans[3], None);
    assert_eq!(
        spans[4],
        Some(Span {
            first: "2026/08/02".into(),
            last: "2026/08/06".into(),
            column: "WHEN".into()
        }),
        "empty in the first row, empty and N/A later: the dated values alone bound it"
    );
    assert!(date_like(b"2026/08/01") && !date_like(b"2026-08-01") && !date_like(b"1.5"));
}

#[test]
fn every_block_of_every_file_of_a_month_is_recorded_with_its_rows() {
    let dir = tempfile::tempdir().unwrap();
    let cat = scada();
    let month = YearMonth::new(2026, 8).unwrap();
    let mdir = dir.path().join("MMSDM_2026_08");
    let scada_rows = 110_000;
    let csv = three_tables(scada_rows);
    assert!(csv.len() > 4 << 20);
    let zip_name = "PUBLIC_ARCHIVE#DISPATCH_UNIT_SCADA#FILE01#202608010000.zip";
    let zip = zip_of(&[("A.CSV", &csv)]);
    put(&mdir, &format!("DATA/{zip_name}"), &zip);
    // A partial download and hidden files are not files of the month.
    put(
        &mdir,
        "DATA/PUBLIC_ARCHIVE#X#FILE01#202608010000.zip.part",
        b"x",
    );
    put(&mdir, "DATA/.DS_Store", b"x");
    put(&mdir, "._CTL/x.ctl", b"x");
    put(
        &mdir,
        "DATA/PUBLIC_ARCHIVE#BAD#FILE01#202608010000.zip",
        b"not a zip",
    );
    put(
        &mdir,
        "CTL/PUBLIC_ARCHIVE#DISPATCH_UNIT_SCADA#FILE01#202608.ctl",
        SCADA_CTL.as_bytes(),
    );
    // Neither data nor a control file.
    put(&mdir, "DATA/notes.pdf", b"%PDF");
    // The month's documentation is not market data.
    put(
        &mdir,
        "DOCUMENTATION/MMS Data Model/v5.6/MMSDM_create_v5.6.zip",
        &zip_of(&[("PDR.CSV", &three_tables(1))]),
    );
    let files = month_files(&mdir).unwrap();
    assert_eq!(files.len(), 5, "{files:?}");
    let tmp = tempfile::tempdir().unwrap();
    let census = census_month(&cat, month, &files, tmp.path(), 3);
    assert_eq!(census.problems.len(), 1, "{:?}", census.problems);
    assert!(census.problems[0].error.contains("could not be opened"));
    assert!(!census.files.contains_key("MMSDM_create_v5.6.zip"));
    assert!(
        census.files["PUBLIC_ARCHIVE#BAD#FILE01#202608010000.zip"]
            .blocks
            .is_empty(),
        "{:?}",
        census.files.keys().collect::<Vec<_>>()
    );
    let file = &census.files[zip_name];
    assert_eq!(
        (file.source.as_str(), file.kind.as_str()),
        (zip_name, "archive")
    );
    assert_eq!(file.sha256, crate::ctl::sha256(&zip), "of the file read");
    assert_eq!(file.size, zip.len() as u64);
    let blocks = &file.blocks;
    assert_eq!(blocks.len(), 3);
    assert_eq!(
        blocks.iter().map(|b| b.rows).collect::<Vec<_>>(),
        [scada_rows as u64, 2, 1]
    );
    assert_eq!(
        blocks[0].span,
        Some(Span {
            first: "2026/08/01 00:05:00".into(),
            last: "2026/08/01 00:05:00".into(),
            column: "SETTLEMENTDATE".into()
        })
    );
    assert!(
        blocks[1].span.is_none() && blocks[2].span.is_none(),
        "no dated column"
    );
    assert_eq!(
        blocks[0].shape.columns,
        ["SETTLEMENTDATE", "DUID", "SCADAVALUE"]
    );
    assert_eq!(
        blocks[0].ctl,
        [TypeClass::Timestamp, TypeClass::Varchar, TypeClass::Number]
    );
    assert_eq!(blocks[0].profile[2].kind, Kind::Decimal);
    assert_eq!(blocks[1].shape.report_subtype, "EXTRA");
    assert_eq!(blocks[2].shape.report_subtype, "LAST");
    assert!(blocks[2].ctl.iter().all(|c| *c == TypeClass::Unknown));

    // Recorded: every block with its rows and profile, read back as it
    // was.
    let root = dir.path().join("repo");
    assert_eq!(read_month(&root, month).unwrap(), None);
    assert_eq!(record(&root, &census).unwrap(), 3);
    let path = census_path(&root, month);
    assert!(path.ends_with("evidence/census/2026-08.toml"));
    let recorded = read_month(&root, month).unwrap().unwrap();
    assert_eq!(recorded[zip_name], FileRecord::of(file));
    assert_eq!(recorded[zip_name].ids().len(), 3);
    let b = &recorded[zip_name].block[0];
    assert_eq!(b.profile_of(2), blocks[0].profile[2]);
    assert_eq!(b.ctl_of(0), TypeClass::Timestamp);
    assert_eq!(b.ctl_of(9), TypeClass::Unknown);
    assert_eq!(b.profile_of(9), ColumnProfile::default());
    // The same census again is the same bytes.
    let before = std::fs::read(&path).unwrap();
    record(&root, &census).unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), before);
    let text = String::from_utf8(before).unwrap();
    assert!(
        text.starts_with(
            "# Generated by `utilake-schemas census`: every block of every file of 2026-08"
        ),
        "{text}"
    );
    assert!(!text.contains("published"), "{text}");

    // A census file that is not one, or cannot be read, or written.
    std::fs::write(&path, "x = [").unwrap();
    let e = read_month(&root, month).unwrap_err().to_string();
    assert!(e.contains("2026-08.toml cannot be read"), "{e}");
    std::fs::remove_file(&path).unwrap();
    std::fs::create_dir(&path).unwrap();
    assert!(read_month(&root, month).is_err());
    let e = record(&root, &census).unwrap_err().to_string();
    assert!(e.contains("2026-08.toml could not be written"), "{e}");

    // An empty directory says to download the month; a missing one cannot
    // be read.
    std::fs::create_dir(dir.path().join("empty")).unwrap();
    let e = month_files(&dir.path().join("empty")).unwrap_err();
    assert!(e.to_string().contains("utilake-schemas fetch"), "{e}");
    let e = month_files(&dir.path().join("none")).unwrap_err();
    assert!(matches!(e, CensusError::Read { .. }), "{e}");
}

#[test]
fn a_file_that_cannot_be_read_is_a_problem_of_its_own() {
    use std::os::unix::fs::PermissionsExt as _;
    let dir = tempfile::tempdir().unwrap();
    let cat = scada();
    let month = YearMonth::new(2026, 8).unwrap();
    put(dir.path(), "DATA/PUBLIC_BAD_202608.CSV", b"D,A,B,1,x\n");
    put(
        dir.path(),
        "DATA/PUBLIC_BADENTRY_202608.zip",
        &zip_of(&[
            ("GOOD.CSV", &three_tables(1)),
            ("BAD.CSV", b"D,A,B,1,x\n"),
            ("GOOD2.CSV", &three_tables(1)),
        ]),
    );
    put(
        dir.path(),
        "DATA/PUBLIC_BADINNER_202608.zip",
        &zip_of(&[("inner.zip", b"not a zip"), ("GOOD.CSV", &three_tables(1))]),
    );
    put(dir.path(), "DATA/LOCKED.CSV", &three_tables(1));
    put(dir.path(), "CTL/LOCKED.ctl", SCADA_CTL.as_bytes());
    put(
        dir.path(),
        "CTL/CTLS_ONLY.zip",
        &zip_of(&[("README.txt", b"x")]),
    );
    for f in ["DATA/LOCKED.CSV", "CTL/LOCKED.ctl"] {
        let p = dir.path().join(f);
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o000)).unwrap();
    }
    let files = month_files(dir.path()).unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let census = census_month(&cat, month, &files, tmp.path(), 3);
    let files: Vec<&str> = census.problems.iter().map(|p| base(&p.file)).collect();
    assert_eq!(
        files,
        [
            "LOCKED.CSV",
            "BAD.CSV",
            "PUBLIC_BADINNER_202608.zip",
            "PUBLIC_BAD_202608.CSV"
        ],
        "{:?}",
        census.problems
    );
    assert!(
        census.problems[1]
            .file
            .ends_with("PUBLIC_BADENTRY_202608.zip!BAD.CSV")
    );
    assert!(
        census.problems[2].error.contains("inner.zip"),
        "{:?}",
        census.problems[2]
    );
    assert!(
        census.problems[0]
            .error
            .starts_with("The file cannot be read: "),
        "{:?}",
        census.problems[0]
    );
    assert!(census.problems[3].error.contains("before any I row"));
    let names: Vec<&str> = census.files.keys().map(String::as_str).collect();
    assert_eq!(
        names,
        [
            "PUBLIC_BADENTRY_202608.zip",
            "PUBLIC_BADINNER_202608.zip",
            "PUBLIC_BAD_202608.CSV"
        ],
        "an unopened file is not in the census; a zip of no CSV has no blocks to record"
    );
    assert_eq!(
        census.files["PUBLIC_BADENTRY_202608.zip"].blocks.len(),
        6,
        "the entries before and after the bad one"
    );
    assert_eq!(
        census.files["PUBLIC_BADINNER_202608.zip"].blocks.len(),
        3,
        "the entry beside the zip that is not one"
    );
}

#[test]
fn a_whole_month_zip_is_read_through_its_nested_zips_and_control_files() {
    let dir = tempfile::tempdir().unwrap();
    let cat = scada();
    let month = YearMonth::new(2012, 1).unwrap();
    let inner = zip_of(&[(
        "PUBLIC_DVD_DISPATCH_UNIT_SCADA_201201010000.CSV",
        &three_tables(3),
    )]);
    let ctl = SCADA_CTL.replace(
        "PUBLIC_ARCHIVE#DISPATCH_UNIT_SCADA#FILE01#202608010000.CSV",
        "PUBLIC_DVD_DISPATCH_UNIT_SCADA_201201010000.CSV",
    );
    let prefix = "MMSDM_2012_01/MMSDM_Historical_Data_SQLLoader";
    let whole = zip_of(&[
        (
            &format!("{prefix}/DOCUMENTATION/README.txt"),
            b"Static/Standing data tables".as_slice(),
        ),
        (
            &format!("{prefix}/CTL/PUBLIC_DVD_DISPATCH_UNIT_SCADA_201201.ctl"),
            ctl.as_bytes(),
        ),
        (
            &format!("{prefix}/DATA/PUBLIC_DVD_DISPATCH_UNIT_SCADA_201201010000.zip"),
            &inner,
        ),
    ]);
    put(dir.path(), "MMSDM_2012_01.zip", &whole);
    let files = month_files(dir.path()).unwrap();
    assert_eq!(files.len(), 1);
    let tmp = tempfile::tempdir().unwrap();
    let census = census_month(&cat, month, &files, tmp.path(), 3);
    assert!(census.problems.is_empty(), "{:?}", census.problems);
    let file = &census.files["PUBLIC_DVD_DISPATCH_UNIT_SCADA_201201010000.zip"];
    assert_eq!(
        (file.source.as_str(), file.kind.as_str()),
        ("MMSDM_2012_01.zip", "dvd")
    );
    assert_eq!(
        (file.sha256.clone(), file.size),
        (crate::ctl::sha256(&whole), whole.len() as u64),
        "of the whole-month zip it was read out of"
    );
    let blocks = &file.blocks;
    assert_eq!(blocks.len(), 3);
    assert_eq!(blocks[0].rows, 3);
    assert_eq!(blocks[0].ctl[0], TypeClass::Timestamp);
}

#[test]
fn a_month_both_a_directory_and_a_zip_is_counted_once() {
    let dir = tempfile::tempdir().unwrap();
    let cat = scada();
    let month = YearMonth::new(2020, 2).unwrap();
    let same = "PUBLIC_DVD_DISPATCH_UNIT_SCADA_202002010000.zip";
    let other = "PUBLIC_DVD_P5MIN_X_202002010000.zip";
    let scada_zip = zip_of(&[("A.CSV", &three_tables(3))]);
    put(dir.path(), &format!("DATA/{same}"), &scada_zip);
    put(
        dir.path(),
        &format!("DATA/{other}"),
        &zip_of(&[("B.CSV", &three_tables(2))]),
    );
    // The whole-month zip names its entries with backslashes; one of its
    // copies differs from the directory's.
    let prefix = r"MMSDM_2020_02\MMSDM_Historical_Data_SQLLoader\DATA";
    let whole = zip_of(&[
        (&format!(r"{prefix}\{same}"), scada_zip.as_slice()),
        (
            &format!(r"{prefix}\{other}"),
            &zip_of(&[("B.CSV", &three_tables(5))]),
        ),
    ]);
    put(dir.path(), "MMSDM_2020_02.zip", &whole);
    let files = month_files(dir.path()).unwrap();
    assert_eq!(files.len(), 3, "{files:?}");
    let tmp = tempfile::tempdir().unwrap();
    let census = census_month(&cat, month, &files, tmp.path(), 3);
    assert!(census.problems.is_empty(), "{:?}", census.problems);
    let names: Vec<&str> = census.files.keys().map(String::as_str).collect();
    assert_eq!(
        names,
        [&format!("MMSDM_2020_02.zip!{other}"), same, other],
        "the same copy once, a differing copy under its source"
    );
    assert_eq!(census.files[same].blocks.len(), 3);
    assert_eq!(census.files[other].blocks[0].rows, 2);
    let copy = &census.files[&format!("MMSDM_2020_02.zip!{other}")];
    assert_eq!(
        (copy.blocks[0].rows, copy.source.as_str()),
        (5, "MMSDM_2020_02.zip")
    );
    assert_eq!(copy.sha256, crate::ctl::sha256(&whole));
}

#[test]
fn names_tell_kinds_and_tables() {
    assert_eq!(
        file_kind("PUBLIC_ARCHIVE#DISPATCHLOAD#FILE01#202608010000.zip"),
        "archive"
    );
    assert_eq!(
        file_kind("PUBLIC_ARCHIVE#PREDISPATCHPRICE#ALL#FILE01#202608010000.zip"),
        "archive#all"
    );
    assert_eq!(
        file_kind("PUBLIC_DVD_P5MIN_REGIONSOLUTION_ALL_201901010000.zip"),
        "dvd#all"
    );
    assert_eq!(file_kind("MMSDM_2020_02.zip"), "other");
    assert!(ctl_table("readme.txt").is_none());
    assert!(ctl_serves("PREDISPATCHPRICE_D", "PREDISPATCHPRICE", true));
    assert!(!ctl_serves("REGIONAPC", "REGION", true));
    assert_eq!(table_keys("BIDPEROFFER1"), ["BIDPEROFFER", "BIDPEROFFER1"]);
    assert_eq!(table_keys("2024"), ["2024"]);
    assert_eq!(file_key("A.zip", "B.CSV"), "A.zip");
    assert_eq!(file_key("A.zip", r"x\B.zip!B.CSV"), "B.zip");
}

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The census of the heads of 2026-08 gives every file and block the
/// shape, report, version, time column and declared classes utilake's
/// census of the whole files recorded (`utilake-census.toml`, an excerpt
/// of it): the same tables, the same control files, the same answer.
#[test]
fn the_heads_of_a_month_have_the_shapes_utilake_recorded() {
    #[derive(Deserialize)]
    struct Expected {
        kind: String,
        block: Vec<ExpectedBlock>,
    }
    #[derive(Deserialize, PartialEq, Debug)]
    struct ExpectedBlock {
        shape: ShapeId,
        report: (String, String),
        version: u16,
        time: Option<String>,
        columns: Vec<String>,
        ctl: String,
    }
    let cat = Catalog::read(&repo().join("tables")).unwrap();
    let dir = repo().join("testdata/bootstrap/MMSDM_2026_08");
    let text = std::fs::read_to_string(dir.join("utilake-census.toml")).unwrap();
    let expected: BTreeMap<String, Expected> = toml::from_str(&text).unwrap();
    let files: Vec<PathBuf> = month_files(&dir)
        .unwrap()
        .into_iter()
        .filter(|p| !p.ends_with("utilake-census.toml"))
        .collect();
    let tmp = tempfile::tempdir().unwrap();
    let census = census_month(
        &cat,
        YearMonth::new(2026, 8).unwrap(),
        &files,
        tmp.path(),
        3,
    );
    assert!(census.problems.is_empty(), "{:?}", census.problems);
    assert_eq!(census.files.len(), expected.len());
    for (name, want) in &expected {
        let csv = name.replace(".zip", ".CSV");
        let got = FileRecord::of(&census.files[&csv]);
        assert_eq!(got.kind, want.kind, "{name}");
        let blocks: Vec<ExpectedBlock> = got
            .block
            .into_iter()
            .map(|b| ExpectedBlock {
                shape: b.shape,
                report: b.report,
                version: b.version,
                time: b.time,
                columns: b.columns,
                ctl: b.ctl,
            })
            .collect();
        assert_eq!(blocks, want.block, "{name}");
    }
}

/// The census of the heads of 2024-01, by utilake's rules: a split file
/// typed by its own control file, a big-table file by its table's, a
/// split file of a `_D` table by the daily control file.
#[test]
fn the_heads_of_an_old_month_are_typed_by_split_and_daily_control_files() {
    let cat = Catalog::read(&repo().join("tables")).unwrap();
    let dir = repo().join("testdata/bootstrap/MMSDM_2024_01");
    let files = month_files(&dir).unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let census = census_month(
        &cat,
        YearMonth::new(2024, 1).unwrap(),
        &files,
        tmp.path(),
        3,
    );
    assert!(census.problems.is_empty(), "{:?}", census.problems);
    let rec =
        |stem: &str| FileRecord::of(&census.files[&format!("PUBLIC_DVD_{stem}_202401010000.CSV")]);
    let scada = rec("DISPATCH_UNIT_SCADA");
    assert_eq!(scada.kind, "dvd");
    let b = &scada.block[0];
    assert_eq!(b.report, ("DISPATCH".into(), "UNIT_SCADA".into()));
    assert_eq!(b.columns, ["SETTLEMENTDATE", "DUID", "SCADAVALUE"]);
    assert_eq!((b.ctl.as_str(), b.kinds.as_str()), ("tvn", "tsd"));
    assert_eq!(b.rows, 20);
    assert_eq!(b.time.as_deref(), Some("SETTLEMENTDATE"));
    assert_eq!(b.first.as_deref(), Some("2024/01/01 00:05:00"));
    assert_eq!(scada.sha256.len(), 64);
    assert_eq!(rec("P5MIN_CONSTRAINTSOLUTION1_ALL").kind, "dvd#all");
    // The control file loads CONFIDENTIAL_TO as a FILLER: declared as
    // nothing. The big-table file is typed by the same control file.
    for stem in ["P5MIN_CONSTRAINTSOLUTION1", "P5MIN_CONSTRAINTSOLUTION1_ALL"] {
        assert_eq!(rec(stem).block[0].ctl, "tntvnnnt?tnn", "{stem}");
    }
    for stem in [
        "BIDPEROFFER1",
        "BIDPEROFFER2",
        "PREDISPATCHLOAD1",
        "DISPATCHPRICE",
    ] {
        let r = rec(stem);
        assert!(
            r.block.iter().all(|b| !b.ctl.contains('?')),
            "{stem}: every column declared: {:?}",
            r.block
        );
    }
}

#[test]
fn the_census_is_the_same_on_any_number_of_threads() {
    let cat = scada();
    let dir = tempfile::tempdir().unwrap();
    let files: Vec<PathBuf> = (0..12)
        .map(|i| {
            let name = format!("PUBLIC_DVD_DISPATCH_UNIT_SCADA_2024010{i:02}0000.CSV");
            let p = dir.path().join(&name);
            put(dir.path(), &name, format!("C,X\nI,DISPATCH,UNIT_SCADA,1,SETTLEMENTDATE,DUID,SCADAVALUE\nD,DISPATCH,UNIT_SCADA,1,\"2024/01/01 00:05:00\",U{i},1.5\nC,END\n").as_bytes());
            p
        })
        .collect();
    let month = YearMonth::new(2024, 1).unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let one = census_month(&cat, month, &files, tmp.path(), 1);
    for jobs in [2, 5, 12] {
        assert_eq!(
            census_month(&cat, month, &files, tmp.path(), jobs),
            one,
            "{jobs}"
        );
    }
    assert_eq!(one.files.len(), 12);
}

#[test]
fn a_file_that_opens_but_cannot_be_read_is_a_problem() {
    // A directory opens as a file and fails on the first read.
    let dir = tempfile::tempdir().unwrap();
    let odd = dir
        .path()
        .join("PUBLIC_DVD_DISPATCH_UNIT_SCADA_202401010000.zip");
    std::fs::create_dir(&odd).unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let c = census_month(
        &scada(),
        YearMonth::new(2024, 1).unwrap(),
        &[odd],
        tmp.path(),
        1,
    );
    assert_eq!(c.problems.len(), 1);
    assert!(
        c.problems[0].error.starts_with("The file cannot be read"),
        "{:?}",
        c.problems
    );
}
