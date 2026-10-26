use std::io::Write as _;

use super::*;

const PG: &str = "/* Data Model: AEMO Electricity Data Model v5.6.0.0 postgreSQL*/\n/* Created on: 2025/01/02 03:04:05 */\n\
create table REGION (REGIONID VARCHAR(10) not null, DESCRIPTION VARCHAR(64) null);\n\
comment on table REGION is 'The regions';\ncomment on column REGION.REGIONID is 'Region \"id\"';\n\
alter table REGION add constraint R_PK primary key (REGIONID);\n\
create table DISPATCHPRICE (SETTLEMENTDATE TIMESTAMP(0) not null, REGIONID VARCHAR(10) not null, RRP NUMERIC(15,5) null);\n\
alter table DISPATCHPRICE add constraint D_PK primary key (SETTLEMENTDATE, REGIONID);\n";

/// A zip of `body` as the postgreSQL script of `version`, beside an Oracle
/// script that is never read.
fn zip_with(dir: &Path, name: &str, version: &str, body: &[u8]) -> PathBuf {
    let path = dir.join(name);
    let mut w = ::zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
    let opts = ::zip::write::SimpleFileOptions::default();
    w.start_file(
        format!("postgreSQL/{version}/create_mms_data_model.sql"),
        opts,
    )
    .unwrap();
    w.write_all(body).unwrap();
    w.start_file(format!("Oracle/{version}/create_mms_data_model.sql"), opts)
        .unwrap();
    w.write_all(b"create table X (A NUMBER(1) null) tablespace T;")
        .unwrap();
    w.finish().unwrap();
    path
}

fn month(m: &str) -> YearMonth {
    m.parse().unwrap()
}

#[test]
fn a_month_records_its_versions_and_a_second_month_widens_them() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("repo");
    let z56 = zip_with(dir.path(), "MMSDM_create_v5.6.zip", "v5.6", PG.as_bytes());
    let pg57 = PG.replace("v5.6.0.0", "v5.7.0.0").replace(
        "RRP NUMERIC(15,5) null",
        "RRP NUMERIC(15,5) null, ROP NUMERIC(15,5) null",
    );
    let z57 = zip_with(dir.path(), "MMSDM_create_v5.7.zip", "v5.7", pg57.as_bytes());
    let fork = dir.path().join("._MMSDM_create_v5.6.zip");
    std::fs::write(&fork, b"not a zip").unwrap();
    let other = dir.path().join("PUBLIC_DVD_X_202508010000.zip");
    std::fs::write(&other, b"").unwrap();
    let files = vec![other, z57.clone(), fork, z56.clone()];
    let zips = model_zips(&files);
    assert_eq!(
        zips,
        [z56.as_path(), z57.as_path()],
        "sorted, forks and data files left out"
    );

    let oct = month("2025-10");
    let r = record(&root, Market::Nem, oct, &zips).unwrap();
    assert_eq!(r.new_versions, ["v5.6", "v5.7"]);
    assert_eq!(r.versions, ["v5.6", "v5.7"]);
    assert!(r.problems.is_empty(), "{:?}", r.problems);
    let text = std::fs::read_to_string(model_path(&root, Market::Nem, "v5.6")).unwrap();
    assert!(text.starts_with(MODEL_HEADER), "{text}");
    assert!(text.contains("[tables.DISPATCHPRICE]\nprimary_key = [\"SETTLEMENTDATE\", \"REGIONID\"]\ncolumns = [\n    [\"SETTLEMENTDATE\", \"TIMESTAMP(0)\", false, \"\"],"), "{text}");
    assert!(text.contains("comment = \"The regions\""), "{text}");
    assert!(
        text.contains("[\"REGIONID\", \"VARCHAR(10)\", false, \"Region \\\"id\\\"\"]"),
        "{text}"
    );
    let m = read_model(&root, Market::Nem, "v5.6").unwrap().unwrap();
    assert_eq!((m.first_seen, m.last_seen), (oct, oct));
    assert_eq!(m.tables["REGION"].primary_key, ["REGIONID"]);
    assert_eq!(render(&m), text, "the file reads back to itself");

    let nov = month("2025-11");
    let r = record(&root, Market::Nem, nov, &[z57.as_path()]).unwrap();
    assert!(r.new_versions.is_empty());
    assert_eq!(r.versions, ["v5.7"]);
    let m = read_model(&root, Market::Nem, "v5.7").unwrap().unwrap();
    assert_eq!((m.first_seen, m.last_seen), (oct, nov));
    let months = read_months(&root, Market::Nem).unwrap();
    assert_eq!(months[&oct], ["v5.6", "v5.7"]);
    assert_eq!(months[&nov], ["v5.7"]);
    let text = std::fs::read_to_string(months_path(&root, Market::Nem)).unwrap();
    assert_eq!(
        text,
        format!(
            "{MONTHS_HEADER}[months]\n\"2025-10\" = [\"v5.6\", \"v5.7\"]\n\"2025-11\" = [\"v5.7\"]\n"
        )
    );
    let all = read_models(&root, Market::Nem).unwrap();
    assert_eq!(all.keys().collect::<Vec<_>>(), ["v5.6", "v5.7"]);
    let order: Vec<&str> = by_first_seen(&all)
        .iter()
        .map(|m| m.version.as_str())
        .collect();
    assert_eq!(order, ["v5.6", "v5.7"]);
    assert!(
        read_months(&dir.path().join("nowhere"), Market::Nem)
            .unwrap()
            .is_empty()
    );
    assert!(
        read_models(&dir.path().join("nowhere"), Market::Nem)
            .unwrap()
            .is_empty()
    );

    // The same version with a different script is reported and not rewritten.
    let changed = zip_with(
        dir.path(),
        "MMSDM_create_v5.6.zip",
        "v5.6",
        PG.replace("DESCRIPTION VARCHAR(64)", "DESCRIPTION VARCHAR(99)")
            .as_bytes(),
    );
    let dec = month("2025-12");
    let r = record(&root, Market::Nem, dec, &[changed.as_path()]).unwrap();
    assert_eq!(r.problems.len(), 1);
    assert!(r.problems[0].contains("differs from the one recorded from 2025-10"));
    let m = read_model(&root, Market::Nem, "v5.6").unwrap().unwrap();
    assert_eq!(
        m.tables["REGION"].columns[1].1, "VARCHAR(64)",
        "the recorded one stays"
    );
    assert_eq!(m.last_seen, dec, "but the months widen");
    // Recorded again for an earlier month, its first month moves back.
    record(&root, Market::Nem, month("2025-09"), &[changed.as_path()]).unwrap();
    let m = read_model(&root, Market::Nem, "v5.6").unwrap().unwrap();
    assert_eq!(m.first_seen, month("2025-09"));

    // A month with no zip changes nothing; files that are not models are
    // passed over.
    std::fs::write(models_dir(&root, Market::Nem).join("notes.txt"), "x").unwrap();
    std::fs::create_dir_all(models_dir(&root, Market::Nem).join(".toml")).unwrap();
    let r = record(&root, Market::Nem, month("2026-01"), &[]).unwrap();
    assert!(r.versions.is_empty() && r.new_versions.is_empty());
    assert_eq!(read_models(&root, Market::Nem).unwrap().len(), 2);
}

