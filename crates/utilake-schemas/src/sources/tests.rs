use super::*;
use crate::census::tests::put;
use crate::census::{BlockRecord, FileRecord, census_path};
use crate::reports::{FolderCensus, Rewritten, Tally, Version, nem_reports, record_folder};
use crate::shape::ShapeId;

/// The MMSDM directory of the archive's host.
const MMSDM: &str = "www.nemweb.com.au/Data_Archive/Wholesale_Electricity/MMSDM";

/// Records the census of `month`: per file name, its source, sha256 and
/// size.
fn census(root: &Path, month: &str, files: &[(&str, &str, &str, u64)]) {
    let files: BTreeMap<&str, FileRecord> = files
        .iter()
        .map(|&(name, source, sha256, size)| {
            let r = FileRecord {
                source: source.into(),
                sha256: sha256.into(),
                size,
                kind: "dvd".into(),
                bad_rows: 0,
                footer_mismatches: 0,
                block: Vec::new(),
            };
            (name, r)
        })
        .collect();
    let path = census_path(root, Market::Nem, month.parse().unwrap());
    crate::files::write_atomic(&path, toml::to_string(&files).unwrap().as_bytes()).unwrap();
}

/// A sidecar's JSON with these fields after its URL, size and sha256.
fn sidecar_json(url: &str, size: u64, sha256: &str, rest: &str) -> String {
    format!(
        r#"{{"url":"{url}","size":{size},"sha256":"{sha256}","fetched_at":"2026-10-17T23:38:12Z"{rest},"tool":"t"}}"#
    )
}

/// Writes the file `rel` of the archive with `size` bytes, and its sidecar
/// unless `sidecar` is `None`.
fn file(archive: &Path, rel: &str, size: u64, sidecar: Option<String>) {
    put(archive, rel, &vec![b'x'; usize::try_from(size).unwrap()]);
    if let Some(json) = sidecar {
        put(archive, &format!("{rel}.json"), json.as_bytes());
    }
}

const NEW: &str =
    r#","last_modified":"Mon, 18 May 2026 07:27:48 GMT","last_modified_at":"2026-05-18T07:27:48Z""#;

fn url(rel: &str) -> String {
    format!("https://{rel}")
}

/// A census of 2026-04 (a directory month) and 2009-07 (one zip), and an
/// archive that holds both with sidecars that agree.
fn agreeing() -> (tempfile::TempDir, tempfile::TempDir) {
    let (repo, archive) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let (root, a) = (repo.path(), archive.path());
    census(
        root,
        "2026-04",
        &[("A.zip", "A.zip", "aa", 3), ("B.CTL", "B.CTL", "bb", 2)],
    );
    census(
        root,
        "2009-07",
        &[
            ("X.zip", "MMSDM_2009_07.zip", "mm", 5),
            ("Y.zip", "MMSDM_2009_07.zip", "mm", 5),
        ],
    );
    let dir = format!("{MMSDM}/2026/MMSDM_2026_04/MMSDM_Historical_Data_SQLLoader");
    for (rel, size, sha) in [
        (format!("{dir}/DATA/A.zip"), 3, "aa"),
        (format!("{dir}/CTL/B.CTL"), 2, "bb"),
    ] {
        file(
            a,
            &rel,
            size,
            Some(sidecar_json(&url(&rel), size, sha, NEW)),
        );
    }
    // A hidden file is not a file of the month.
    put(a, &format!("{dir}/DATA/._A.zip"), b"junk");
    let zip = format!("{MMSDM}/2009/MMSDM_2009_07.zip");
    // An older sidecar: the server's time as text only.
    let old = r#","last_modified":"Wed, 15 Apr 2026 04:29:11 GMT""#;
    file(a, &zip, 5, Some(sidecar_json(&url(&zip), 5, "mm", old)));
    (repo, archive)
}

