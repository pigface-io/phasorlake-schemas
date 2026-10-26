use utilake_iceberg::compat::check;

use super::*;
use crate::catalog::tests::{decl, scada};
use crate::catalog::{Override, TableDecl};
use crate::census::{BlockRecord, census_path};
use crate::reports::nem_reports;
use crate::reports::{FolderCensus, Tally, record_folder};
use crate::shape::ShapeId;

/// A block of DISPATCH/UNIT_SCADA over August's first day, its SCADAVALUE
/// with `int` digits before the point and two after.
fn scada_block(month: &str, int: u8) -> BlockRecord {
    let day = format!("{}/{}/01", &month[..4], &month[5..]);
    BlockRecord {
        shape: ShapeId(7),
        report: ("DISPATCH".into(), "UNIT_SCADA".into()),
        version: 1,
        rows: 3,
        rows_repeated: 0,
        headers: Vec::new(),
        time: Some("SETTLEMENTDATE".into()),
        first: Some(format!("{day} 00:05:00")),
        last: Some(format!("{day} 00:15:00")),
        midnight: None,
        columns: vec!["SETTLEMENTDATE".into(), "DUID".into(), "SCADAVALUE".into()],
        ctl: "tvn".into(),
        kinds: "tsd".into(),
        nulls: vec![0, 0, 0],
        int_digits: vec![0, 0, int],
        scale: vec![0, 0, 2],
        sig_digits: vec![0, 0, int + 2],
        max_len: vec![19, 8, 8],
        min: vec![String::new(); 3],
        max: vec![String::new(); 3],
    }
}

/// Records the census of `month`: one DVD file of one SCADA block, and a
/// file of a pair no table declares.
fn census(root: &Path, month: &str, int: u8) {
    let mut other = scada_block(month, int);
    other.report = ("DISPATCH".into(), "OTHER".into());
    let file = |b: BlockRecord| FileRecord {
        source: "x.zip".into(),
        sha256: "ab".into(),
        size: 9,
        kind: "dvd".into(),
        bad_rows: 0,
        footer_mismatches: 0,
        block: vec![b],
    };
    let files = BTreeMap::from([
        (format!("SCADA_{month}.CSV"), file(scada_block(month, int))),
        (format!("OTHER_{month}.CSV"), file(other)),
    ]);
    let path = census_path(root, Market::Nem, month.parse().unwrap());
    crate::files::write_atomic(&path, toml::to_string(&files).unwrap().as_bytes()).unwrap();
}

/// Publishes `h` as the export does: as table metadata, undeclared.
fn publish(root: &Path, h: &History) {
    let p = utilake_iceberg::Published {
        market: utilake_iceberg::Market::Nem,
        history: h.clone(),
        declaration: None,
        description: None,
        docs: BTreeMap::new(),
        model_types: BTreeMap::new(),
        model: Vec::new(),
    };
    let t = utilake_iceberg::to_metadata(&p, "l", "u").unwrap();
    let text = serde_json::to_string(&t.metadata).unwrap();
    crate::files::write_atomic(
        &published_path(root, Market::Nem, &h.table),
        text.as_bytes(),
    )
    .unwrap();
}

/// A repository of the SCADA table, two months, a report folder at the
/// root of ARCHIVE and one below it, and a data-model release.
fn repo() -> (tempfile::TempDir, Catalog) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    census(root, "2026-07", 3);
    census(root, "2026-08", 3);
    std::fs::write(
        census_dir(root, Market::Nem).join("README.md"),
        "not a month",
    )
    .unwrap();
    let tally = |csv: &str| Tally {
        month: "2026-08".parse().unwrap(),
        csvs: 4,
        first_csv: format!("{csv}_1.CSV"),
        last_csv: format!("{csv}_4.CSV"),
        first_file: format!("{csv}_1.zip"),
        first_sha256: "a1".into(),
        last_file: String::new(),
        last_sha256: String::new(),
        block: BlockRecord {
            last: Some("2026/08/01 00:30:00".into()),
            midnight: None,
            ..scada_block("2026-08", 2)
        },
    };
    let folder = |csv: &str| FolderCensus {
        files: 1,
        csvs: 4,
        blocks: vec![tally(csv)],
        ..FolderCensus::default()
    };
    record_folder(root, Market::Nem, (nem_reports(), ""), &folder("ROOT")).unwrap();
    record_folder(
        root,
        Market::Nem,
        (nem_reports(), "Dispatch_SCADA/2026"),
        &folder("NESTED"),
    )
    .unwrap();
    let model = "version = \"5.7\"\nmodel = \"m\"\ncreated = \"c\"\nscript_sha256 = \"s\"\nfirst_seen = \"2026-05\"\nlast_seen = \"2026-09\"\n\n[tables.DISPATCH_UNIT_SCADA]\nprimary_key = [\"SETTLEMENTDATE\", \"DUID\"]\ncolumns = [\n    [\"SETTLEMENTDATE\", \"TIMESTAMP(0)\", false, \"\"],\n    [\"scadavalue\", \"NUMERIC(16,6)\", true, \"\"],\n]\n";
    crate::files::write_atomic(
        &crate::models::model_path(root, Market::Nem, "5.7"),
        model.as_bytes(),
    )
    .unwrap();
    (dir, scada())
}