/// A cut of AEMO's 5.6 create script (two tables, as the 2025-10 archive
/// carries them, bytes untouched) gives the same file utilake recorded
/// from the whole script, cut to the same two tables: every byte but the
/// first line, which names the tool, and the script's SHA-256, which is
/// the cut's.
#[test]
fn the_script_gives_the_bytes_utilake_recorded() {
    let testdata = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../testdata/models");
    let sql = std::fs::read(testdata.join("create_mms_data_model_v5.6_excerpt.sql")).unwrap();
    let recorded = std::fs::read_to_string(testdata.join("utilake_v5.6_excerpt.toml")).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let zip = zip_with(dir.path(), "MMSDM_create_v5.6.zip", "5.6", &sql);
    let root = dir.path().join("repo");
    for m in ["2026-09", "2025-10", "2026-01"] {
        assert!(
            record(&root, Market::Nem, month(m), &[zip.as_path()])
                .unwrap()
                .problems
                .is_empty()
        );
    }
    let ours = std::fs::read_to_string(model_path(&root, Market::Nem, "5.6")).unwrap();
    let whole = "b1f932b1f43ea32385809b204a7a47cf2df5c0a09bb7c469eb2204ffc56fe1f6";
    let (first, rest) = recorded.split_once('\n').unwrap();
    assert_eq!(
        first,
        "# Generated by `utilake-nem-census models`: AEMO's data model as its create script declares it."
    );
    let want = format!("{MODEL_HEADER}{rest}").replace(whole, &sha256(&sql));
    assert_eq!(ours, want);
    // And utilake's bytes read back to themselves.
    let m: Model = toml::from_str(&recorded).unwrap();
    assert_eq!(render(&m), format!("{MODEL_HEADER}{rest}"));
}

