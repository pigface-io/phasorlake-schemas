use utilake_iceberg::compat::check;
use utilake_iceberg::history::{Change, Update};

use super::*;
use crate::shape::ShapeId;

/// A column: name, observed kind, digits before and after the point,
/// nulls.
type Col<'a> = (&'a str, Kind, u8, u8, u64);

fn record(kind: &str) -> FileRecord {
    FileRecord {
        source: "src".into(),
        sha256: "ab".into(),
        size: 1,
        kind: kind.into(),
        bad_rows: 0,
        footer_mismatches: 0,
        block: Vec::new(),
    }
}

fn block(version: u16, span: Option<(&str, &str)>, cols: &[Col<'_>]) -> BlockRecord {
    BlockRecord {
        shape: ShapeId(1),
        report: ("DISPATCH".into(), "PRICE".into()),
        version,
        rows: 10,
        rows_repeated: 0,
        headers: Vec::new(),
        time: span.map(|_| "SETTLEMENTDATE".into()),
        first: span.map(|s| s.0.into()),
        last: span.map(|s| s.1.into()),
        midnight: None,
        columns: cols.iter().map(|c| c.0.into()).collect(),
        ctl: cols.iter().map(|_| TypeClass::Unknown.letter()).collect(),
        kinds: cols.iter().map(|c| c.1.letter()).collect(),
        nulls: cols.iter().map(|c| c.4).collect(),
        int_digits: cols.iter().map(|c| c.2).collect(),
        scale: cols.iter().map(|c| c.3).collect(),
        sig_digits: cols.iter().map(|c| c.2 + c.3).collect(),
        max_len: cols.iter().map(|_| 8).collect(),
        min: cols.iter().map(|_| String::new()).collect(),
        max: cols.iter().map(|_| String::new()).collect(),
    }
}

const T: Col<'static> = ("SETTLEMENTDATE", Kind::Timestamp, 0, 0, 0);
const R: Col<'static> = ("REGIONID", Kind::Text, 0, 0, 0);
const P: Col<'static> = ("RRP", Kind::Decimal, 3, 2, 0);
const X: Col<'static> = ("RAISE1SECRRP", Kind::Decimal, 2, 2, 3);

fn decl() -> Declarations {
    Declarations {
        keys: BTreeMap::from([(
            "DISPATCHPRICE".into(),
            vec!["SETTLEMENTDATE".into(), "REGIONID".into()],
        )]),
        ..Declarations::default()
    }
}

fn month(m: &str) -> YearMonth {
    m.parse().unwrap()
}

fn span_of(m: &str) -> (String, String) {
    (
        format!("{}/{}/01 00:05:00", &m[..4], &m[5..]),
        format!("{}/{}/28 00:00:00", &m[..4], &m[5..]),
    )
}

/// A month's block: its release month, version, kind and columns.
type Month<'a> = (&'a str, u16, &'a str, &'a [Col<'a>]);

/// One block per month, in the order given, each spanning that month,
/// from a file named for the month.
fn observe(blocks: &[Month<'_>]) -> Observations {
    let mut h = Observations::default();
    for (m, version, kind, cols) in blocks {
        let (start, end) = span_of(m);
        h.block(
            "DISPATCHPRICE",
            month(m),
            (&format!("{m}.zip"), 0),
            &record(kind),
            &block(*version, Some((&start, &end)), cols),
        );
    }
    h
}

/// A run of report files of month `m`, ending on its day 29 so it sorts
/// after the month's archive block, as one observation.
fn report_run(h: &mut Observations, m: &str, cols: &[Col<'_>]) {
    let (start, _) = span_of(m);
    let end = format!("{}/{}/29 00:00:00", &m[..4], &m[5..]);
    let (first, last) = (format!("R{m}a.csv"), format!("R{m}b.csv"));
    h.run(
        "DISPATCHPRICE",
        month(m),
        ((&first, "cd"), (&last, "ef"), 2),
        &record("reports/archive"),
        &block(1, Some((&start, &end)), cols),
    );
}

/// The fold of `blocks` under `d`, kept to `published`.
fn fold_with(
    blocks: &[Month<'_>],
    d: &Declarations,
    published: Option<&History>,
) -> Result<History, HistoryError> {
    let published: BTreeMap<String, History> = published
        .map(|h| (h.table.clone(), h.clone()))
        .into_iter()
        .collect();
    observe(blocks)
        .finish(d, &published)
        .map(|mut t| t.remove("DISPATCHPRICE").unwrap())
}

fn months(blocks: &[Month<'_>]) -> History {
    fold_with(blocks, &decl(), None).unwrap()
}

fn schemas_of(h: &History) -> Vec<u32> {
    h.runs.iter().map(|r| r.schema_id).collect()
}

fn add(id: u32) -> Update {
    Update::Add { field_id: id }
}

#[test]
fn a_column_list_that_comes_back_keeps_its_schema_and_ids() {
    let h = months(&[
        ("2024-06", 1, "dvd", &[T, R, P]),
        ("2024-07", 1, "dvd", &[T, R, X, P]),
        ("2024-08", 1, "dvd", &[T, R, P]),
        ("2024-09", 1, "dvd", &[T, R, X, P]),
    ]);
    assert_eq!(schemas_of(&h), [0, 1, 0, 1]);
    assert_eq!(h.schemas.len(), 2);
    assert_eq!(h.schemas[1].by_name("RAISE1SECRRP").map(|f| f.id), Some(4));
    assert_eq!(h.runs[1].updates, [add(4)], "an insertion moves nothing");
    assert_eq!(h.runs[2].updates, [Update::Delete { field_id: 4 }]);
    assert_eq!(h.runs[3].updates, [Update::Restore { field_id: 4 }]);
    assert_eq!(h.runs[0].changes, [Change::First]);
    assert_eq!(h.runs[1].changes, [Change::Schema]);
    assert!(h.breaking.is_empty());
    // The key: both columns, both required; RAISE1SECRRP held nulls.
    assert_eq!(h.schemas[0].identifier_field_ids, [1, 2]);
    assert!(!h.schemas[1].by_name("RAISE1SECRRP").unwrap().required);
    assert_eq!((h.table.as_str(), h.format_version), ("DISPATCHPRICE", 3));
}

#[test]
fn a_column_keeps_one_type_until_its_values_turn_to_text() {
    let wide = ("RRP", Kind::Decimal, 9, 3, 0);
    let text = ("RRP", Kind::Text, 0, 0, 0);
    let h = months(&[
        ("2024-01", 1, "dvd", &[T, R, P]),
        ("2024-02", 1, "dvd", &[T, R, wide]),
        ("2024-03", 1, "dvd", &[T, R, P]),
        ("2024-04", 1, "dvd", &[T, R, text]),
        ("2024-05", 1, "dvd", &[T, R, P]),
    ]);
    // The widest the lineage holds, from its first file: no change for
    // more digits, and none Iceberg could not make for a wider scale.
    let rrp = |s: usize| h.schemas[s].by_name("RRP").map(|f| (f.id, f.ty));
    let wide_ty = IcebergType::decimal(12, 3).unwrap();
    assert_eq!(rrp(0), Some((3, wide_ty)));
    assert_eq!(h.runs[0].files, 3);
    // Text ends the lineage: a new field, and text holds what follows.
    assert_eq!(rrp(1), Some((4, IcebergType::String)));
    assert_eq!(schemas_of(&h), [0, 1]);
    let b = &h.breaking[0];
    assert_eq!((b.old_field_id, b.old_type.as_str()), (3, "decimal(12, 3)"));
    assert_eq!((b.new_field_id, b.new_type.as_str()), (4, "string"));
    assert_eq!(
        (b.at.file.as_str(), b.at.release.as_str()),
        ("2024-04.zip", "2024-04")
    );
    assert_eq!(h.runs[1].updates, [Update::Delete { field_id: 3 }, add(4)]);
}

#[test]
fn a_column_null_at_first_takes_the_type_its_values_have_later() {
    let empty = ("LASTCHANGED", Kind::Empty, 0, 0, 5);
    let times = ("LASTCHANGED", Kind::Timestamp, 0, 0, 0);
    let h = months(&[
        ("2024-10", 1, "dvd", &[T, R, empty]),
        ("2024-11", 1, "dvd", &[T, R, times]),
    ]);
    assert!(h.breaking.is_empty());
    assert_eq!(schemas_of(&h), [0]);
    let f = h.schemas[0].by_name("LASTCHANGED").unwrap();
    assert_eq!((f.id, f.ty), (3, IcebergType::Timestamptz));
}

#[test]
fn a_key_column_that_takes_a_null_leaves_the_key() {
    let null_region = ("REGIONID", Kind::Text, 0, 0, 2);
    let h = months(&[
        ("2024-01", 1, "dvd", &[T, R, P]),
        ("2024-02", 1, "dvd", &[T, null_region, P]),
        ("2024-03", 1, "dvd", &[T, R, P]),
    ]);
    assert_eq!(
        h.runs[1].updates,
        [
            Update::MakeOptional { field_id: 2 },
            Update::SetIdentifierFields { field_ids: vec![1] }
        ]
    );
    assert_eq!(schemas_of(&h), [0, 1], "once optional, always optional");
}

#[test]
fn a_moved_column_is_one_move() {
    let h = months(&[
        ("2024-01", 1, "dvd", &[T, R, P, X]),
        ("2024-02", 1, "dvd", &[X, T, R, P]),
    ]);
    assert_eq!(h.runs[1].updates, [Update::Move { field_id: 4 }]);
}

#[test]
fn versions_and_reports_change_runs_not_schemas() {
    let h = months(&[
        ("2024-01", 1, "dvd", &[T, R, P]),
        ("2024-02", 2, "dvd", &[T, R, P]),
        ("2024-03", 2, "archive", &[T, R, P]),
    ]);
    assert_eq!(schemas_of(&h), [0, 0]);
    assert_eq!(h.runs[1].changes, [Change::Version]);
    assert!(h.runs[1].updates.is_empty());
    // The same data as DVD and archive files is one run of both kinds.
    assert_eq!(h.runs[1].kinds, ["archive", "dvd"]);
    assert_eq!(h.runs[1].files, 2);

    let mut hist = Observations::default();
    let mut other = block(1, None, &[T, R, P]);
    other.report = ("DISPATCH".into(), "PRICE2".into());
    let rec = record("dvd");
    let jan = month("2024-01");
    let first = block(1, None, &[T, R, P]);
    hist.block("DISPATCHPRICE", jan, ("A.zip", 0), &rec, &first);
    hist.block("DISPATCHPRICE", jan, ("A.zip", 1), &rec, &other);
    let h = hist.finish(&decl(), &BTreeMap::new()).unwrap();
    let h = &h["DISPATCHPRICE"];
    assert_eq!(h.runs[1].changes, [Change::Report]);
    assert_eq!(h.runs[0].from.data_time, None);
}

#[test]
fn observations_go_in_data_order_and_overlaps_are_marked() {
    let mut h = Observations::default();
    let rec = record("dvd");
    let feb = month("2024-02");
    // Taken out of order: the later data first.
    let b = block(
        1,
        Some(("2024/02/01 00:05:00", "2024/02/29 00:00:00")),
        &[T, R, X, P],
    );
    h.block("DISPATCHPRICE", feb, ("B.zip", 0), &rec, &b);
    let a = block(
        1,
        Some(("2024/01/01 00:05:00", "2024/02/10 00:00:00")),
        &[T, R, P],
    );
    h.block("DISPATCHPRICE", month("2024-01"), ("A.zip", 0), &rec, &a);
    // A report run of B's shape, of another kind.
    let r = block(
        1,
        Some(("2024/02/05 00:05:00", "2024/03/01 00:00:00")),
        &[T, R, X, P],
    );
    let reports = record("reports/current");
    let csvs = (("PUBLIC_1.zip", "f1"), ("PUBLIC_9.CSV", "f9"), 9);
    h.run("DISPATCHPRICE", feb, csvs, &reports, &r);
    let t = h.finish(&decl(), &BTreeMap::new()).unwrap();
    let t = &t["DISPATCHPRICE"];
    assert_eq!(t.runs[0].from.file, "A.zip");
    assert_eq!(t.runs[1].from.file, "B.zip");
    assert!(t.runs[1].overlaps, "B starts before A's data ends");
    // The report run has B's schema: one run of 10 files of two kinds.
    assert_eq!(t.runs.len(), 2);
    assert_eq!(t.runs[1].files, 10);
    assert_eq!(t.runs[1].kinds, ["dvd", "reports/current"]);
    assert_eq!(
        (t.runs[1].to.file.as_str(), t.runs[1].to.sha256.as_str()),
        ("PUBLIC_9.CSV", "f9"),
        "the report run ends last, with its file's hash"
    );
    assert_eq!(t.runs[1].to.release, "2024-02");
}

#[test]
fn a_run_ends_where_its_latest_data_ends() {
    let mut h = Observations::default();
    let rec = record("dvd");
    let jan = month("2024-01");
    let long = block(
        1,
        Some(("2024/01/01 00:05:00", "2024/03/01 00:00:00")),
        &[T, R, P],
    );
    let short = block(
        1,
        Some(("2024/01/02 00:05:00", "2024/01/03 00:00:00")),
        &[T, R, P],
    );
    h.block("DISPATCHPRICE", jan, ("A.zip", 0), &rec, &long);
    h.block("DISPATCHPRICE", jan, ("B.zip", 0), &rec, &short);
    let t = h.finish(&decl(), &BTreeMap::new()).unwrap();
    let t = &t["DISPATCHPRICE"];
    assert_eq!(t.runs.len(), 1);
    assert_eq!(t.runs[0].from.file, "B.zip", "B ends first, so it leads");
    assert_eq!(t.runs[0].to.file, "A.zip");
    assert_eq!(t.runs[0].rows, 20);
}

#[test]
fn the_widest_declaration_of_a_lineage_sets_its_type() {
    let mut d = decl();
    let release = |p, s| {
        BTreeMap::from([(
            "DISPATCHPRICE".to_owned(),
            Numeric::from([("RRP".to_owned(), (p, s))]),
        )])
    };
    d.releases = vec![
        (month("2024-02"), release(15, 5)),
        (month("2024-04"), release(18, 5)),
        (month("2024-06"), release(15, 5)),
    ];
    let mut h = Observations::default();
    for m in ["2024-01", "2024-03", "2024-05", "2024-07"] {
        let (start, end) = span_of(m);
        let b = block(1, Some((&start, &end)), &[T, R, P]);
        h.block("DISPATCHPRICE", month(m), (m, 0), &record("dvd"), &b);
    }
    let t = h.finish(&d, &BTreeMap::new()).unwrap();
    let rrp = t["DISPATCHPRICE"].schemas[0].by_name("RRP").unwrap().ty;
    assert_eq!(rrp, IcebergType::decimal(18, 5).unwrap());
    assert_eq!(t["DISPATCHPRICE"].runs[0].files, 4);
    // Before the first release, the first that declares the table.
    let first = d.numeric("DISPATCHPRICE", month("2024-01")).unwrap();
    assert_eq!(first["RRP"], (15, 5));
    assert_eq!(d.numeric("NONE", month("2024-01")), None);
    // A whole number the model declares whole, never seen: a long.
    let whole = declared(TypeClass::Unknown, Some((10, 0)), false);
    assert_eq!(choose(Evidence::default(), whole).ty, IcebergType::Long);
    assert_eq!(declared(TypeClass::Integer, None, false).fallback, None);
    let huge = declared(TypeClass::Number, Some((40, 2)), false);
    assert!(huge.fallback.is_none());
    // Declared whole, then with a fraction: no longer whole.
    let w = widest(
        declared(TypeClass::Integer, None, true),
        declared(TypeClass::Number, None, false),
    );
    assert!(!w.whole && w.fractional && w.key);
    assert!(ends(Kind::Integer, Kind::Text) && ends(Kind::Date, Kind::Decimal));
    assert!(!ends(Kind::Text, Kind::Integer) && !ends(Kind::Empty, Kind::Text));
    assert!(!ends(Kind::Integer, Kind::Empty) && !ends(Kind::Integer, Kind::Decimal));
}

#[test]
fn files_go_in_the_order_they_were_released_even_when_they_restate_the_past() {
    let lc = ("LASTCHANGED", Kind::Timestamp, 0, 0, 0);
    let mut h = Observations::default();
    // Monthly releases, each holding everything since January, then a
    // June release in a new layout that restates January only.
    let rec = record("archive");
    for (file, release, end, cols) in [
        ("JAN.zip", "2024-01", "2024/01/31 00:00:00", &[T, R, P][..]),
        (
            "BACKFILL.zip",
            "2024-06",
            "2024/01/31 00:00:00",
            &[T, R, P, lc][..],
        ),
        ("MAR.zip", "2024-03", "2024/03/31 00:00:00", &[T, R, P][..]),
        ("FEB.zip", "2024-02", "2024/02/28 00:00:00", &[T, R, P][..]),
    ] {
        let b = block(1, Some(("2024/01/01 00:05:00", end)), cols);
        h.block("DISPATCHPRICE", month(release), (file, 0), &rec, &b);
    }
    let t = h.finish(&decl(), &BTreeMap::new()).unwrap();
    let t = &t["DISPATCHPRICE"];
    let files: Vec<&str> = t.runs.iter().map(|r| r.from.file.as_str()).collect();
    assert_eq!(files, ["JAN.zip", "BACKFILL.zip"]);
    assert_eq!(
        (t.runs[0].files, t.runs[0].to.file.as_str()),
        (3, "MAR.zip")
    );
    let backfill = &t.runs[1];
    assert_eq!(
        backfill.from.data_time.as_deref(),
        Some("2024-01-01T00:05:00+10:00")
    );
    assert!(backfill.overlaps, "it restates data the run before holds");
}

#[test]
fn instants_are_seconds_since_1970() {
    let i = |s: &str| instant(Some(s));
    assert_eq!(i("1970-01-01T00:00:00Z"), Some(0));
    assert_eq!(i("2024-01-01T10:00:00+10:00"), i("2024-01-01T00:00:00Z"));
    assert_eq!(i("2024-01-01T00:00:00Z"), Some(1_704_067_200));
    assert_eq!(i("2024-02-29T23:59:59.5-01:30"), Some(1_709_251_199 + 5400));
    assert_eq!(i("2000-03-01T00:00:00Z"), Some(951_868_800));
    assert_eq!(i("1969-12-31T23:59:59Z"), Some(-1));
    for bad in [
        "not a time",
        "2024-01-01 00:00:00Z",
        "2024-01-01T00:00:00",
        "2024-01-01T00:00:00.Z",
        "2024-01-01T00:00:00+1000",
        "2024-01-01T00:00:00+24:00",
        "2024-01-01T00:00:00+10:60",
        "2024-01-01T00:00:00+1a:00",
        "2024-01-01T00:00:00Zz",
        "2023-02-29T00:00:00Z",
        "2100-02-29T00:00:00Z",
        "2024-13-01T00:00:00Z",
        "2024-00-01T00:00:00Z",
        "2024-01-00T00:00:00Z",
        "2024-01-01T24:00:00Z",
        "2024-01-01T00:60:00Z",
        "2024-01-01T00:00:60Z",
        "20x4-01-01T00:00:00Z",
        "2024-01-01T0",
    ] {
        assert_eq!(i(bad), None, "{bad}");
    }
    assert_eq!(i("2000-02-29T00:00:00Z"), Some(951_782_400));
    assert_eq!(instant(None), None);
}

// ── What a published history promises ──────────────────────────────────────

/// Folds `blocks[..k]` for each `k`, each kept to the one before, and
/// checks every promise held; returns the last.
fn chain(blocks: &[Month<'_>], d: &Declarations) -> History {
    let order: Vec<usize> = (0..blocks.len()).collect();
    chain_inserting(blocks, &order, d)
}

/// Folds the months `order[..k]` of `blocks` (in month order) for each
/// `k`, each kept to the one before, and checks every promise held;
/// returns the last. An order that is not the months' adds months before
/// and between those published.
fn chain_inserting(blocks: &[Month<'_>], order: &[usize], d: &Declarations) -> History {
    let taken = |k: usize| -> Vec<Month<'_>> {
        let mut ix: Vec<usize> = order[..k].to_vec();
        ix.sort_unstable();
        ix.iter().map(|&i| blocks[i]).collect()
    };
    let mut published: Option<History> = None;
    for k in 1..=order.len() {
        let now = fold_with(&taken(k), d, published.as_ref()).unwrap();
        if let Some(old) = &published {
            assert_eq!(check(old, &now), [], "after {k} month(s) of {order:?}");
        }
        // Without one, the fold of the same months as a baseline also
        // keeps it.
        if k > 1 {
            let plain = fold_with(&taken(k - 1), d, None).unwrap();
            let next = fold_with(&taken(k), d, Some(&plain)).unwrap();
            assert_eq!(
                check(&plain, &next),
                [],
                "plain, after {k} month(s) of {order:?}"
            );
        }
        published = Some(now);
    }
    published.unwrap()
}

/// A small generator of pseudo-random numbers (xorshift), so a failing
/// case can be run again by its seed.
struct Rng(u64);

impl Rng {
    fn below(&mut self, n: u64) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0 % n
    }
}

/// Whatever the months hold, each fold keeps what the one before it
/// published: columns that come and go, move, widen, take nulls, turn to
/// text, and versions and kinds that change.
#[test]
fn every_fold_keeps_what_the_fold_before_it_published() {
    const NAMES: [&str; 5] = ["RRP", "EEP", "ROP", "FLAG", "LASTCHANGED"];
    let kinds = [
        Kind::Empty,
        Kind::Integer,
        Kind::Decimal,
        Kind::Date,
        Kind::Timestamp,
        Kind::Text,
    ];
    for seed in 1..=60u64 {
        let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15));
        let mut months: Vec<(String, u16, &str, Vec<Col<'static>>)> = Vec::new();
        let mut m = month("2023-01");
        for _ in 0..8 {
            let mut cols = vec![
                T,
                if rng.below(8) == 0 {
                    ("REGIONID", Kind::Text, 0, 0, 1)
                } else {
                    R
                },
            ];
            for name in NAMES {
                if rng.below(4) != 0 {
                    // Mostly numbers, now and then anything.
                    let kind = if rng.below(6) == 0 {
                        kinds[usize::try_from(rng.below(6)).unwrap()]
                    } else {
                        Kind::Decimal
                    };
                    let (i, s) = match kind {
                        Kind::Integer | Kind::Decimal => (rng.below(12) as u8, rng.below(5) as u8),
                        _ => (0, 0),
                    };
                    cols.push((name, kind, i, s, rng.below(3)));
                }
            }
            if rng.below(3) == 0 {
                let last = cols.len() - 1;
                cols.swap(1, last);
            }
            let version = 1 + u16::from(rng.below(5) == 0);
            let kind = if rng.below(2) == 0 { "dvd" } else { "archive" };
            months.push((m.to_string(), version, kind, cols));
            m = m.next();
        }
        let blocks: Vec<Month<'_>> = months
            .iter()
            .map(|(m, v, k, c)| (m.as_str(), *v, *k, c.as_slice()))
            .collect();
        chain(&blocks, &decl());
        // And in any order: months before and between those published.
        let mut order: Vec<usize> = (0..blocks.len()).collect();
        for i in (1..order.len()).rev() {
            let j = usize::try_from(rng.below(i as u64 + 1)).unwrap();
            order.swap(i, j);
        }
        chain_inserting(&blocks, &order, &decl());
    }
}

// ── Evidence from before and between what was published ────────────────────

/// Months published, then a month before them: every field and schema
/// keeps its id, the published first run is kept whole after the new one.
#[test]
fn a_backfilled_month_keeps_every_published_id() {
    let later: [Month<'_>; 3] = [
        ("2024-02", 1, "dvd", &[T, R, P]),
        ("2024-03", 1, "dvd", &[T, R, P]),
        ("2024-04", 1, "dvd", &[T, R, P]),
    ];
    let published = months(&later);
    let mut all = vec![("2024-01", 1, "dvd", &[T, R, P][..])];
    all.extend_from_slice(&later);
    let now = fold_with(&all, &decl(), Some(&published)).unwrap();
    assert_eq!(check(&published, &now), []);
    assert_eq!(now.schemas, published.schemas);
    assert_eq!(now.fields(), published.fields());
    assert_eq!(schemas_of(&now), [0, 0]);
    assert_eq!(now.runs[0].changes, [Change::First]);
    assert_eq!(now.runs[1].changes, [Change::Published]);
    assert_eq!(now.runs[1].from.file, "2024-02.zip");
    assert_eq!((now.runs[0].files, now.runs[1].files), (1, 3));
    assert!(now.breaking.is_empty());
    assert_eq!(now.current_schema_id, 0);
    // The same fold, from scratch, is one run: the boundary is only kept.
    assert_eq!(months(&all).runs.len(), 1);
    // And another month later still grows the published run.
    let mut more = all.clone();
    more.push(("2024-05", 1, "dvd", &[T, R, P]));
    let later = fold_with(&more, &decl(), Some(&now)).unwrap();
    assert_eq!(check(&now, &later), []);
    assert_eq!(later.runs[1].files, 4);
}

/// A month before the published ones whose values the published field
/// cannot hold is a new field, which breaks into the published one where
/// that begins; values it holds continue the published field back in
/// time.
#[test]
fn a_lineage_before_a_published_one_takes_a_new_id() {
    let later: [Month<'_>; 2] = [
        ("2024-03", 1, "dvd", &[T, R, P]),
        ("2024-04", 1, "dvd", &[T, R, P]),
    ];
    let published = months(&later);
    let five_two = IcebergType::decimal(5, 2).unwrap();
    assert_eq!(published.schemas[0].by_name("RRP").unwrap().ty, five_two);
    // Text before: its own field, string, breaking into field 3.
    let text = [T, R, ("RRP", Kind::Text, 0, 0, 0)];
    let mut all = vec![("2024-02", 1, "dvd", &text[..])];
    all.extend_from_slice(&later);
    let now = fold_with(&all, &decl(), Some(&published)).unwrap();
    assert_eq!(check(&published, &now), []);
    assert_eq!(schemas_of(&now), [1, 0]);
    assert_eq!(
        now.schemas[1].by_name("RRP").map(|f| (f.id, f.ty)),
        Some((4, IcebergType::String))
    );
    let b = &now.breaking[0];
    assert_eq!((b.old_field_id, b.new_field_id), (4, 3));
    assert_eq!(b.at.file, "2024-03.zip");
    assert_eq!(
        now.runs[1].updates,
        [Update::Delete { field_id: 4 }, add(3)]
    );
    // Narrower numbers before: the published field, back in time.
    let narrow = [T, R, ("RRP", Kind::Decimal, 2, 1, 0)];
    all[0] = ("2024-02", 1, "dvd", &narrow);
    let now = fold_with(&all, &decl(), Some(&published)).unwrap();
    assert_eq!(check(&published, &now), []);
    assert_eq!(now.schemas, published.schemas);
    assert_eq!(schemas_of(&now), [0, 0]);
    assert!(now.breaking.is_empty());
    // Wider numbers before: a new field, as text was.
    let wide = [T, R, ("RRP", Kind::Decimal, 9, 2, 0)];
    all[0] = ("2024-02", 1, "dvd", &wide);
    let now = fold_with(&all, &decl(), Some(&published)).unwrap();
    assert_eq!(check(&published, &now), []);
    assert_eq!(
        now.schemas[1].by_name("RRP").map(|f| (f.id, f.ty)),
        Some((4, IcebergType::decimal(11, 2).unwrap()))
    );
}

/// A key column null in a month before the published ones is its own
/// optional field there: the published field stays required.
#[test]
fn a_key_column_null_in_a_backfill_is_its_own_field() {
    let later: [Month<'_>; 2] = [
        ("2024-03", 1, "dvd", &[T, R, P]),
        ("2024-04", 1, "dvd", &[T, R, P]),
    ];
    let published = months(&later);
    assert!(published.schemas[0].by_name("REGIONID").unwrap().required);
    let nulled = [T, ("REGIONID", Kind::Text, 0, 0, 1), P];
    let mut all = vec![("2024-02", 1, "dvd", &nulled[..])];
    all.extend_from_slice(&later);
    let now = fold_with(&all, &decl(), Some(&published)).unwrap();
    assert_eq!(check(&published, &now), []);
    assert_eq!(now.schemas[0], published.schemas[0]);
    let region = now.schemas[1].by_name("REGIONID").unwrap();
    assert_eq!((region.id, region.required), (4, false));
    assert_eq!(now.schemas[1].identifier_field_ids, [1]);
    assert_eq!(schemas_of(&now), [1, 0]);
    assert_eq!(
        (now.breaking[0].old_field_id, now.breaking[0].new_field_id),
        (4, 2)
    );
}

/// Report files found later inside a published run, in a month between
/// its files: the same values join the run; values the field cannot hold
/// split it around a new field, and every published file keeps its
/// schema.
#[test]
fn a_report_run_inside_a_published_run_keeps_its_schema() {
    let blocks: [Month<'_>; 2] = [
        ("2024-02", 1, "dvd", &[T, R, P]),
        ("2024-04", 1, "dvd", &[T, R, P]),
    ];
    let published = months(&blocks);
    let d = decl();
    let floor = BTreeMap::from([("DISPATCHPRICE".to_owned(), published.clone())]);
    let with = |cols: &[Col<'_>]| {
        let mut obs = observe(&blocks);
        report_run(&mut obs, "2024-03", cols);
        obs.finish(&d, &floor)
            .map(|mut t| t.remove("DISPATCHPRICE").unwrap())
            .unwrap()
    };
    let same = with(&[T, R, P]);
    assert_eq!(check(&published, &same), []);
    assert_eq!(schemas_of(&same), [0]);
    assert_eq!(same.runs[0].files, 4);
    assert_eq!(same.runs[0].kinds, ["dvd", "reports/archive"]);
    let wide = with(&[T, R, ("RRP", Kind::Decimal, 9, 2, 0)]);
    assert_eq!(check(&published, &wide), []);
    assert_eq!(schemas_of(&wide), [0, 1, 0]);
    assert_eq!(wide.runs[1].from.file, "R2024-03a.csv");
    assert_eq!(wide.runs[1].kinds, ["reports/archive"]);
    let breaks: Vec<(u32, u32, &str)> = wide
        .breaking
        .iter()
        .map(|b| (b.old_field_id, b.new_field_id, b.at.file.as_str()))
        .collect();
    assert_eq!(breaks, [(3, 4, "R2024-03a.csv"), (4, 3, "2024-04.zip")]);
    assert_eq!(wide.runs[2].changes, [Change::Schema]);
    assert_eq!(
        wide.runs[2].updates,
        [
            Update::Delete { field_id: 4 },
            Update::Restore { field_id: 3 }
        ]
    );
}

/// One report zip whose blocks change shape part way through its month
/// ends one run and begins the next at the same file: the fold of the
/// same evidence again keeps both runs whole, and more files at the end
/// of the second block extend it.
#[test]
fn a_file_that_ends_one_run_and_begins_the_next_marks_each_by_its_time() {
    let d = decl();
    let evidence = |later: u64| {
        let mut obs = observe(&[("2024-04", 1, "dvd", &[T, R, P])]);
        let file = ("R2024-05.zip", "cd");
        let early = ("2024/05/01 00:05:00", "2024/05/14 00:00:00");
        let late = ("2024/05/14 00:05:00", "2024/05/28 00:00:00");
        let current = record("reports/current");
        obs.run(
            "DISPATCHPRICE",
            month("2024-05"),
            (file, file, 13),
            &current,
            &block(1, Some(early), &[T, R, P]),
        );
        obs.run(
            "DISPATCHPRICE",
            month("2024-05"),
            (file, file, later),
            &current,
            &block(1, Some(late), &[T, R, X, P]),
        );
        obs
    };
    let fold = |obs: Observations, floor: &BTreeMap<String, History>| {
        obs.finish(&d, floor)
            .map(|mut t| t.remove("DISPATCHPRICE").unwrap())
            .unwrap()
    };
    let published = fold(evidence(17), &BTreeMap::new());
    assert_eq!(schemas_of(&published), [0, 1]);
    assert_eq!(published.runs[0].files, 14);
    assert_eq!(published.runs[0].to.file, "R2024-05.zip");
    assert_eq!(published.runs[1].from.file, "R2024-05.zip");
    let floor = BTreeMap::from([("DISPATCHPRICE".to_owned(), published.clone())]);
    let again = fold(evidence(17), &floor);
    assert_eq!(check(&published, &again), []);
    assert_eq!(again.runs, published.runs);
    let grown = fold(evidence(20), &floor);
    assert_eq!(check(&published, &grown), []);
    assert_eq!(schemas_of(&grown), [0, 1]);
    assert_eq!((grown.runs[0].files, grown.runs[1].files), (14, 20));
}

/// Two column lists that alternate keep their two schemas and ids when a
/// month of either is added before them.
#[test]
fn schemas_that_alternate_keep_their_ids_under_backfill() {
    let a: &[Col<'_>] = &[T, R, P];
    let b: &[Col<'_>] = &[T, R, X, P];
    let later: [Month<'_>; 4] = [
        ("2024-03", 1, "dvd", a),
        ("2024-04", 1, "dvd", b),
        ("2024-05", 1, "dvd", a),
        ("2024-06", 1, "dvd", b),
    ];
    let published = months(&later);
    assert_eq!(schemas_of(&published), [0, 1, 0, 1]);
    for (cols, runs) in [(b, [1, 0, 1, 0, 1]), (a, [0, 0, 1, 0, 1])] {
        let mut all = vec![("2024-02", 1, "dvd", cols)];
        all.extend_from_slice(&later);
        let now = fold_with(&all, &decl(), Some(&published)).unwrap();
        assert_eq!(check(&published, &now), []);
        assert_eq!(now.schemas, published.schemas);
        assert_eq!(schemas_of(&now), runs);
        assert!(now.breaking.is_empty());
    }
}

/// A month whose decimal is wider than the published type is a new field
/// from that month, never a retype; with nothing published, the one field
/// is as wide as everything.
#[test]
fn a_decimal_wider_than_published_splits_the_column() {
    let wide = ("RRP", Kind::Decimal, 9, 2, 0);
    let blocks: [Month<'_>; 3] = [
        ("2024-01", 1, "dvd", &[T, R, P]),
        ("2024-02", 1, "dvd", &[T, R, P]),
        ("2024-03", 1, "dvd", &[T, R, wide]),
    ];
    let published = months(&blocks[..2]);
    let five_two = IcebergType::decimal(5, 2).unwrap();
    assert_eq!(published.schemas[0].by_name("RRP").unwrap().ty, five_two);
    let now = fold_with(&blocks, &decl(), Some(&published)).unwrap();
    assert_eq!(check(&published, &now), []);
    assert_eq!(now.breaking.len(), 1);
    let b = &now.breaking[0];
    assert_eq!(b.column, "RRP");
    assert_eq!((b.old_field_id, b.old_type.as_str()), (3, "decimal(5, 2)"));
    assert_eq!((b.new_field_id, b.new_type.as_str()), (4, "decimal(11, 2)"));
    assert_eq!(
        (b.at.file.as_str(), b.at.release.as_str()),
        ("2024-03.zip", "2024-03")
    );
    assert_eq!(b.at.data_time.as_deref(), Some("2024-03-01T00:05:00+10:00"));
    assert_eq!(schemas_of(&now), [0, 1]);
    assert_eq!(
        now.runs[1].updates,
        [Update::Delete { field_id: 3 }, add(4)]
    );
    // A later month the published type holds again stays on the new field.
    let mut more = blocks.to_vec();
    more.push(("2024-04", 1, "dvd", &[T, R, P]));
    let later = fold_with(&more, &decl(), Some(&now)).unwrap();
    assert_eq!(check(&now, &later), []);
    assert_eq!(later.runs.len(), 2);
    // Without a baseline, exactly utilake's fold: one field, as wide as
    // everything.
    let plain = months(&blocks);
    assert!(plain.breaking.is_empty());
    let eleven = IcebergType::decimal(11, 2).unwrap();
    assert_eq!(plain.schemas[0].by_name("RRP").unwrap().ty, eleven);
    assert_eq!(chain(&blocks, &decl()), now);
}

/// A published history the census no longer gives is an error, not a
/// silent change: another type for a field, a field gone, a table gone.
#[test]
fn a_fold_that_cannot_keep_the_promise_is_an_error() {
    let blocks: [Month<'_>; 2] = [
        ("2024-01", 1, "dvd", &[T, R, P]),
        ("2024-02", 1, "dvd", &[T, R, P]),
    ];
    // Published under another name: no field of that name covers the
    // column, so it is a new one, and the run begins another schema.
    let mut renamed = months(&blocks);
    for s in &mut renamed.schemas {
        s.fields[2].name = "PRICE".into();
    }
    let e = fold_with(&blocks, &decl(), Some(&renamed)).unwrap_err();
    assert_eq!(
        e.to_string(),
        "the history of DISPATCHPRICE breaks what nem/DISPATCHPRICE/table.json published: run 0 has changed"
    );
    // Published with a type the census's own values do not fit: the
    // field cannot be kept, and a new one is minted at once.
    let mut narrow = months(&blocks);
    for s in &mut narrow.schemas {
        s.fields[2].ty = IcebergType::Long;
    }
    let e = fold_with(&blocks, &decl(), Some(&narrow)).unwrap_err();
    assert!(e.to_string().ends_with("run 0 has changed"), "{e}");
    // Published from a file the census no longer has.
    let mut moved = months(&blocks);
    moved.runs[0].from.file = "Z.zip".into();
    let e = fold_with(&blocks, &decl(), Some(&moved)).unwrap_err();
    assert!(
        e.to_string().ends_with("run 0's first file begins no run"),
        "{e}"
    );
    // Published to a file after every observation of it: the first alone.
    let mut cut = months(&blocks);
    cut.runs[0].to = cut.runs[0].from.clone();
    cut.runs[0].to.file = "Z.zip".into();
    let now = fold_with(&blocks, &decl(), Some(&cut)).unwrap();
    assert_eq!(check(&cut, &now), []);
    // A table published that no census holds.
    let mut gone = months(&blocks);
    gone.table = "GONE".into();
    let published = BTreeMap::from([("GONE".to_owned(), gone)]);
    let e = observe(&blocks).finish(&decl(), &published).unwrap_err();
    assert!(
        matches!(e, HistoryError::Broken { ref table, .. } if table == "GONE"),
        "{e}"
    );
}

// ── Overrides ───────────────────────────────────────────────────────────────

fn overridden(column: &str, ty: IcebergType) -> Declarations {
    let mut d = decl();
    d.overrides = BTreeMap::from([(
        "DISPATCHPRICE".to_owned(),
        BTreeMap::from([(column.to_owned(), ty)]),
    )]);
    d
}

#[test]
fn an_override_types_the_column_and_must_hold_its_values() {
    let text = ("RRP", Kind::Text, 0, 0, 0);
    let blocks: [Month<'_>; 3] = [
        ("2024-01", 1, "dvd", &[T, R, P]),
        ("2024-02", 1, "dvd", &[T, R, text]),
        ("2024-03", 1, "dvd", &[T, R, P]),
    ];
    // Text everywhere: one field, no break where the values turn.
    let h = fold_with(&blocks, &overridden("RRP", IcebergType::String), None).unwrap();
    assert!(h.breaking.is_empty());
    assert_eq!(schemas_of(&h), [0]);
    assert_eq!(h.schemas[0].by_name("RRP").unwrap().ty, IcebergType::String);
    // A decimal wider than the values.
    let d16 = IcebergType::decimal(16, 6).unwrap();
    let h = fold_with(&blocks[..1], &overridden("RRP", d16), None).unwrap();
    assert_eq!(h.schemas[0].by_name("RRP").unwrap().ty, d16);
    // A type that cannot hold them names the table, column and file.
    let e = fold_with(&blocks, &overridden("RRP", d16), None).unwrap_err();
    assert_eq!(
        e.to_string(),
        "the override of RRP in DISPATCHPRICE as decimal(16, 6) cannot hold the values of 2024-02.zip; tables/DISPATCHPRICE.toml needs another type"
    );
    // A published field keeps its type; the override types the field that
    // follows it once the published type no longer holds the values.
    let published = months(&blocks[..1]);
    let now = fold_with(
        &blocks,
        &overridden("RRP", IcebergType::String),
        Some(&published),
    )
    .unwrap();
    assert_eq!(check(&published, &now), []);
    let b = &now.breaking[0];
    assert_eq!(
        (b.old_type.as_str(), b.new_type.as_str()),
        ("decimal(5, 2)", "string")
    );
    assert_eq!(b.at.file, "2024-02.zip");
    let e = fold_with(
        &blocks,
        &overridden("RRP", IcebergType::Long),
        Some(&published),
    );
    assert!(matches!(e, Err(HistoryError::Override { .. })));
}

#[test]
fn every_block_resolves_to_its_runs_fields() {
    // A key column that takes a null later in the month makes a schema
    // with the same names and fields: the block before still resolves to
    // fields it reads the same with.
    let nulled: Col<'static> = ("REGIONID", Kind::Text, 0, 0, 1);
    let h = months(&[
        ("2024-06", 1, "dvd", &[T, R, P]),
        ("2024-06", 1, "dvd", &[T, nulled, P]),
    ]);
    assert_eq!(schemas_of(&h), [0, 1]);
    // A column that turns to text within the month is a new field under
    // the same names: the block before it would be read with that field.
    let text: Col<'static> = ("RRP", Kind::Text, 0, 0, 0);
    let e = fold_with(
        &[
            ("2024-06", 1, "dvd", &[T, R, P]),
            ("2024-06", 1, "dvd", &[T, R, text]),
        ],
        &decl(),
        None,
    )
    .unwrap_err();
    assert!(matches!(
        &e,
        HistoryError::Unresolved { file, schema: 0, resolved: Some(1), .. } if file == "2024-06.zip"
    ));
    assert!(
        e.to_string()
            .contains("resolve to schema 1, with other fields")
    );
    let none = HistoryError::Unresolved {
        table: "T".into(),
        file: "F".into(),
        schema: 0,
        resolved: None,
    };
    assert!(none.to_string().contains("resolve to no schema"));
}

#[test]
fn a_table_is_a_day_label_only_when_every_block_that_says_so_agrees() {
    let mut h = Observations::default();
    let rec = record("dvd");
    let mut b = block(1, Some(("2024/01/01", "2024/01/31")), &[T, R, P]);
    assert_eq!(h.midnight_only("DISPATCHPRICE"), None, "nothing observed");
    // A block from before the census kept it says nothing.
    b.midnight = None;
    h.block("DISPATCHPRICE", month("2024-01"), ("A.zip", 0), &rec, &b);
    assert_eq!(h.midnight_only("DISPATCHPRICE"), None);
    b.midnight = Some(true);
    h.block("DISPATCHPRICE", month("2024-02"), ("B.zip", 0), &rec, &b);
    assert_eq!(h.midnight_only("DISPATCHPRICE"), Some(true));
    b.midnight = Some(false);
    h.block("DISPATCHPRICE", month("2024-03"), ("C.zip", 0), &rec, &b);
    assert_eq!(
        h.midnight_only("DISPATCHPRICE"),
        Some(false),
        "one time of day is enough"
    );
}