#[test]
fn every_census_folds_into_one_file_per_table() {
    let (dir, cat) = repo();
    let root = dir.path();
    // A history of a table no census holds, and a file that is no history.
    crate::files::write_atomic(&history_path(root, Market::Nem, "GONE"), b"{}").unwrap();
    std::fs::write(histories_dir(root, Market::Nem).join("notes.txt"), "kept").unwrap();

    let would = fold(root, Market::Nem, &cat, true, None).unwrap();
    assert_eq!(
        would,
        Folded {
            tables: 2,
            schemas: 2,
            runs: 2,
            breaks: 0,
            changed: 3,
            notes: Vec::new(),
        }
    );
    assert!(
        !history_path(root, Market::Nem, "DISPATCH_UNIT_SCADA").exists(),
        "a check writes nothing"
    );
    assert_eq!(fold(root, Market::Nem, &cat, false, None).unwrap(), would);
    assert!(!history_path(root, Market::Nem, "GONE").exists());
    assert!(histories_dir(root, Market::Nem).join("notes.txt").exists());
    assert_eq!(
        fold(root, Market::Nem, &cat, true, None).unwrap().changed,
        0,
        "the same bytes again"
    );

    let text =
        std::fs::read_to_string(history_path(root, Market::Nem, "DISPATCH_UNIT_SCADA")).unwrap();
    assert!(
        text.ends_with("}\n") && text.starts_with("{\n  \"breaking\": []"),
        "{text}"
    );
    let h: History = serde_json::from_str(&text).unwrap();
    assert_eq!(render(&h), text);
    // Typed by the release: NUMERIC(16,6).
    let f = h.schemas[0].by_name("SCADAVALUE").unwrap();
    assert_eq!(f.ty, utilake_iceberg::IcebergType::decimal(16, 6).unwrap());
    assert_eq!(h.schemas[0].identifier_field_ids, [1]);
    // Two census months and two report runs, in one run: report files
    // carry no SHA-256.
    let run = &h.runs[0];
    assert_eq!((run.files, run.rows), (10, 12));
    assert_eq!(run.kinds, ["dvd", "reports"]);
    assert_eq!(run.from.file, "SCADA_2026-07.CSV");
    assert_eq!(run.from.release, "2026-07");
    assert_eq!(
        run.from.data_time.as_deref(),
        Some("2026-07-01T00:05:00+10:00")
    );
    assert_eq!(run.to.sha256, "");
    let other: History = serde_json::from_slice(
        &std::fs::read(history_path(root, Market::Nem, "DISPATCH_OTHER")).unwrap(),
    )
    .unwrap();
    assert!(other.schemas[0].identifier_field_ids.is_empty());
}

#[test]
fn the_published_history_is_kept_and_a_wider_month_splits() {
    let (dir, cat) = repo();
    let root = dir.path();
    fold(root, Market::Nem, &cat, false, None).unwrap();
    // Publish it as the export does, beside a directory that publishes
    // none and a table of format 2, which is refused: the format change
    // has been crossed.
    let text =
        std::fs::read_to_string(history_path(root, Market::Nem, "DISPATCH_UNIT_SCADA")).unwrap();
    let old: History = serde_json::from_str(&text).unwrap();
    publish(root, &old);
    std::fs::create_dir_all(root.join("nem/EMPTY")).unwrap();
    std::fs::write(root.join("nem/README.md"), "x").unwrap();
    let plain = utilake_iceberg::table::TableMetadata::new("u", "l", &old.schemas[0], 0);
    let meta = published_path(root, Market::Nem, "LEGACY");
    crate::files::write_atomic(&meta, serde_json::to_string(&plain).unwrap().as_bytes()).unwrap();
    let e = published(root, Market::Nem).unwrap_err().to_string();
    assert!(e.contains("no utilake.format"), "{e}");
    std::fs::remove_dir_all(root.join("nem/LEGACY")).unwrap();
    assert_eq!(published(root, Market::Nem).unwrap().len(), 1);
    // A September of 12 digits before the point: past decimal(16, 6).
    census(root, "2026-09", 12);
    let folded = fold(root, Market::Nem, &cat, false, None).unwrap();
    assert_eq!((folded.breaks, folded.changed), (1, 2));
    let new: History = serde_json::from_slice(
        &std::fs::read(history_path(root, Market::Nem, "DISPATCH_UNIT_SCADA")).unwrap(),
    )
    .unwrap();
    assert_eq!(check(&old, &new), []);
    let b = &new.breaking[0];
    assert_eq!(
        (b.old_type.as_str(), b.new_type.as_str()),
        ("decimal(16, 6)", "decimal(18, 6)")
    );
    assert_eq!(
        (b.at.file.as_str(), b.at.release.as_str()),
        ("SCADA_2026-09.CSV", "2026-09")
    );
}