#[test]
fn backslash_entry_names_read_too() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("MMSDM_create_v5.1.zip");
    let mut w = ::zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
    w.start_file(
        "postgreSQL\\5.1\\create_mms_data_model.sql",
        ::zip::write::SimpleFileOptions::default(),
    )
    .unwrap();
    w.write_all(PG.as_bytes()).unwrap();
    w.finish().unwrap();
    let (version, _, script) = read_zip(&path).unwrap();
    assert_eq!(version, "5.1");
    assert_eq!(script.tables.len(), 2);
}

#[test]
fn broken_zips_and_files_are_problems_naming_them() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("repo");
    let bad = dir.path().join("MMSDM_create_v9.9.zip");
    std::fs::write(&bad, b"not a zip").unwrap();
    let r = record(&root, Market::Nem, month("2025-10"), &[bad.as_path()]).unwrap();
    assert!(
        r.problems[0].contains("The data-model zip") && r.problems[0].contains("v9.9.zip"),
        "{:?}",
        r.problems
    );
    let no_pg = dir.path().join("MMSDM_create_v9.8.zip");
    let mut w = ::zip::ZipWriter::new(std::fs::File::create(&no_pg).unwrap());
    let opts = ::zip::write::SimpleFileOptions::default();
    w.start_file("Oracle/v9.8/create_mms_data_model.sql", opts)
        .unwrap();
    w.start_file("postgreSQL/v9.8/drop_mms_data_model.sql", opts)
        .unwrap();
    w.start_file("postgreSQL/README", opts).unwrap();
    w.finish().unwrap();
    let e = read_zip(&no_pg).unwrap_err().to_string();
    assert!(e.contains("no postgreSQL"), "{e}");
    let bad_sql = zip_with(
        dir.path(),
        "MMSDM_create_v9.7.zip",
        "v9.7",
        b"drop table X;",
    );
    let e = read_zip(&bad_sql).unwrap_err().to_string();
    assert!(e.contains("not a statement"), "{e}");
    assert!(matches!(
        read_zip(Path::new("/nonexistent.zip")).unwrap_err(),
        ModelError::Zip { .. }
    ));
    // An entry whose compressed bytes are not deflate's.
    let stored = dir.path().join("MMSDM_create_v9.6.zip");
    let mut w = ::zip::ZipWriter::new(std::fs::File::create(&stored).unwrap());
    let entry = "postgreSQL/v9.6/create_mms_data_model.sql";
    w.start_file(entry, opts).unwrap();
    w.write_all(&b"create table T (A VARCHAR(1) null);".repeat(50))
        .unwrap();
    w.finish().unwrap();
    let mut bytes = std::fs::read(&stored).unwrap();
    // The data follows the 30-byte local header and the entry's name.
    bytes[30 + entry.len()] = 0xff;
    std::fs::write(&stored, bytes).unwrap();
    let e = read_zip(&stored).unwrap_err().to_string();
    assert!(e.contains("v9.6.zip cannot be read: postgreSQL/"), "{e}");

    // A model file that is not TOML.
    std::fs::create_dir_all(models_dir(&root, Market::Nem)).unwrap();
    std::fs::write(model_path(&root, Market::Nem, "v1.0"), "version = ").unwrap();
    let e = read_model(&root, Market::Nem, "v1.0")
        .unwrap_err()
        .to_string();
    assert!(
        e.starts_with("The model file ") && e.contains("v1.0.toml"),
        "{e}"
    );
    assert!(read_models(&root, Market::Nem).is_err());
    std::fs::write(
        months_path(&root, Market::Nem),
        "[months]\n\"2025-13\" = [\"v1.0\"]\n",
    )
    .unwrap();
    let e = read_months(&root, Market::Nem).unwrap_err().to_string();
    assert!(e.contains("is not a month"), "{e}");
    std::fs::write(months_path(&root, Market::Nem), "[months").unwrap();
    assert!(read_months(&root, Market::Nem).is_err());
    // The models directory a file.
    std::fs::remove_dir_all(models_dir(&root, Market::Nem)).unwrap();
    std::fs::write(models_dir(&root, Market::Nem), "").unwrap();
    assert!(read_models(&root, Market::Nem).is_err());
    assert!(read_model(&root, Market::Nem, "v1").is_err());
    let good = zip_with(dir.path(), "MMSDM_create_v5.6.zip", "v5.6", PG.as_bytes());
    let e = record(&root, Market::Nem, month("2025-10"), &[good.as_path()]).unwrap_err();
    assert!(matches!(e, ModelError::Read { .. }), "{e}");
    // A file that cannot be written.
    let e = write_text(dir.path(), "x").unwrap_err().to_string();
    assert!(e.ends_with("could not be written."), "{e}");
}

