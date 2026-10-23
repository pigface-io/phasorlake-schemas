use super::*;

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("testdata/models/gas")
}

fn page(name: &str) -> String {
    std::fs::read_to_string(fixture().join("Gas Data Model Report_files").join(name)).unwrap()
}

#[test]
fn the_contents_name_the_version_and_each_tables_page() {
    let c = contents(&page("Gas Data Model Report_toc.htm"));
    assert_eq!(c.version.as_deref(), Some("2.2"));
    assert_eq!(c.model, "AEMO Gas Data Model v2.2");
    assert_eq!(c.pages.len(), 5);
    assert_eq!(c.pages["GAS_DAILY_FLOW_ACTUAL"], "Gas_11.htm");
    assert_eq!(
        c.pages["GAS_NOPAGE"], "Gas_15.htm",
        "a page without a fragment"
    );
    let none =
        contents(r#"<a class="Browser" href="x.htm">Description of the model Something Else.</a>"#);
    assert_eq!(
        (none.version, none.model.as_str()),
        (None, "Something Else")
    );
    assert!(contents("no anchors").pages.is_empty());
}

#[test]
fn a_tables_page_gives_its_comment_key_and_columns() {
    let (name, t) = table_page(&page("Gas_11.htm")).unwrap();
    assert_eq!(name, "GAS_DAILY_FLOW_ACTUAL");
    assert_eq!(
        t.comment,
        "Provides on each gas day D, the BB facility operator’s daily gas flow data for gas day D-1."
    );
    assert_eq!(t.primary_key, ["ConnectionPointId", "GasDate"]);
    assert_eq!(
        t.columns,
        [
            (
                "GasDate".into(),
                "DATE".into(),
                false,
                "Date of gas day".into()
            ),
            (
                "ConnectionPointId".into(),
                "NUMBER(10,0)".into(),
                false,
                "Unique connection point identifier.".into()
            ),
            (
                "ActualQuantity".into(),
                "NUMBER(18,3)".into(),
                true,
                "The actual flow quantity reported in TJ & GJ, \"to the nearest\".".into()
            ),
            ("Quality".into(), "VARCHAR2 (5)".into(), true, String::new()),
        ]
    );
    let (name, basin) = table_page(&page("Gas_12.htm")).unwrap();
    assert_eq!(name, "GAS_BASIN");
    assert!(basin.primary_key.is_empty() && basin.comment.is_empty());
    assert_eq!(basin.columns.len(), 2);
    assert_eq!(basin.columns[1].3, "Name < 100");
    assert_eq!(table_page(&page("Gas_13.htm")), None);
}

#[test]
fn html_is_read_leniently() {
    assert_eq!(text_of("a <b>b</b> &amp; c <unclosed"), "a b & c");
    assert_eq!(
        blocks("<td>x</td><tdx>y</tdx><td\n>z</td>", "td"),
        ["x", "z"]
    );
    assert_eq!(blocks("<td>open", "td"), Vec::<&str>::new());
    assert_eq!(blocks("<td", "td"), Vec::<&str>::new());
    assert_eq!(
        anchors(r#"<a href="p.htm">T</a><a name="x">no href</a>"#).len(),
        1
    );
}

#[test]
fn a_report_is_a_model_with_its_unreadable_pages_named() {
    let month = YearMonth::new(2026, 10).unwrap();
    let (m, problems) = read_report(&fixture(), month, None).unwrap();
    assert_eq!(
        (m.version.as_str(), m.model.as_str()),
        ("2.2", "AEMO Gas Data Model v2.2")
    );
    assert_eq!((m.first_seen, m.last_seen), (month, month));
    assert_eq!(m.script_sha256.len(), 64);
    assert!(m.created.is_empty());
    let names: Vec<&str> = m.tables.keys().map(String::as_str).collect();
    assert_eq!(names, ["GAS_BASIN", "GAS_DAILY_FLOW_ACTUAL"]);
    assert_eq!(problems.len(), 3, "{problems:?}");
    assert!(
        problems[0].ends_with("Gas_13.htm does not describe table GAS_MISSING."),
        "{}",
        problems[0]
    );
    assert!(problems[1].contains("Gas_15.htm"), "{}", problems[1]);
    assert!(
        problems[2].ends_with("Gas_14.htm does not describe table GAS_WRONG."),
        "{}",
        problems[2]
    );
    // The pages directory itself reads the same; a version given wins.
    let (same, _) = read_report(
        &fixture().join("Gas Data Model Report_files"),
        month,
        Some("9.9"),
    )
    .unwrap();
    assert_eq!((same.version.as_str(), &same.tables), ("9.9", &m.tables));
    // Nothing to read.
    let empty = tempfile::tempdir().unwrap();
    let e = read_report(empty.path(), month, None)
        .unwrap_err()
        .to_string();
    assert!(e.contains("no `*_toc.htm`"), "{e}");
    std::fs::write(
        empty.path().join("X_toc.htm"),
        "<a href=\"a.htm\">Table: A</a>",
    )
    .unwrap();
    let e = read_report(empty.path(), month, None)
        .unwrap_err()
        .to_string();
    assert!(e.contains("name no version"), "{e}");
    std::fs::create_dir(empty.path().join("X_files")).unwrap();
    std::fs::remove_file(empty.path().join("X_toc.htm")).unwrap();
    assert!(read_report(empty.path(), month, None).is_err());
}

#[test]
fn a_report_is_recorded_like_a_script_and_a_second_month_widens() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let oct = YearMonth::new(2026, 10).unwrap();
    let r = record(root, Market::Gas, oct, &fixture(), None).unwrap();
    assert_eq!(r.versions, ["2.2"]);
    assert_eq!(r.new_versions, ["2.2"]);
    assert_eq!(r.problems.len(), 3);
    let models = crate::models::read_models(root, Market::Gas).unwrap();
    assert_eq!(models["2.2"].tables.len(), 2);
    let nov = YearMonth::new(2026, 11).unwrap();
    let r = record(root, Market::Gas, nov, &fixture(), None).unwrap();
    assert!(r.new_versions.is_empty());
    let models = crate::models::read_models(root, Market::Gas).unwrap();
    assert_eq!(
        (models["2.2"].first_seen, models["2.2"].last_seen),
        (oct, nov)
    );
    let months = crate::models::read_months(root, Market::Gas).unwrap();
    assert_eq!(months.len(), 2);
    // The fold reads the report's numbers as declarations.
    let d = crate::fold::declarations(
        root,
        Market::Gas,
        &crate::catalog::Catalog::new(vec![]).unwrap(),
    )
    .unwrap();
    assert_eq!(
        d.numeric("GAS_DAILY_FLOW_ACTUAL", oct)
            .and_then(|n| n.get("ACTUALQUANTITY"))
            .copied(),
        Some((18, 3))
    );
    // A root that is a file: nothing can be recorded under it.
    let blocked = root.join("blocked");
    std::fs::write(&blocked, "").unwrap();
    assert!(record(&blocked, Market::Gas, oct, &fixture(), None).is_err());
}