/// A fold that keeps no published history: the ids are minted anew, and
/// the baseline record says which data version began and why.
#[test]
fn a_rebaseline_keeps_nothing_and_records_why() {
    let (dir, cat) = repo();
    let root = dir.path();
    fold(root, Market::Nem, &cat, false, None).unwrap();
    let text =
        std::fs::read_to_string(history_path(root, Market::Nem, "DISPATCH_UNIT_SCADA")).unwrap();
    publish(root, &serde_json::from_str(&text).unwrap());
    assert_eq!(Baseline::read(root, Market::Nem).unwrap(), Baseline::FIRST);
    census(root, "2026-09", 12);
    // Kept, this would be a split; not kept, one wider field. Every
    // history changes, and the record.
    let would = fold(root, Market::Nem, &cat, true, Some("wider evidence")).unwrap();
    assert_eq!((would.breaks, would.changed), (0, would.tables + 1));
    assert!(!baseline_path(root, Market::Nem).exists());
    let folded = fold(root, Market::Nem, &cat, false, Some("wider evidence")).unwrap();
    assert_eq!(folded, would);
    let new: History = serde_json::from_slice(
        &std::fs::read(history_path(root, Market::Nem, "DISPATCH_UNIT_SCADA")).unwrap(),
    )
    .unwrap();
    assert!(new.breaking.is_empty());
    assert_eq!(new.schemas.len(), 1);
    let b = Baseline::read(root, Market::Nem).unwrap();
    assert_eq!(
        b,
        Baseline {
            version: 2,
            reason: "wider evidence".into()
        }
    );
    assert_eq!(b.render(), "version = 2\nreason = \"wider evidence\"\n");
    // Again: version 3; a record that cannot be read is an error.
    fold(root, Market::Nem, &cat, false, Some("again")).unwrap();
    assert_eq!(Baseline::read(root, Market::Nem).unwrap().version, 3);
    std::fs::write(baseline_path(root, Market::Nem), "version = ").unwrap();
    assert!(Baseline::read(root, Market::Nem).is_err());
    assert!(fold(root, Market::Nem, &cat, false, Some("x")).is_err());
    std::fs::remove_file(baseline_path(root, Market::Nem)).unwrap();
    std::fs::create_dir(baseline_path(root, Market::Nem)).unwrap();
    assert!(Baseline::read(root, Market::Nem).is_err());
}