#[test]
fn model_types_read_as_the_script_writes_them() {
    // AEMO's report writes Oracle's types, with a space before the size.
    assert_eq!(ModelType::parse("NUMBER(18,3)"), ModelType::Numeric(18, 3));
    assert_eq!(ModelType::parse("NUMBER(10)"), ModelType::Numeric(10, 0));
    assert_eq!(ModelType::parse("VARCHAR2 (20)"), ModelType::Varchar(20));
    assert_eq!(ModelType::parse("Varchar2 (100)"), ModelType::Varchar(100));
    assert_eq!(ModelType::parse("Date"), ModelType::Timestamp);
    assert_eq!(ModelType::parse("varchar(20)"), ModelType::Varchar(20));
    assert_eq!(ModelType::parse(" CHAR(1) "), ModelType::Varchar(1));
    assert_eq!(ModelType::parse("VARCHAR(MAX)"), ModelType::Other);
    assert_eq!(ModelType::parse("NUMERIC(15,5)"), ModelType::Numeric(15, 5));
    assert_eq!(ModelType::parse("DECIMAL(3)"), ModelType::Numeric(3, 0));
    assert_eq!(ModelType::parse("NUMERIC(x,1)"), ModelType::Other);
    assert_eq!(ModelType::parse("NUMERIC(3,y)"), ModelType::Other);
    assert_eq!(ModelType::parse("TIMESTAMP(0)"), ModelType::Timestamp);
    assert_eq!(ModelType::parse("date"), ModelType::Timestamp);
    assert_eq!(ModelType::parse("BLOB"), ModelType::Other);
    assert_eq!(q("a\tb\r\u{1}\\\n"), "\"a\\tb\\r\\u0001\\\\\\n\"");
}

const LOADER: &str = "C,PDR_CONFIG,5.7\r\n\
I,PDR_REPORT,FILE,1,REPORT_TYPE,TRANSACTION_TYPE_OVERRIDE\r\n\
D,PDR_REPORT,FILE,1,DEMAND,\r\n\
I,PDR_REPORT,TABLE,1,REPORT_NAME,REPORT_SUB_TYPE,VERSION,DESTINATION_TABLE,TRANSACTION_TYPE,ROW_FILTER_TYPE,IS_ACTIVE,IS_NEW\r\n\
D,PDR_REPORT,TABLE,1,ROOFTOP,ACTUAL,2,ROOFTOP_PV_ACTUAL,INSERT,,1,0\r\n\
D,PDR_REPORT,TABLE,1,OPERATIONAL_DEMAND,ACTUAL,3,DEMANDOPERATIONALACTUAL,INSERT-UPDATE,LASTCHANGED,1,0\r\n\
D,PDR_REPORT,TABLE,1,OPERATIONAL_DEMAND,ACTUAL,x,BAD,INSERT,,1,0\r\n";

#[test]
fn a_zips_loader_configuration_is_recorded_beside_its_model() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("MMSDM_create_v5.7.zip");
    let mut w = ::zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
    let opts = ::zip::write::SimpleFileOptions::default();
    w.start_file("postgreSQL/5.7/create_mms_data_model.sql", opts)
        .unwrap();
    w.write_all(PG.as_bytes()).unwrap();
    w.start_file("Config/5.7/PUBLIC_PDR_CONFIG_5.7_FULL.CSV", opts)
        .unwrap();
    w.write_all(LOADER.as_bytes()).unwrap();
    w.finish().unwrap();
    let l = read_loads(&path).unwrap().unwrap();
    assert_eq!(l.version, "5.7");
    // Sorted, the row without a version left out.
    assert_eq!(l.loads.len(), 2);
    assert_eq!(l.loads[0].table, "DEMANDOPERATIONALACTUAL");
    assert_eq!(l.loads[0].row_filter, "LASTCHANGED");
    assert_eq!(
        l.loads[1].report,
        ["ROOFTOP".to_owned(), "ACTUAL".to_owned()]
    );
    let root = dir.path().join("repo");
    let r = record(&root, Market::Nem, month("2026-05"), &[path.as_path()]).unwrap();
    assert!(r.problems.is_empty(), "{:?}", r.problems);
    let all = read_all_loads(&root, Market::Nem).unwrap();
    assert_eq!(all["5.7"], l);
    // A zip with no configuration has none.
    let bare = zip_with(dir.path(), "MMSDM_create_v5.1.zip", "5.1", PG.as_bytes());
    assert_eq!(read_loads(&bare).unwrap(), None);
    assert!(
        read_all_loads(&dir.path().join("none"), Market::Nem)
            .unwrap()
            .is_empty()
    );
}