#[test]
fn every_census_file_is_listed_from_its_sidecar() {
    let (repo, archive) = agreeing();
    let (root, a) = (repo.path(), archive.path());
    let s = collect(root, Market::Nem, a, 2).unwrap();
    assert_eq!(s.mismatches, Vec::new());
    assert_eq!(s.files(), 3, "the whole-month zip once");
    assert_eq!(write(root, Market::Nem, &s).unwrap(), 2);
    let read =
        |m: &str| std::fs::read_to_string(root.join(format!("sources/nem/{m}.tsv"))).unwrap();
    let d = format!("https://{MMSDM}/2026/MMSDM_2026_04/MMSDM_Historical_Data_SQLLoader");
    assert_eq!(
        read("2026-04"),
        format!(
            "url\tsha256\tsize\tlast_modified\n{d}/CTL/B.CTL\tbb\t2\tMon, 18 May 2026 07:27:48 GMT\n{d}/DATA/A.zip\taa\t3\tMon, 18 May 2026 07:27:48 GMT\n"
        )
    );
    assert_eq!(
        read("2009-07"),
        format!(
            "url\tsha256\tsize\tlast_modified\nhttps://{MMSDM}/2009/MMSDM_2009_07.zip\tmm\t5\tWed, 15 Apr 2026 04:29:11 GMT\n"
        )
    );
    // What `fetch` reads back is what was written.
    let back =
        crate::fetch::read_sources(&sources_path(root, Market::Nem, "2009-07".parse().unwrap()))
            .unwrap();
    assert_eq!(back, s.months[&"2009-07".parse().unwrap()]);
    // The archive's host directory may be given itself; the same again
    // writes nothing.
    let again = collect(root, Market::Nem, &a.join("www.nemweb.com.au"), 1).unwrap();
    assert_eq!(again, s);
    assert_eq!(stale(root, Market::Nem, &again), Vec::<PathBuf>::new());
    assert_eq!(write(root, Market::Nem, &again).unwrap(), 0);
}

