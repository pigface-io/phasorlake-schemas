use utilake_iceberg::compat::check;

use super::*;
use crate::catalog::tests::{decl, scada};
use crate::catalog::{Override, TableDecl};
use crate::census::{BlockRecord, census_path};
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
        time: Some("SETTLEMENTDATE".into()),
        first: Some(format!("{day} 00:05:00")),
        last: Some(format!("{day} 00:15:00")),
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
        headers: BTreeMap::new(),
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
            ..scada_block("2026-08", 2)
        },
    };
    let folder = |csv: &str| FolderCensus {
        files: 1,
        csvs: 4,
        blocks: vec![tally(csv)],
        ..FolderCensus::default()
    };
    record_folder(root, Market::Nem, "ARCHIVE", "", &folder("ROOT")).unwrap();
    record_folder(
        root,
        Market::Nem,
        "ARCHIVE",
        "Dispatch_SCADA/2026",
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
            changed: 3
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
    assert_eq!(run.kinds, ["dvd", "reports/archive"]);
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
    // none and a table of format 2, whose history is in its table.json.
    let text =
        std::fs::read_to_string(history_path(root, Market::Nem, "DISPATCH_UNIT_SCADA")).unwrap();
    let old: History = serde_json::from_str(&text).unwrap();
    publish(root, &old);
    std::fs::create_dir_all(root.join("nem/EMPTY")).unwrap();
    std::fs::write(root.join("nem/README.md"), "x").unwrap();
    let mut legacy = old.clone();
    legacy.table = "LEGACY".into();
    let plain = utilake_iceberg::table::TableMetadata::new("u", "l", &legacy.schemas[0], 0);
    let meta = published_path(root, Market::Nem, "LEGACY");
    crate::files::write_atomic(&meta, serde_json::to_string(&plain).unwrap().as_bytes()).unwrap();
    let err = |root: &Path| published(root, Market::Nem).unwrap_err().to_string();
    assert!(
        err(root).contains("LEGACY/table.json cannot be read"),
        "{}",
        err(root)
    );
    let doc = root.join("nem/LEGACY/table.json");
    std::fs::write(&doc, "{}").unwrap();
    assert_eq!(
        published(root, Market::Nem).unwrap().len(),
        1,
        "no iceberg block: no history"
    );
    std::fs::write(&doc, r#"{"iceberg": {"table": 1}}"#).unwrap();
    assert!(
        err(root).contains("LEGACY/table.json cannot be read"),
        "{}",
        err(root)
    );
    std::fs::write(&doc, serde_json::json!({"iceberg": &legacy}).to_string()).unwrap();
    let both = published(root, Market::Nem).unwrap();
    assert_eq!(both.len(), 2);
    assert_eq!(both["LEGACY"], legacy);
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
        err(root, &cat).starts_with("The published history "),
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
            "v1.metadata.json cannot be read: In the table's properties, utilake.declared"
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
    assert!(err(root, &cat).ends_with("DISPATCH_UNIT_SCADA.json could not be written."));
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
    assert!(err(root, &cat).starts_with("The model file "));
    std::fs::remove_file(&model).unwrap();
    let month = census_path(root, Market::Nem, "2026-08".parse().unwrap());
    std::fs::write(&month, "x = ").unwrap();
    assert!(err(root, &cat).contains("2026-08.toml cannot be read"));
    std::fs::remove_dir_all(census_dir(root, Market::Nem)).unwrap();
    std::fs::write(census_dir(root, Market::Nem), "").unwrap();
    assert!(err(root, &cat).contains("census/nem cannot be read"));
}

#[test]
fn reports_folders_are_found_below_their_root() {
    let (dir, _) = repo();
    let found = recorded_folders(dir.path(), Market::Nem, "ARCHIVE").unwrap();
    assert_eq!(found, ["Dispatch_SCADA/2026", ""], "by path");
    assert!(
        recorded_folders(dir.path(), Market::Nem, "CURRENT")
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
    assert_eq!(
        market_time("2024/08/01 00:05:00").as_deref(),
        Some("2024-08-01T00:05:00+10:00")
    );
    assert_eq!(
        market_time("2024/08/01 00:05:00.5").as_deref(),
        Some("2024-08-01T00:05:00.5+10:00")
    );
    assert_eq!(
        market_time("2024/08/01").as_deref(),
        Some("2024-08-01T00:00:00+10:00")
    );
    assert_eq!(market_time("2024/08/01 00:05"), None);
    assert_eq!(market_time("2024-08-01"), None);
    assert_eq!(market_time("2024/08/01X"), None);
}