#[test]
fn every_failure_names_its_file() {
    let (dir, cat) = repo();
    let root = dir.path();
    let err = |root: &Path, cat: &Catalog| {
        fold(root, Market::Nem, cat, false, None)
            .unwrap_err()
            .to_string()
    };
    // Published documents that cannot be read.
    let table = published_path(root, Market::Nem, "DISPATCH_UNIT_SCADA");
    crate::files::write_atomic(&table, b"{").unwrap();
    assert!(
        err(root, &cat).starts_with("the published history "),
        "{}",
        err(root, &cat)
    );
    let mut m: serde_json::Value = {
        std::fs::remove_file(&table).unwrap();
        fold(root, Market::Nem, &cat, false, None).unwrap();
        let h: History = serde_json::from_slice(
            &std::fs::read(history_path(root, Market::Nem, "DISPATCH_UNIT_SCADA")).unwrap(),
        )
        .unwrap();
        publish(root, &h);
        serde_json::from_slice(&std::fs::read(&table).unwrap()).unwrap()
    };
    m["properties"]["utilake.declared"] = serde_json::json!("maybe");
    crate::files::write_atomic(&table, m.to_string().as_bytes()).unwrap();
    assert!(
        err(root, &cat).contains(
            "v1.metadata.json cannot be read: in the table's properties, utilake.declared"
        ),
        "{}",
        err(root, &cat)
    );
    std::fs::remove_file(&table).unwrap();
    std::fs::create_dir_all(&table).unwrap();
    assert!(err(root, &cat).contains("v1.metadata.json cannot be read"));
    std::fs::remove_dir_all(root.join("nem")).unwrap();
    std::fs::write(root.join("nem"), "a file").unwrap();
    assert!(err(root, &cat).contains("nem cannot be read"));
    std::fs::remove_file(root.join("nem")).unwrap();
    std::fs::remove_dir_all(histories_dir(root, Market::Nem)).unwrap();

    // An override that cannot hold the values.
    let mut t: TableDecl = cat.tables()[0].clone();
    t.overrides = vec![Override {
        column: "SCADAVALUE".into(),
        ty: "long".into(),
        reason: "r".into(),
    }];
    let forced = Catalog::new(vec![("t.toml".into(), t)]).unwrap();
    let e = err(root, &forced);
    assert!(
        e.contains(
            "SCADAVALUE in DISPATCH_UNIT_SCADA as long cannot hold the values of SCADA_2026-07.CSV"
        ),
        "{e}"
    );
    let free = Catalog::new(vec![("p.toml".into(), decl("P", ("A", "B"), None))]).unwrap();
    assert_eq!(
        fold(root, Market::Nem, &free, true, None).unwrap().tables,
        2,
        "undeclared pairs are tables too"
    );

    // History files that cannot be written or removed.
    std::fs::create_dir_all(history_path(root, Market::Nem, "DISPATCH_UNIT_SCADA").join("x"))
        .unwrap();
    assert!(err(root, &cat).ends_with("DISPATCH_UNIT_SCADA.json could not be written"));
    let stale = history_path(root, Market::Nem, "STALE");
    std::fs::create_dir_all(stale.join("x")).unwrap();
    assert!(err(root, &cat).contains("STALE.json could not be written"));
    std::fs::remove_dir_all(histories_dir(root, Market::Nem)).unwrap();
    std::fs::write(histories_dir(root, Market::Nem), "").unwrap();
    assert!(err(root, &cat).contains("histories/nem could not be written"));
    std::fs::remove_file(histories_dir(root, Market::Nem)).unwrap();

    // Model files and census files that cannot be read.
    let model = crate::models::model_path(root, Market::Nem, "5.7");
    std::fs::write(&model, "version = ").unwrap();
    assert!(err(root, &cat).starts_with("the model file "));
    std::fs::remove_file(&model).unwrap();
    let month = census_path(root, Market::Nem, "2026-08".parse().unwrap());
    std::fs::write(&month, "x = ").unwrap();
    assert!(err(root, &cat).contains("2026-08.toml cannot be read"));
    std::fs::remove_dir_all(census_dir(root, Market::Nem)).unwrap();
    std::fs::write(census_dir(root, Market::Nem), "").unwrap();
    assert!(err(root, &cat).contains("census/nem cannot be read"));
}

/// A run of a plain feed's folder: one block of `pair`, its columns
/// written under `headers`, its time column `GASDATE` over one day.
fn plain_tally(pair: (&str, &str), columns: &[&str], headers: &[&str]) -> Tally {
    let n = columns.len();
    Tally {
        month: "2026-10".parse().unwrap(),
        csvs: 2,
        first_csv: "F.zip!F.CSV".into(),
        last_csv: "F.zip@20261021T221100+1000!F.CSV".into(),
        first_file: "F.zip".into(),
        first_sha256: "f1".into(),
        last_file: "F.zip@20261021T221100+1000".into(),
        last_sha256: "f2".into(),
        block: BlockRecord {
            shape: ShapeId(9),
            report: (pair.0.into(), pair.1.into()),
            version: 1,
            rows: 4,
            rows_repeated: 1,
            headers: headers.iter().map(|h| (*h).to_owned()).collect(),
            time: Some(columns[0].to_owned()),
            first: Some("2026/09/20".into()),
            last: Some("2026/09/21".into()),
            midnight: None,
            columns: columns.iter().map(|c| (*c).to_owned()).collect(),
            ctl: "?".repeat(n),
            kinds: "Dsd".into(),
            nulls: vec![0; n],
            int_digits: vec![0, 0, 1],
            scale: vec![0, 0, 1],
            sig_digits: vec![0, 0, 2],
            max_len: vec![10, 1, 3],
            min: vec![String::new(); n],
            max: vec![String::new(); n],
        },
    }
}