#[test]
fn an_older_sidecar_gives_its_times_from_the_text() {
    let (repo, archive) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let (root, a) = (repo.path(), archive.path());
    census(
        root,
        "2026-05",
        &[
            ("T.zip", "T.zip", "t1", 1),
            ("I.zip", "I.zip", "i1", 1),
            ("N.zip", "N.zip", "n1", 1),
            ("E.zip", "E.zip", "e1", 1),
        ],
    );
    let dir = format!("{MMSDM}/2026/MMSDM_2026_05");
    let sc = |n: &str, sha: &str, rest: &str| {
        let rel = format!("{dir}/{n}");
        file(a, &rel, 1, Some(sidecar_json(&url(&rel), 1, sha, rest)));
    };
    // The text as sent; an instant alone; neither; text that is empty.
    sc(
        "T.zip",
        "t1",
        r#","last_modified":"Mon, 18 May 2026 07:27:50 GMT","listed_modified":"Monday, May 18, 2026 05:27 PM""#,
    );
    sc(
        "I.zip",
        "i1",
        r#","last_modified_at":"2026-05-18T08:27:50+01:00""#,
    );
    sc("N.zip", "n1", "");
    sc("E.zip", "e1", r#","last_modified":"""#);
    let s = collect(root, Market::Nem, a, 1).unwrap();
    assert_eq!(s.mismatches, Vec::new());
    let rows = &s.months[&"2026-05".parse().unwrap()];
    let at = |n: &str| rows[&url(&format!("{dir}/{n}"))].last_modified.clone();
    assert_eq!(
        at("T.zip").as_deref(),
        Some("Mon, 18 May 2026 07:27:50 GMT")
    );
    assert_eq!(at("I.zip").as_deref(), Some("2026-05-18T08:27:50+01:00"));
    assert_eq!(at("N.zip"), None);
    assert_eq!(at("E.zip"), None);
}

#[test]
fn every_mismatch_is_named_and_nothing_is_written() {
    let (repo, archive) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let (root, a) = (repo.path(), archive.path());
    census(
        root,
        "2026-06",
        &[
            ("GONE.zip", "GONE.zip", "g", 1),
            ("BARE.zip", "BARE.zip", "b", 1),
            ("SHA.zip", "SHA.zip", "right", 1),
            ("SIZE.zip", "SIZE.zip", "s", 4),
            ("BAD.zip", "BAD.zip", "d", 1),
            ("TWO.zip", "MMSDM_2026_06.zip", "one", 1),
            ("TWO2.zip", "MMSDM_2026_06.zip", "two", 1),
            ("DUP.zip", "DUP.zip", "good", 1),
        ],
    );
    let dir = format!("{MMSDM}/2026/MMSDM_2026_06");
    let sc = |rel: &str, sha: &str| Some(sidecar_json(&url(rel), 1, sha, NEW));
    file(a, &format!("{dir}/BARE.zip"), 1, None);
    file(
        a,
        &format!("{dir}/SHA.zip"),
        1,
        sc(&format!("{dir}/SHA.zip"), "wrong"),
    );
    file(
        a,
        &format!("{dir}/SIZE.zip"),
        1,
        sc(&format!("{dir}/SIZE.zip"), "s"),
    );
    file(
        a,
        &format!("{dir}/BAD.zip"),
        1,
        Some(r#"{"url":"u"}"#.into()),
    );
    let zip = format!("{MMSDM}/2026/MMSDM_2026_06.zip");
    file(a, &zip, 1, sc(&zip, "one"));
    // A name in two places: the one whose sidecar agrees is taken.
    for (sub, sha) in [("A", "stale"), ("B", "good")] {
        let rel = format!("{dir}/{sub}/DUP.zip");
        file(a, &rel, 1, sc(&rel, sha));
    }
    let s = collect(root, Market::Nem, a, 1).unwrap();
    let shown: Vec<String> = s.mismatches.iter().map(ToString::to_string).collect();
    assert_eq!(
        shown,
        [
            "2026-06/MMSDM_2026_06.zip: the census gives it two SHA-256s or sizes",
            "2026-06/BAD.zip: its sidecar does not read",
            "2026-06/BARE.zip: the archive has no sidecar for it",
            "2026-06/GONE.zip: the archive does not have it",
            "2026-06/SHA.zip: its sidecar has SHA-256 wrong, the census right",
            "2026-06/SIZE.zip: its sidecar has size 1, the census 4",
        ]
    );
    let rows = &s.months[&"2026-06".parse().unwrap()];
    assert_eq!(
        rows.keys().cloned().collect::<Vec<_>>(),
        [url(&zip), url(&format!("{dir}/B/DUP.zip"))]
    );
}

#[test]
fn check_lists_each_file_that_would_change() {
    let (repo, archive) = agreeing();
    let (root, a) = (repo.path(), archive.path());
    let s = collect(root, Market::Nem, a, 1).unwrap();
    let all = vec![
        sources_path(root, Market::Nem, "2009-07".parse().unwrap()),
        sources_path(root, Market::Nem, "2026-04".parse().unwrap()),
    ];
    assert_eq!(stale(root, Market::Nem, &s), all, "none written yet");
    write(root, Market::Nem, &s).unwrap();
    // A row changed by hand, and a sources file of a month no census has,
    // which is left alone.
    let path = &all[1];
    let text = std::fs::read_to_string(path)
        .unwrap()
        .replace("\taa\t", "\tzz\t");
    std::fs::write(path, text).unwrap();
    std::fs::write(root.join("sources/nem/2001-01.tsv"), "kept").unwrap();
    assert_eq!(stale(root, Market::Nem, &s), vec![all[1].clone()]);
    assert_eq!(write(root, Market::Nem, &s).unwrap(), 1);
    assert_eq!(stale(root, Market::Nem, &s), Vec::<PathBuf>::new());
    assert_eq!(
        std::fs::read_to_string(root.join("sources/nem/2001-01.tsv")).unwrap(),
        "kept"
    );
}

fn record(report: &str) -> BlockRecord {
    BlockRecord {
        shape: ShapeId(7),
        report: ("DISPATCH".into(), report.into()),
        version: 1,
        rows: 1,
        rows_repeated: 0,
        headers: Vec::new(),
        time: None,
        first: None,
        last: None,
        midnight: None,
        columns: vec!["A".into()],
        ctl: "u".into(),
        kinds: "s".into(),
        nulls: vec![0],
        int_digits: vec![0],
        scale: vec![0],
        sig_digits: vec![0],
        max_len: vec![1],
        min: vec![String::new()],
        max: vec![String::new()],
    }
}

fn tally(first: (&str, &str), last: (&str, &str)) -> Tally {
    Tally {
        month: "2026-08".parse().unwrap(),
        csvs: 2,
        first_csv: "F.CSV".into(),
        last_csv: "L.CSV".into(),
        first_file: first.0.into(),
        first_sha256: first.1.into(),
        last_file: last.0.into(),
        last_sha256: last.1.into(),
        block: record("SCADA"),
    }
}

fn version(file: &str, sha256: &str) -> Version {
    Version {
        file: file.into(),
        change: crate::reports::Change::First,
        same_data_as: None,
        listed_at: None,
        published_at: None,
        sha256: sha256.into(),
    }
}

#[test]
fn report_folders_list_their_runs_and_versions_by_month() {
    let (repo, archive) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let (root, a) = (repo.path(), archive.path());
    let host = "www.nemweb.com.au/Reports";
    // A folder at the root of ARCHIVE whose run names files from before
    // they were hashed, and a nested one of CURRENT.
    let old = FolderCensus {
        blocks: vec![tally(("", ""), ("R_20260801.zip", "r"))],
        ..FolderCensus::default()
    };
    record_folder(root, Market::Nem, (nem_reports(), ""), &old).unwrap();
    let gas = FolderCensus {
        blocks: vec![
            tally(("P_20260801.zip", "p1"), ("P_20260902.zip", "p2")),
            tally(("P_20260902.zip", "other"), ("", "")),
        ],
        rewritten: vec![Rewritten {
            file: "int.csv".into(),
            relisted: 0,
            repacked: 0,
            versions: vec![
                version("int.csv", "v1"),
                version("int.csv@20261021T191200+1000", "v2"),
                version("gone.csv", "g"),
            ],
        }],
        ..FolderCensus::default()
    };
    record_folder(root, Market::Nem, (nem_reports(), "VicGas/sub"), &gas).unwrap();
    // Kept in one folder per report folder; each sidecar names the listing
    // the file came from.
    let sc = |listing: &str, rel: &str, sha: &str, rest: &str| {
        let kept = format!("{host}/{rel}");
        let from = format!("{host}/{listing}/{}", rel.split('@').next().unwrap());
        file(a, &kept, 2, Some(sidecar_json(&url(&from), 2, sha, rest)));
    };
    sc("ARCHIVE", "R_20260801.zip", "r", NEW);
    // Found again as an entry of the day's zip: where it is still served.
    let in_zip = format!(
        r#"{NEW},"also":[{{"url":"https://{host}/ARCHIVE/VicGas/sub/D.zip!P_20260801.zip","how":"entry","seen_at":"2026-10-22T00:00:00Z"}},{{"url":"x","how":"same"}}]"#
    );
    sc("CURRENT", "VicGas/sub/P_20260801.zip", "p1", &in_zip);
    sc("CURRENT", "VicGas/sub/P_20260902.zip", "p2", NEW);
    // A name without a month: the month it was published, else fetched.
    sc("CURRENT", "VicGas/sub/int.csv", "v1", NEW);
    sc(
        "CURRENT",
        "VicGas/sub/int.csv@20261021T191200+1000",
        "v2",
        "",
    );
    let s = collect(root, Market::Nem, a, 2).unwrap();
    let shown: Vec<String> = s.mismatches.iter().map(ToString::to_string).collect();
    assert_eq!(
        shown,
        [
            "Reports/VicGas/sub/P_20260902.zip: the census gives it two SHA-256s or sizes",
            "Reports/VicGas/sub/gone.csv: the archive has no sidecar for it",
        ]
    );
    let urls = |m: &str| {
        s.months[&m.parse().unwrap()]
            .iter()
            .map(|(u, s)| (u.rsplit('/').next().unwrap().to_owned(), s.size))
            .collect::<Vec<_>>()
    };
    assert_eq!(
        urls("2026-08"),
        [
            ("R_20260801.zip".to_owned(), 2),
            ("P_20260801.zip".to_owned(), 2)
        ]
    );
    assert_eq!(urls("2026-09"), [("P_20260902.zip".to_owned(), 2)]);
    let in_aug = &s.months[&"2026-08".parse().unwrap()];
    let p1 = in_aug
        .iter()
        .find(|(u, _)| u.ends_with("P_20260801.zip"))
        .unwrap()
        .1;
    assert_eq!(
        p1.archived_in.as_deref(),
        Some("https://www.nemweb.com.au/Reports/ARCHIVE/VicGas/sub/D.zip!P_20260801.zip")
    );
    assert_eq!(urls("2026-05"), [("int.csv".to_owned(), 2)]);
    assert_eq!(
        urls("2026-10"),
        [("int.csv".to_owned(), 2)],
        "the version's URL is the file's"
    );
}

#[test]
fn a_census_or_sources_file_that_cannot_be_read_or_written_is_an_error() {
    let (repo, archive) = agreeing();
    let (root, a) = (repo.path(), archive.path());
    let s = collect(root, Market::Nem, a, 1).unwrap();
    // `sources/nem` is a file, so nothing can be written under it.
    std::fs::create_dir_all(root.join("sources")).unwrap();
    std::fs::write(root.join("sources/nem"), "").unwrap();
    assert!(matches!(
        write(root, Market::Nem, &s),
        Err(SourcesError::Write(_))
    ));
    std::fs::write(
        census_path(root, Market::Nem, "2026-04".parse().unwrap()),
        "not toml [",
    )
    .unwrap();
    let e = collect(root, Market::Nem, a, 1).unwrap_err();
    assert!(matches!(e, SourcesError::Census(_)), "{e}");
    let repo = tempfile::tempdir().unwrap();
    let bad = crate::reports::folder_path(repo.path(), Market::Nem, nem_reports(), "X");
    crate::files::write_atomic(&bad, b"files = [").unwrap();
    assert!(
        collect(repo.path(), Market::Nem, a, 1)
            .unwrap_err()
            .to_string()
            .contains("X.toml")
    );
}
