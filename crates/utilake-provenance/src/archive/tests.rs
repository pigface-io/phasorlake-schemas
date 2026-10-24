use super::*;
use crate::content::{Content, Sha256};
use crate::provenance::{Address, Via};

const URL: &str = "https://www.nemweb.com.au/Reports/CURRENT/Dispatch_SCADA/A.zip";
const OTHER: &str = "https://www.nemweb.com.au/Reports/ARCHIVE/Dispatch_SCADA/A.zip";

fn provenance(url: &str, bytes: &[u8]) -> Provenance {
    Provenance::new(
        Content {
            size: bytes.len() as u64,
            sha256: Sha256::of(bytes),
            crc32: None,
        },
        "test/0",
        Address {
            url: url.into(),
            seen: Stamp::from_unix(1_800_000_000),
            gone: None,
            via: Via::Fetched {
                listed: None,
                served: None,
            },
        },
    )
}

fn hold(path: &Path, url: &str, bytes: &[u8]) -> Provenance {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, bytes).unwrap();
    let p = provenance(url, bytes);
    Archive::write(path, &p).unwrap();
    p
}

fn v1(url: &str) -> String {
    format!(
        r#"{{"url":"{url}","size":1,"sha256":"{}","fetched_at":"2026-10-01T00:00:00Z"}}"#,
        Sha256::of(b"x")
    )
}

#[test]
fn paths_follow_the_layout() {
    let a = Archive::new("/a");
    assert_eq!(a.root(), Path::new("/a"));
    assert_eq!(
        a.path_for(URL),
        Path::new("/a/www.nemweb.com.au/Reports/Dispatch_SCADA/A.zip")
    );
    assert_eq!(
        a.url_path(URL),
        Path::new("/a/www.nemweb.com.au/Reports/CURRENT/Dispatch_SCADA/A.zip")
    );
    assert_eq!(
        Archive::record_path(Path::new("/a/x.zip")),
        Path::new("/a/x.zip.provenance.json")
    );
    assert!(is_record("x.zip.provenance.json") && !is_record("x.zip.json"));
    for noise in [
        "._x.zip",
        "x.zip.part",
        "x.zip.part.resume",
        "x.zip.part.json",
        "x.json.tmp",
    ] {
        assert!(is_noise(noise), "{noise}");
    }
    assert!(!is_noise("x.zip") && !is_noise("x.json"));
}

#[test]
fn versions_parts_gone_and_verify() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("A.zip");
    let at = Stamp::parse("2026-10-21T11:53:00+10:00").unwrap();
    assert_eq!(
        Archive::version_path(&path, at),
        dir.path().join("A.zip@20261021T115300+1000")
    );
    assert_eq!(Archive::part_path(&path), dir.path().join("A.zip.part"));
    let gone = Stamp::from_unix(1_800_000_200);
    assert!(!Archive::gone(&path, [URL], gone).unwrap(), "no record");
    let p = hold(&path, URL, b"abc");
    assert!(Archive::gone(&path, [URL, OTHER], gone).unwrap());
    assert!(!Archive::gone(&path, [URL], gone).unwrap(), "noted already");
    assert_eq!(
        Archive::read(&path).unwrap().unwrap().origin().gone,
        Some(gone)
    );
    let held = Held {
        path: path.clone(),
        provenance: p,
    };
    assert!(Archive::verify(&held).unwrap());
    std::fs::write(&path, b"abd").unwrap();
    assert!(!Archive::verify(&held).unwrap());
    std::fs::remove_file(&path).unwrap();
    assert!(Archive::verify(&held).is_err());
}

#[test]
fn a_record_is_written_whole_and_read_back() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("h/A.zip");
    assert_eq!(Archive::read(&path).unwrap(), None);
    let p = hold(&path, URL, b"abc");
    assert_eq!(Archive::read(&path).unwrap(), Some(p.clone()));
    assert!(!dir.path().join("h/A.zip.provenance.json.tmp").exists());
    // Written again, it replaces the record.
    let mut q = p;
    q.gone(URL, Stamp::from_unix(1_800_000_100));
    Archive::write(&path, &q).unwrap();
    assert_eq!(Archive::read(&path).unwrap(), Some(q));
}

#[test]
fn a_format_1_record_is_read_and_replaced_by_the_new_one() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("A.zip");
    std::fs::write(&path, b"x").unwrap();
    std::fs::write(dir.path().join("A.zip.json"), v1(URL)).unwrap();
    let p = Archive::read(&path).unwrap().unwrap();
    assert_eq!(p.url(), URL);
    Archive::write(&path, &p).unwrap();
    assert!(!dir.path().join("A.zip.json").exists());
    assert_eq!(Archive::read(&path).unwrap(), Some(p));
    // A `.json` beside it that is not a record is not touched.
    let data = dir.path().join("B");
    std::fs::write(&data, b"x").unwrap();
    std::fs::write(dir.path().join("B.json"), b"{\"not\":\"a record\"}").unwrap();
    Archive::write(&data, &provenance(URL, b"x")).unwrap();
    assert!(dir.path().join("B.json").exists());
}