#[test]
fn another_market_folds_in_its_own_clock_with_its_headers() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let columns = ["GASDATE", "FACILITYNAME", "DEMAND"];
    let gbb = FolderCensus {
        files: 2,
        csvs: 2,
        blocks: vec![plain_tally(
            ("GBB", "ACTUAL_FLOW_STORAGE"),
            &columns,
            &["GasDate", "FACILITYNAME", "Demand"],
        )],
        ..FolderCensus::default()
    };
    record_folder(
        root,
        Market::Gas,
        (crate::reports::feed("gbb").unwrap(), ""),
        &gbb,
    )
    .unwrap();
    let cat = Catalog::new(vec![(
        "GBB_ACTUAL_FLOW_STORAGE.toml".into(),
        TableDecl {
            primary_key: vec!["GASDATE".into()],
            ..decl(
                "GBB_ACTUAL_FLOW_STORAGE",
                ("GBB", "ACTUAL_FLOW_STORAGE"),
                Some("GASDATE"),
            )
        },
    )])
    .unwrap();
    let folded = fold(root, Market::Gas, &cat, false, None).unwrap();
    assert_eq!((folded.tables, folded.runs), (1, 1));
    let h: History = serde_json::from_slice(
        &std::fs::read(history_path(root, Market::Gas, "GBB_ACTUAL_FLOW_STORAGE")).unwrap(),
    )
    .unwrap();
    let run = &h.runs[0];
    assert_eq!(run.kinds, ["gbb"]);
    assert_eq!((run.files, run.rows), (2, 4));
    assert_eq!(run.from.file, "F.zip");
    assert_eq!(run.to.file, "F.zip@20261021T221100+1000");
    assert_eq!(
        run.from.data_time.as_deref(),
        Some("2026-09-20T00:00:00+10:00")
    );
    // The header a column was written under, where it is not the name.
    let gasdate = h.schemas[0].by_name("GASDATE").unwrap().id;
    let demand = h.schemas[0].by_name("DEMAND").unwrap().id;
    assert_eq!(h.headers.get(&gasdate).map(String::as_str), Some("GasDate"));
    assert_eq!(h.headers.get(&demand).map(String::as_str), Some("Demand"));
    assert_eq!(h.headers.len(), 2, "a header that is its name is not kept");
    let text = std::fs::read_to_string(history_path(root, Market::Gas, "GBB_ACTUAL_FLOW_STORAGE"))
        .unwrap();
    assert!(text.contains("\"headers\": {"), "{text}");
    assert_eq!(
        fold(root, Market::Gas, &cat, true, None).unwrap().changed,
        0
    );

    // The WEM: its own clock, and a pair no table declares named without
    // the hyphen.
    let wem = FolderCensus {
        files: 2,
        csvs: 2,
        blocks: vec![plain_tally(("WEM", "FACILITY-SCADA"), &columns, &[])],
        ..FolderCensus::default()
    };
    record_folder(
        root,
        Market::Wem,
        (crate::reports::feed("datafiles").unwrap(), "facility-scada"),
        &wem,
    )
    .unwrap();
    let none = Catalog::new(vec![]).unwrap();
    fold(root, Market::Wem, &none, false, None).unwrap();
    let h: History = serde_json::from_slice(
        &std::fs::read(history_path(root, Market::Wem, "WEM_FACILITY_SCADA")).unwrap(),
    )
    .unwrap();
    assert_eq!(h.runs[0].kinds, ["datafiles"]);
    assert_eq!(
        h.runs[0].from.data_time.as_deref(),
        Some("2026-09-20T00:00:00+08:00")
    );
    assert!(h.headers.is_empty());
    assert!(
        !std::fs::read_to_string(history_path(root, Market::Wem, "WEM_FACILITY_SCADA"))
            .unwrap()
            .contains("headers")
    );
}

#[test]
fn reports_folders_are_found_below_their_root() {
    let (dir, _) = repo();
    let found = recorded_folders(dir.path(), Market::Nem, nem_reports()).unwrap();
    assert_eq!(found, ["Dispatch_SCADA/2026", ""], "by path");
    assert!(
        recorded_folders(
            dir.path(),
            Market::Gas,
            crate::reports::feed("gbb").unwrap()
        )
        .unwrap()
        .is_empty()
    );
    assert!(
        recorded_months(&dir.path().join("none"), Market::Nem)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn market_times() {
    let nem = |t: &str| market_time(t, "+10:00");
    assert_eq!(
        nem("2024/08/01 00:05:00").as_deref(),
        Some("2024-08-01T00:05:00+10:00")
    );
    assert_eq!(
        nem("2024/08/01 00:05:00.5").as_deref(),
        Some("2024-08-01T00:05:00.5+10:00")
    );
    assert_eq!(
        nem("2024/08/01").as_deref(),
        Some("2024-08-01T00:00:00+10:00")
    );
    assert_eq!(
        market_time("2024/08/01 08:00:00", "+08:00").as_deref(),
        Some("2024-08-01T08:00:00+08:00")
    );
    assert_eq!(nem("2024/08/01 00:05"), None);
    assert_eq!(nem("2024-08-01"), None);
    assert_eq!(nem("2024/08/01X"), None);
}

#[test]
fn a_valid_column_seen_only_at_midnight_is_noted_unless_declared_a_day() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let mut b = scada_block("2026-08", 3);
    b.first = Some("2026/08/01".into());
    b.last = Some("2026/08/31".into());
    b.midnight = Some(true);
    let files = BTreeMap::from([(
        "SCADA_2026-08.CSV".to_owned(),
        FileRecord {
            source: "x.zip".into(),
            sha256: "ab".into(),
            size: 9,
            kind: "dvd".into(),
            bad_rows: 0,
            footer_mismatches: 0,
            block: vec![b],
        },
    )]);
    let path = census_path(root, Market::Nem, "2026-08".parse().unwrap());
    crate::files::write_atomic(&path, toml::to_string(&files).unwrap().as_bytes()).unwrap();

    // Derived roles call SETTLEMENTDATE an interval end: a note.
    let f = fold(root, Market::Nem, &scada(), true, None).unwrap();
    assert_eq!(f.notes.len(), 1, "{:?}", f.notes);
    assert!(
        f.notes[0].starts_with(
            "DISPATCH_UNIT_SCADA: every recorded value of SETTLEMENTDATE is at midnight"
        ) && f.notes[0].contains("valid = { kind = \"day\", column = \"SETTLEMENTDATE\" }")
            && f.notes[0].ends_with("tables/nem/DISPATCH_UNIT_SCADA.toml"),
        "{}",
        f.notes[0]
    );
    // A hand block that calls it a day: nothing to say.
    let mut d = crate::catalog::tests::decl(
        "DISPATCH_UNIT_SCADA",
        ("DISPATCH", "UNIT_SCADA"),
        Some("SETTLEMENTDATE"),
    );
    d.temporal = Some(
        toml::from_str(
            "entity = []\norder = []\nvalid = { kind = \"day\", column = \"SETTLEMENTDATE\" }",
        )
        .unwrap(),
    );
    let cat = Catalog::new(vec![("DISPATCH_UNIT_SCADA.toml".into(), d)]).unwrap();
    assert!(
        fold(root, Market::Nem, &cat, true, None)
            .unwrap()
            .notes
            .is_empty()
    );
    // A table no declaration names has no roles to hold it to.
    let none = Catalog::new(vec![]).unwrap();
    assert!(
        fold(root, Market::Nem, &none, true, None)
            .unwrap()
            .notes
            .is_empty()
    );
}

/// Records a month's census of one file per block, each block of
/// `report` with `columns` (three, typed as SCADA's are).
fn census_of(root: &Path, month: &str, report: (&str, &str), blocks: &[[&str; 3]]) {
    let files: BTreeMap<String, FileRecord> = blocks
        .iter()
        .enumerate()
        .map(|(i, columns)| {
            let b = BlockRecord {
                report: (report.0.into(), report.1.into()),
                columns: columns.iter().map(|c| (*c).to_owned()).collect(),
                shape: ShapeId(10 + i as u64),
                ..scada_block(month, 3)
            };
            let f = FileRecord {
                source: format!("F{i}.zip"),
                sha256: "ab".into(),
                size: 9,
                kind: "dvd".into(),
                bad_rows: 0,
                footer_mismatches: 0,
                block: vec![b],
            };
            (format!("F{i}_{month}.CSV"), f)
        })
        .collect();
    let path = census_path(root, Market::Nem, month.parse().unwrap());
    crate::files::write_atomic(&path, toml::to_string(&files).unwrap().as_bytes()).unwrap();
}

/// A declaration of `name` for `pair` keyed on `key`, taking only blocks
/// with `with`.
fn keyed(name: &str, pair: (&str, &str), key: &[&str], with: &[&str]) -> (PathBuf, TableDecl) {
    let d = TableDecl {
        primary_key: key.iter().map(|c| (*c).to_owned()).collect(),
        with_columns: with.iter().map(|c| (*c).to_owned()).collect(),
        ..decl(name, pair, Some("SETTLEMENTDATE"))
    };
    (format!("{name}.toml").into(), d)
}

#[test]
fn a_pair_of_two_kinds_folds_into_two_tables_and_a_key_left_out_fails() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let pair = ("ROOFTOP", "ACTUAL");
    census_of(
        root,
        "2026-08",
        pair,
        &[
            ["SETTLEMENTDATE", "REGIONID", "POWER"],
            ["SETTLEMENTDATE", "AREAID", "POWER"],
        ],
    );
    let region = keyed(
        "ROOFTOP_PV_ACTUAL",
        pair,
        &["SETTLEMENTDATE", "REGIONID"],
        &[],
    );
    let area = keyed(
        "ROOFTOP_PV_ACTUAL_AREA",
        pair,
        &["SETTLEMENTDATE", "AREAID"],
        &["AREAID"],
    );
    let both = Catalog::new(vec![region.clone(), area]).unwrap();
    let folded = fold(root, Market::Nem, &both, false, None).unwrap();
    assert_eq!((folded.tables, folded.schemas), (2, 2));
    let h: History = serde_json::from_slice(
        &std::fs::read(history_path(root, Market::Nem, "ROOFTOP_PV_ACTUAL_AREA")).unwrap(),
    )
    .unwrap();
    assert!(h.schemas[0].by_name("AREAID").is_some());
    // With one table for both kinds, the area's schema has no REGIONID.
    let one = Catalog::new(vec![region]).unwrap();
    let e = fold(root, Market::Nem, &one, true, None).unwrap_err();
    assert!(
        matches!(&e, FoldError::KeyMissing { column, .. } if column == "REGIONID"),
        "{e}"
    );
    assert!(e.to_string().contains("another kind of row"), "{e}");
}