#[test]
fn what_does_not_read_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("A.zip");
    std::fs::write(dir.path().join("A.zip.provenance.json"), b"{").unwrap();
    let e = Archive::read(&path).unwrap_err().to_string();
    assert!(
        e.contains("A.zip.provenance.json") && e.contains("not a provenance record"),
        "{e}"
    );
    // A record that is a directory cannot be read.
    std::fs::create_dir(dir.path().join("B.provenance.json")).unwrap();
    assert!(matches!(
        Archive::read(&dir.path().join("B")),
        Err(Error::Io { .. })
    ));
    // Nowhere to write.
    let gone = dir.path().join("no/such/C");
    let e = Archive::write(&gone, &provenance(URL, b"x")).unwrap_err();
    assert!(matches!(e, Error::Io { .. }), "{e}");
    // A record the rename cannot replace: a non-empty directory there.
    let d = dir.path().join("D");
    std::fs::create_dir_all(dir.path().join("D.provenance.json/x")).unwrap();
    assert!(Archive::write(&d, &provenance(URL, b"x")).is_err());
    // A format-1 record that cannot be removed.
    let ro = dir.path().join("ro");
    std::fs::create_dir(&ro).unwrap();
    std::fs::write(ro.join("E"), b"x").unwrap();
    std::fs::write(ro.join("E.json"), v1(URL)).unwrap();
    std::fs::write(
        ro.join("E.provenance.json"),
        provenance(URL, b"x").to_json(),
    )
    .unwrap();
    let mut perms = std::fs::metadata(&ro).unwrap().permissions();
    perms.set_readonly(true);
    std::fs::set_permissions(&ro, perms.clone()).unwrap();
    let r = Archive::write(&ro.join("E"), &provenance(URL, b"x"));
    #[allow(clippy::permissions_set_readonly_false)]
    perms.set_readonly(false);
    std::fs::set_permissions(&ro, perms).unwrap();
    assert!(r.is_err());
}

#[test]
fn every_version_at_a_path_and_the_one_a_url_names() {
    let dir = tempfile::tempdir().unwrap();
    let a = Archive::new(dir.path());
    let path = a.path_for(URL);
    assert!(Archive::held_at(&path).unwrap().is_empty());
    assert_eq!(a.lookup(URL).unwrap(), None);
    let first = hold(&path, URL, b"one");
    let later = path.with_file_name("A.zip@20261019T000000Z");
    let second = hold(&later, OTHER, b"two");
    // Noise and records beside them are not versions.
    std::fs::write(path.with_file_name("A.zip@20261020T000000Z.part"), b"").unwrap();
    std::fs::write(path.with_file_name("._A.zip@x"), b"").unwrap();
    // A version without a record is not held.
    std::fs::write(path.with_file_name("A.zip@20261021T000000Z"), b"three").unwrap();
    let held = Archive::held_at(&path).unwrap();
    assert_eq!(
        held,
        [
            Held {
                path: path.clone(),
                provenance: first.clone()
            },
            Held {
                path: later.clone(),
                provenance: second
            }
        ]
    );
    assert_eq!(a.lookup(URL).unwrap().unwrap().provenance, first);
    assert_eq!(a.lookup(OTHER).unwrap().unwrap().path, later);
    assert_eq!(
        a.lookup("https://www.nemweb.com.au/Reports/CURRENT/Dispatch_SCADA/B.zip")
            .unwrap(),
        None
    );
    // A version whose record does not read, or whose bytes are cut
    // short, is not held.
    std::fs::write(
        path.with_file_name("A.zip@20261021T000000Z.provenance.json"),
        b"[",
    )
    .unwrap();
    assert_eq!(Archive::held_at(&path).unwrap().len(), 2);
    std::fs::write(&later, b"tw").unwrap();
    assert_eq!(Archive::held_at(&path).unwrap().len(), 1);
    // A version that is a link to nothing is not held.
    std::os::unix::fs::symlink(
        dir.path().join("nowhere"),
        path.with_file_name("A.zip@20261020T000000Z"),
    )
    .unwrap();
    assert_eq!(Archive::held_at(&path).unwrap().len(), 1);
    // A record that cannot be read at all stops the read.
    std::fs::create_dir(path.with_file_name("A.zip@20261022T000000Z")).unwrap();
    std::fs::create_dir(path.with_file_name("A.zip@20261022T000000Z.provenance.json")).unwrap();
    assert!(Archive::held_at(&path).is_err());
}

#[test]
fn the_walk_finds_held_files_by_their_records() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let sub = root.join("h/Reports/X");
    hold(&sub.join("A.zip"), URL, b"a");
    // A WEMDE file that is JSON, with its record.
    hold(&sub.join("day.json"), URL, b"{}");
    // A file held before format 2, and a file with no record.
    std::fs::write(sub.join("old.zip"), b"o").unwrap();
    std::fs::write(sub.join("old.zip.json"), v1(URL)).unwrap();
    std::fs::write(sub.join("bare.zip"), b"b").unwrap();
    // Noise.
    std::fs::write(sub.join("._A.zip"), b"").unwrap();
    std::fs::write(sub.join("A.zip.provenance.json.tmp"), b"").unwrap();
    std::fs::write(sub.join("c.zip.part"), b"").unwrap();
    hold(&root.join("h/top.zip"), URL, b"t");
    assert_eq!(
        Archive::walk(root).unwrap(),
        [
            sub.join("A.zip"),
            sub.join("day.json"),
            sub.join("old.zip"),
            root.join("h/top.zip"),
        ]
    );
    assert!(Archive::walk(&root.join("nope")).is_err());
}