#[test]
fn a_renamed_column_is_one_field_and_an_undecided_one_fails() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let pair = ("MTPASA", "REGIONRESULT");
    census_of(
        root,
        "2024-07",
        pair,
        &[["SETTLEMENTDATE", "DUID", "SPELT"]],
    );
    census_of(
        root,
        "2024-08",
        pair,
        &[["SETTLEMENTDATE", "DUID", "SPELLED"]],
    );
    let (path, plain) = keyed("MTPASA_REGIONRESULT", pair, &["SETTLEMENTDATE"], &[]);
    let with = |d: TableDecl| Catalog::new(vec![(path.clone(), d)]).unwrap();
    // Nothing says: the fold names the pair.
    let e = fold(root, Market::Nem, &with(plain.clone()), true, None).unwrap_err();
    assert!(e.to_string().contains("SPELT to SPELLED"), "{e}");
    // A rename: one field, renamed in the second run.
    let renamed = TableDecl {
        renames: vec![crate::catalog::Rename {
            from: "SPELT".into(),
            to: "SPELLED".into(),
            evidence: "respelt in version 2".into(),
        }],
        ..plain.clone()
    };
    fold(root, Market::Nem, &with(renamed.clone()), false, None).unwrap();
    let h: History = serde_json::from_slice(
        &std::fs::read(history_path(root, Market::Nem, "MTPASA_REGIONRESULT")).unwrap(),
    )
    .unwrap();
    let fields = h.fields();
    assert_eq!(fields.len(), 3, "{fields:?}");
    assert_eq!(fields[2].name, "SPELLED", "the newest name");
    assert_eq!(
        h.runs[1].updates,
        [utilake_iceberg::history::Update::Rename {
            field_id: fields[2].id,
            name: "SPELLED".into()
        }]
    );
    // Distinct: two fields, decided.
    let distinct = TableDecl {
        distinct: vec![crate::catalog::Distinct {
            columns: vec!["SPELT".into(), "SPELLED".into()],
            reason: "two meanings".into(),
        }],
        ..plain
    };
    fold(root, Market::Nem, &with(distinct), false, None).unwrap();
    let h: History = serde_json::from_slice(
        &std::fs::read(history_path(root, Market::Nem, "MTPASA_REGIONRESULT")).unwrap(),
    )
    .unwrap();
    assert_eq!(h.fields().len(), 4);
    // One block with both names: they are two columns, not a rename.
    census_of(root, "2024-09", pair, &[["SPELT", "DUID", "SPELLED"]]);
    let e = fold(root, Market::Nem, &with(renamed), true, None).unwrap_err();
    assert!(e.to_string().contains("holds both"), "{e}");
}

#[test]
fn an_undeclared_tables_look_alike_columns_are_a_note() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let pair = ("DREGION", "");
    census_of(
        root,
        "2025-08",
        pair,
        &[["SETTLEMENTDATE", "DUID", "OLDFLAG"]],
    );
    census_of(
        root,
        "2025-09",
        pair,
        &[["SETTLEMENTDATE", "DUID", "NEWFLAG"]],
    );
    let folded = fold(root, Market::Nem, &Catalog::default(), true, None).unwrap();
    assert!(
        folded
            .notes
            .iter()
            .any(|n| n.starts_with("DREGION: 1 column(s) dropped")
                && n.contains("OLDFLAG as NEWFLAG")),
        "{:?}",
        folded.notes
    );
}

#[test]
fn where_aemos_loader_puts_a_runs_blocks_elsewhere_is_a_note() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let pair = ("ROOFTOP", "ACTUAL");
    census_of(
        root,
        "2026-08",
        pair,
        &[["SETTLEMENTDATE", "AREAID", "POWER"]],
    );
    let loads = crate::models::loads_dir(root, Market::Nem);
    std::fs::create_dir_all(&loads).unwrap();
    std::fs::write(
        loads.join("5.7.toml"),
        "version = \"5.7\"\n[[load]]\nreport = [\"ROOFTOP\", \"ACTUAL\"]\nversion = 1\ntable = \"ROOFTOP_PV_ACTUAL\"\ntransaction = \"INSERT\"\n\n[[load]]\nreport = [\"ROOFTOP\", \"ACTUAL\"]\nversion = 2\ntable = \"ROOFTOP_PV_ACTUAL\"\ntransaction = \"INSERT\"\n",
    )
    .unwrap();
    let area = keyed(
        "ROOFTOP_PV_ACTUAL_AREA",
        pair,
        &["SETTLEMENTDATE", "AREAID"],
        &["AREAID"],
    );
    let folded = fold(
        root,
        Market::Nem,
        &Catalog::new(vec![area]).unwrap(),
        true,
        None,
    )
    .unwrap();
    assert!(
        folded.notes.iter().any(|n| n
            == "ROOFTOP_PV_ACTUAL_AREA: AEMO's loader disagrees: ROOFTOP/ACTUAL v1 loads ROOFTOP_PV_ACTUAL in data model 5.7"),
        "{:?}",
        folded.notes
    );
    // An older release that loads another version, and a version none
    // loads: the newest that loads each decides.
    std::fs::write(
        loads.join("5.5.toml"),
        "version = \"5.5\"\n[[load]]\nreport = [\"ROOFTOP\", \"ACTUAL\"]\nversion = 1\ntable = \"ROOFTOP_PV_ACTUAL_AREA\"\ntransaction = \"INSERT\"\n",
    )
    .unwrap();
    census_of(
        root,
        "2026-09",
        ("ROOFTOP", "FORECAST"),
        &[["SETTLEMENTDATE", "AREAID", "POWER"]],
    );
    std::fs::write(
        loads.join("5.10.toml"),
        "version = \"5.10\"\n[[load]]\nreport = [\"ROOFTOP\", \"FORECAST\"]\nversion = 2\ntable = \"ROOFTOP_PV_FORECAST\"\ntransaction = \"INSERT\"\n",
    )
    .unwrap();
    std::fs::write(loads.join("README.md"), "not a release").unwrap();
    let forecast = keyed(
        "ROOFTOP_PV_FORECAST_AREA",
        ("ROOFTOP", "FORECAST"),
        &["SETTLEMENTDATE", "AREAID"],
        &["AREAID"],
    );
    let area = keyed(
        "ROOFTOP_PV_ACTUAL_AREA",
        pair,
        &["SETTLEMENTDATE", "AREAID"],
        &["AREAID"],
    );
    let both = Catalog::new(vec![area, forecast]).unwrap();
    let folded = fold(root, Market::Nem, &both, true, None).unwrap();
    let said: Vec<&String> = folded
        .notes
        .iter()
        .filter(|n| n.contains("loader"))
        .collect();
    assert_eq!(
        said,
        [
            "ROOFTOP_PV_ACTUAL_AREA: AEMO's loader disagrees: ROOFTOP/ACTUAL v1 loads ROOFTOP_PV_ACTUAL in data model 5.7",
            "ROOFTOP_PV_FORECAST_AREA: AEMO's loader disagrees: ROOFTOP/FORECAST v1 is a version no recorded data model loads",
        ],
        "5.10 is newer than 5.7, and 5.7 newer than 5.5"
    );
    // A loads directory that is not one cannot be read.
    std::fs::remove_dir_all(&loads).unwrap();
    std::fs::write(&loads, "a file").unwrap();
    assert!(fold(root, Market::Nem, &both, true, None).is_err());
}
