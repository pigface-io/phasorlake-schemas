use std::collections::HashMap;
use std::fmt::Write as _;
use std::io::Cursor;

use super::*;

/// What the in-memory server answers for a URL.
enum Page {
    /// 200, a body and a `Last-Modified`.
    Ok(Vec<u8>, Option<&'static str>),
    /// Another status, no body.
    Status(u16),
    /// No answer.
    Fail,
    /// 200, a body that breaks off.
    Broken,
}

/// A body that breaks off after a byte.
struct BreaksOff(bool);

impl Read for BreaksOff {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if std::mem::replace(&mut self.0, true) {
            return Err(std::io::Error::other("connection reset"));
        }
        buf[0] = b'x';
        Ok(1)
    }
}

/// nemweb in memory: pages by URL, 404 for any other, and a log of every
/// request with its User-Agent and time.
#[derive(Default)]
struct Server {
    pages: HashMap<String, Page>,
    log: Vec<(String, String, Instant)>,
}

impl Server {
    fn page(&mut self, url: &str, body: &[u8]) {
        self.pages.insert(url.into(), Page::Ok(body.to_vec(), None));
    }
    fn urls(&self) -> Vec<&str> {
        self.log.iter().map(|(u, ..)| u.as_str()).collect()
    }
}

impl Http for Server {
    fn get(&mut self, url: &str, user_agent: &str) -> std::io::Result<Response> {
        self.log
            .push((url.into(), user_agent.into(), Instant::now()));
        let ok = |body: Box<dyn Read>, last: Option<&str>| Response {
            status: 200,
            last_modified: last.map(str::to_owned),
            body,
        };
        match self.pages.get(url) {
            Some(Page::Ok(b, last)) => Ok(ok(Box::new(Cursor::new(b.clone())), *last)),
            Some(Page::Broken) => Ok(ok(Box::new(BreaksOff(false)), None)),
            Some(Page::Fail) => Err(std::io::Error::other("no route to host")),
            Some(Page::Status(s)) => Ok(Response {
                status: *s,
                last_modified: None,
                body: Box::new(std::io::empty()),
            }),
            None => Ok(Response {
                status: 404,
                last_modified: None,
                body: Box::new(std::io::empty()),
            }),
        }
    }
}

const BASE_URL: &str = "https://h";

/// An IIS listing of `path` (which ends in `/`): `(href, text, size)`,
/// `None` for a directory.
fn listing(path: &str, entries: &[(&str, &str, Option<u64>)]) -> Vec<u8> {
    let mut html = format!("<pre><A HREF=\"{path}../\">[To Parent Directory]</A><br><br>");
    for (href, text, size) in entries {
        let size = size.map_or("&lt;dir&gt;".to_owned(), |s| s.to_string());
        let href = if href.contains("://") {
            (*href).to_owned()
        } else {
            format!("{path}{href}")
        };
        let _ = write!(
            html,
            " 2/13/2024  8:27 AM {size:>12} <A HREF=\"{href}\">{text}</A><br>"
        );
    }
    html.push_str("</pre>");
    html.into_bytes()
}

/// The path of a URL on the server.
fn path_of(url: &str) -> &str {
    &url[BASE_URL.len()..]
}

/// A month of 2024 as a directory: two data zips (one in the `#` naming),
/// a big-table zip listed without a size, a control file and the README;
/// beside them what is not fetched.
fn january(s: &mut Server) -> String {
    let m = YearMonth::new(2024, 1).unwrap();
    let root = month_root(BASE_URL, m);
    let data = format!("{root}DATA/");
    s.page(
        &data,
        &listing(
            path_of(&data),
            &[
                (
                    "PUBLIC_DVD_DISPATCHPRICE_202401010000.zip",
                    "PUBLIC_DVD_DISPATCHPRICE_202401010000.zip",
                    Some(5),
                ),
                (
                    "PUBLIC_ARCHIVE%23X%23FILE01%23202401010000.zip",
                    "PUBLIC_ARCHIVE#X#FILE01#202401010000.zip",
                    Some(3),
                ),
                ("notes.txt", "notes.txt", Some(1)),
                ("%2E%2E.zip", "...zip", Some(1)),
                ("a%2Fb.zip", "a/b.zip", Some(1)),
                ("https://elsewhere/x.zip", "x.zip", Some(1)),
                ("https://hx/y.zip", "y.zip", Some(1)),
            ],
        ),
    );
    let predisp = format!("{root}PREDISP_ALL_DATA/");
    // Listed without a size: neither a number nor `<dir>` before it.
    let load = "PUBLIC_DVD_PREDISPATCHLOAD_ALL_202401010000.zip";
    let html = listing(path_of(&predisp), &[(load, load, None)]);
    let html = String::from_utf8(html).unwrap().replace("&lt;dir&gt;", "");
    s.page(&predisp, html.as_bytes());
    let ctl = format!("{root}CTL/");
    s.page(
        &ctl,
        &listing(
            path_of(&ctl),
            &[
                (
                    "PUBLIC_DVD_DISPATCHPRICE_202401.ctl",
                    "PUBLIC_DVD_DISPATCHPRICE_202401.ctl",
                    Some(4),
                ),
                ("sub.ctl/", "sub.ctl", None),
                ("other.txt", "other.txt", Some(1)),
            ],
        ),
    );
    let doc = format!("{root}DOCUMENTATION/");
    s.page(
        &doc,
        &listing(
            path_of(&doc),
            &[
                ("MMS%20Data%20Model/", "MMS Data Model", None),
                ("README.txt", "README.txt", Some(2)),
            ],
        ),
    );
    s.pages.insert(
        format!("{data}PUBLIC_DVD_DISPATCHPRICE_202401010000.zip"),
        Page::Ok(b"price".to_vec(), Some("Tue, 13 Feb 2024 08:27:00 GMT")),
    );
    s.page(
        &format!("{data}PUBLIC_ARCHIVE%23X%23FILE01%23202401010000.zip"),
        b"xxx",
    );
    s.page(
        &format!("{predisp}PUBLIC_DVD_PREDISPATCHLOAD_ALL_202401010000.zip"),
        b"load",
    );
    s.page(
        &format!("{ctl}PUBLIC_DVD_DISPATCHPRICE_202401.ctl"),
        b"ctl.",
    );
    s.page(&format!("{doc}README.txt"), b"rd");
    root
}

#[test]
fn a_month_is_fetched_politely_once_and_recorded() {
    let dir = tempfile::tempdir().unwrap();
    let (out, repo) = (dir.path().join("MMSDM_2024_01"), dir.path().join("repo"));
    let month = YearMonth::new(2024, 1).unwrap();
    let mut s = Server::default();
    let root = january(&mut s);
    let gap = Duration::from_millis(15);
    let done = fetch_month(&mut s, BASE_URL, month, &out, &repo, gap).unwrap();
    assert_eq!(
        done,
        Fetched {
            listed: 5,
            fetched: 5,
            skipped: 0,
            bytes: 18,
            requests: 10
        }
    );
    // One request at a time, the interval apart, each naming this
    // repository; the big-table directory that is not there is a 404.
    assert!(s.log.iter().all(|(_, ua, _)| ua == USER_AGENT));
    assert!(USER_AGENT.contains("https://github.com/g0itb4/utilake-schemas"));
    for w in s.log.windows(2) {
        assert!(w[1].2 - w[0].2 >= gap, "{:?}", s.urls());
    }
    assert!(
        s.urls()
            .contains(&format!("{root}P5MIN_ALL_DATA/").as_str())
    );
    let read = |rel: &str| std::fs::read(out.join(rel)).unwrap();
    assert_eq!(
        read("DATA/PUBLIC_DVD_DISPATCHPRICE_202401010000.zip"),
        b"price"
    );
    assert_eq!(
        read("DATA/PUBLIC_ARCHIVE#X#FILE01#202401010000.zip"),
        b"xxx"
    );
    assert_eq!(
        read("PREDISP_ALL_DATA/PUBLIC_DVD_PREDISPATCHLOAD_ALL_202401010000.zip"),
        b"load"
    );
    assert_eq!(read("CTL/PUBLIC_DVD_DISPATCHPRICE_202401.ctl"), b"ctl.");
    assert_eq!(read("DOCUMENTATION/README.txt"), b"rd");
    let all = crate::census::files_under(&out).unwrap();
    assert_eq!(all.len(), 5, "no .part left, nothing else fetched: {all:?}");
    let tsv = sources_path(&repo, Market::Nem, month);
    assert!(tsv.ends_with("sources/nem/2024-01.tsv"));
    let text = std::fs::read_to_string(&tsv).unwrap();
    let sha = crate::ctl::sha256;
    let want = format!(
        "url\tsha256\tsize\tlast_modified\n\
{root}CTL/PUBLIC_DVD_DISPATCHPRICE_202401.ctl\t{}\t4\t\n\
{root}DATA/PUBLIC_ARCHIVE%23X%23FILE01%23202401010000.zip\t{}\t3\t\n\
{root}DATA/PUBLIC_DVD_DISPATCHPRICE_202401010000.zip\t{}\t5\tTue, 13 Feb 2024 08:27:00 GMT\n\
{root}DOCUMENTATION/README.txt\t{}\t2\t\n\
{root}PREDISP_ALL_DATA/PUBLIC_DVD_PREDISPATCHLOAD_ALL_202401010000.zip\t{}\t4\t\n",
        sha(b"ctl."),
        sha(b"xxx"),
        sha(b"price"),
        sha(b"rd"),
        sha(b"load")
    );
    assert_eq!(text, want);
    assert_eq!(read_sources(&tsv).unwrap().len(), 5);

    // Again: what is there with its listed size is not fetched; the file
    // listed without a size is. The record is the same.
    s.log.clear();
    let again = fetch_month(&mut s, BASE_URL, month, &out, &repo, Duration::ZERO).unwrap();
    assert_eq!((again.fetched, again.skipped, again.requests), (1, 4, 6));
    assert_eq!(std::fs::read_to_string(&tsv).unwrap(), text);
    // A record lost is made again from the files, without what only the
    // server said.
    std::fs::remove_file(&tsv).unwrap();
    fetch_month(&mut s, BASE_URL, month, &out, &repo, Duration::ZERO).unwrap();
    let remade = std::fs::read_to_string(&tsv).unwrap();
    assert_eq!(remade, want.replace("Tue, 13 Feb 2024 08:27:00 GMT", ""));
}

#[test]
fn an_old_month_is_its_whole_zip_from_the_year_listing() {
    let dir = tempfile::tempdir().unwrap();
    let (out, repo) = (dir.path().join("m"), dir.path().join("repo"));
    let mut s = Server::default();
    let year = year_root(BASE_URL, 2012);
    s.page(
        &year,
        &listing(
            path_of(&year),
            &[
                ("MMSDM_2012_01.zip", "MMSDM_2012_01.zip", Some(5)),
                ("MMSDM_2012_03.zip", "MMSDM_2012_03.zip", Some(5)),
                ("other.zip", "other.zip", Some(5)),
            ],
        ),
    );
    s.page(&format!("{year}MMSDM_2012_01.zip"), b"whole");
    let jan = YearMonth::new(2012, 1).unwrap();
    let done = fetch_month(&mut s, BASE_URL, jan, &out, &repo, Duration::ZERO).unwrap();
    assert_eq!((done.listed, done.fetched, done.requests), (1, 1, 2));
    assert_eq!(
        std::fs::read(out.join("MMSDM_2012_01.zip")).unwrap(),
        b"whole"
    );
    // A month the year does not have says which it has; a year that is not
    // there says so.
    let feb = YearMonth::new(2012, 2).unwrap();
    let e = fetch_month(&mut s, BASE_URL, feb, &out, &repo, Duration::ZERO).unwrap_err();
    assert_eq!(
        e.to_string(),
        format!("2012-02 is not in the archive at {year}; it has MMSDM_2012_01, MMSDM_2012_03")
    );
    let e = fetch_month(
        &mut s,
        BASE_URL,
        YearMonth::new(2011, 1).unwrap(),
        &out,
        &repo,
        Duration::ZERO,
    )
    .unwrap_err();
    assert!(e.to_string().ends_with("it has no such year"), "{e}");
    let y2010 = year_root(BASE_URL, 2010);
    s.page(
        &y2010,
        &listing(path_of(&y2010), &[("x.zip", "x.zip", Some(1))]),
    );
    let e = fetch_month(
        &mut s,
        BASE_URL,
        YearMonth::new(2010, 1).unwrap(),
        &out,
        &repo,
        Duration::ZERO,
    )
    .unwrap_err();
    assert!(e.to_string().ends_with("it has no months"), "{e}");
    // A month from 2015 whose directory is missing is looked up by year:
    // listed there as a directory, and still not found, it is a 404.
    let y2016 = year_root(BASE_URL, 2016);
    s.page(
        &y2016,
        &listing(
            path_of(&y2016),
            &[("MMSDM_2016_03/", "MMSDM_2016_03", None)],
        ),
    );
    let e = fetch_month(
        &mut s,
        BASE_URL,
        YearMonth::new(2016, 3).unwrap(),
        &out,
        &repo,
        Duration::ZERO,
    )
    .unwrap_err();
    assert!(matches!(e, FetchError::NotFound { .. }), "{e}");
    s.pages.insert(y2016.clone(), Page::Status(500));
    let e = fetch_month(
        &mut s,
        BASE_URL,
        YearMonth::new(2016, 3).unwrap(),
        &out,
        &repo,
        Duration::ZERO,
    )
    .unwrap_err();
    assert_eq!(
        e.to_string(),
        format!("nemweb answered {y2016} with HTTP 500")
    );
}

#[test]
fn a_refusal_stops_the_fetch_and_keeps_what_landed() {
    let dir = tempfile::tempdir().unwrap();
    let (out, repo) = (dir.path().join("m"), dir.path().join("repo"));
    let month = YearMonth::new(2024, 1).unwrap();
    for status in [403, 429] {
        let mut s = Server::default();
        let root = january(&mut s);
        let second = format!("{root}DATA/PUBLIC_ARCHIVE%23X%23FILE01%23202401010000.zip");
        s.pages.insert(second.clone(), Page::Status(status));
        let e = fetch_month(&mut s, BASE_URL, month, &out, &repo, Duration::ZERO).unwrap_err();
        assert!(matches!(e, FetchError::Refused { .. }), "{e}");
        assert!(
            e.to_string()
                .contains(&format!("HTTP {status}, so fetching stops"))
        );
        assert_eq!(s.urls().last(), Some(&second.as_str()), "nothing after it");
        let recorded = read_sources(&sources_path(&repo, Market::Nem, month)).unwrap();
        assert_eq!(recorded.len(), 1, "the control file, fetched first");
        // A listing refused stops it too.
        let mut s = Server::default();
        let root = january(&mut s);
        s.pages.insert(format!("{root}DATA/"), Page::Status(status));
        let e = fetch_month(&mut s, BASE_URL, month, &out, &repo, Duration::ZERO).unwrap_err();
        assert!(matches!(e, FetchError::Refused { .. }), "{e}");
        assert_eq!(s.log.len(), 1);
    }
}

#[test]
fn what_cannot_be_fetched_or_written_is_an_error_naming_it() {
    use std::os::unix::fs::PermissionsExt as _;
    let dir = tempfile::tempdir().unwrap();
    let (out, repo) = (dir.path().join("m"), dir.path().join("repo"));
    let month = YearMonth::new(2024, 1).unwrap();
    let fetch = |s: &mut Server, out: &Path, repo: &Path| {
        fetch_month(s, BASE_URL, month, out, repo, Duration::ZERO).unwrap_err()
    };
    let mut s = Server::default();
    let root = january(&mut s);
    // A month without its documentation is fetched without it.
    s.pages.remove(&format!("{root}DOCUMENTATION/"));
    let done = fetch_month(
        &mut s,
        BASE_URL,
        month,
        &dir.path().join("nodoc"),
        &repo,
        Duration::ZERO,
    );
    assert_eq!(done.unwrap().listed, 4);
    std::fs::remove_dir_all(&repo).unwrap();
    let ctl = format!("{root}CTL/PUBLIC_DVD_DISPATCHPRICE_202401.ctl");

    // No answer; an answer that breaks off, for a file and for a listing.
    s.pages.insert(ctl.clone(), Page::Fail);
    let e = fetch(&mut s, &out, &repo);
    assert_eq!(e.to_string(), format!("the request for {ctl} failed"));
    s.pages.insert(ctl.clone(), Page::Broken);
    assert!(matches!(
        fetch(&mut s, &out, &repo),
        FetchError::Http { .. }
    ));
    assert!(
        out.join("CTL/PUBLIC_DVD_DISPATCHPRICE_202401.ctl.part")
            .is_file()
    );
    assert!(!out.join("CTL/PUBLIC_DVD_DISPATCHPRICE_202401.ctl").exists());
    s.pages.insert(format!("{root}CTL/"), Page::Broken);
    assert!(matches!(
        fetch(&mut s, &out, &repo),
        FetchError::Http { .. }
    ));
    s.pages.insert(format!("{root}CTL/"), Page::Status(503));
    assert!(matches!(
        fetch(&mut s, &out, &repo),
        FetchError::Status { status: 503, .. }
    ));

    // A listing of no file.
    let mut s = Server::default();
    let root = january(&mut s);
    s.page(&format!("{root}DATA/"), b"<html>maintenance</html>");
    let e = fetch(&mut s, &out, &repo);
    assert!(e.to_string().contains("links to no file"), "{e}");

    // A directory where a file goes; a month directory under a file; a
    // repository that is a file.
    let mut s = Server::default();
    january(&mut s);
    let fresh = dir.path().join("fresh");
    std::fs::create_dir_all(fresh.join("CTL/PUBLIC_DVD_DISPATCHPRICE_202401.ctl")).unwrap();
    let e = fetch(&mut s, &fresh, &repo);
    assert!(
        e.to_string()
            .ends_with("CTL/PUBLIC_DVD_DISPATCHPRICE_202401.ctl could not be written"),
        "{e}"
    );
    std::fs::create_dir_all(fresh.join("DATA/PUBLIC_ARCHIVE#X#FILE01#202401010000.zip.part"))
        .unwrap();
    std::fs::remove_dir(fresh.join("CTL/PUBLIC_DVD_DISPATCHPRICE_202401.ctl")).unwrap();
    let e = fetch(&mut s, &fresh, &repo);
    assert!(
        e.to_string().contains(".zip.part could not be written"),
        "{e}"
    );
    let file = dir.path().join("file");
    std::fs::write(&file, "x").unwrap();
    assert!(matches!(
        fetch(&mut s, &file, &repo),
        FetchError::Write { .. }
    ));
    let ro = dir.path().join("ro");
    std::fs::create_dir_all(ro.join("sources/nem")).unwrap();
    std::fs::set_permissions(
        ro.join("sources/nem"),
        std::fs::Permissions::from_mode(0o555),
    )
    .unwrap();
    let e = fetch(&mut s, &dir.path().join("other"), &ro);
    assert!(
        e.to_string().contains("2024-01.tsv could not be written"),
        "{e}"
    );

    // A sources file that is not one, or cannot be read.
    let tsv = sources_path(&repo, Market::Nem, month);
    std::fs::create_dir_all(tsv.parent().unwrap()).unwrap();
    std::fs::write(
        &tsv,
        "url\tsha256\tsize\tlast_modified\nhttps://h/x\tab\tbig\t\n",
    )
    .unwrap();
    let e = fetch(&mut s, &out, &repo);
    assert!(
        e.to_string().ends_with(
            "line 2 is not a URL, sha256, size, Last-Modified and archived-in separated by tabs"
        ),
        "{e}"
    );
    std::fs::write(&tsv, "url\nhttps://h/x\tab\t1\n").unwrap();
    assert!(matches!(
        read_sources(&tsv),
        Err(FetchError::Sources { .. })
    ));
    // The fifth column: where the bytes are still served, written only in
    // a file where some row has it.
    std::fs::write(
        &tsv,
        "url\tsha256\tsize\tlast_modified\tarchived_in\nhttps://h/a\tab\t1\t\thttps://h/D.zip!a\nhttps://h/b\tcd\t2\tT\t\n",
    )
    .unwrap();
    let read = read_sources(&tsv).unwrap();
    assert_eq!(
        read["https://h/a"].archived_in.as_deref(),
        Some("https://h/D.zip!a")
    );
    assert_eq!(read["https://h/b"].archived_in, None);
    assert_eq!(
        crate::fetch::sources_text(&read),
        std::fs::read_to_string(&tsv).unwrap()
    );
    std::fs::write(&tsv, "url\nhttps://h/a\tab\t1\t\tz\tmore\n").unwrap();
    assert!(
        read_sources(&tsv)
            .unwrap_err()
            .to_string()
            .ends_with("line 2 has more than five columns")
    );
    std::fs::remove_file(&tsv).unwrap();
    std::fs::create_dir(&tsv).unwrap();
    assert!(matches!(
        read_sources(&tsv),
        Err(FetchError::Sources { .. })
    ));
    std::fs::remove_dir(&tsv).unwrap();

    // A file there with its listed size but unreadable, and no record.
    let locked = dir.path().join("locked");
    let readme = locked.join("DOCUMENTATION/README.txt");
    std::fs::create_dir_all(readme.parent().unwrap()).unwrap();
    std::fs::write(&readme, "rd").unwrap();
    std::fs::set_permissions(&readme, std::fs::Permissions::from_mode(0o000)).unwrap();
    let e = fetch(&mut s, &locked, &dir.path().join("repo2"));
    assert!(e.to_string().ends_with("README.txt cannot be read"), "{e}");
}

#[test]
fn listings_are_parsed_as_iis_writes_them() {
    let html = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../testdata/listings/mmsdm_2024_01_data.html"
    ))
    .unwrap();
    let entries = parse_listing(&html);
    assert_eq!(
        entries.len(),
        149,
        "every file of the fixture listing, parent link excluded"
    );
    let price = entries
        .iter()
        .find(|e| e.name == "PUBLIC_DVD_DISPATCHPRICE_202401010000.zip")
        .unwrap();
    assert_eq!(price.size, Some(1_584_114));
    assert!(
        price
            .href
            .ends_with("/DATA/PUBLIC_DVD_DISPATCHPRICE_202401010000.zip")
    );
    assert!(!price.is_dir);
    assert!(entries.iter().all(|e| !e.name.starts_with('[')));

    let html = br#"<pre><A HREF="/a/">[To Parent Directory]</A><br><br> 2/13/2024  8:27 AM        &lt;dir&gt; <A HREF="/a/sub/">sub</A><br> 2/13/2024  8:28 AM          123 <a href="/a/f.zip">f.zip</a><br>junk <A HREF="/a/g.csv">g.csv</A><br>7 <A HREF="/a/h.zip">h.zip</A><br> x &lt;dir&gt; <A HREF="/a/d/">d</A> <A HREF="/a/last">last"#;
    let e = parse_listing(html);
    assert_eq!(e.len(), 6, "{e:?}");
    assert!(e[0].is_dir && e[0].size.is_none() && e[0].name == "sub");
    assert_eq!((e[1].size, e[1].name.as_str()), (Some(123), "f.zip"));
    assert_eq!(e[2].size, None, "no number before it");
    assert_eq!(e[3].size, None, "a number alone is no size");
    assert!(e[4].is_dir);
    assert_eq!(e[5].name, "last", "a link to the end");
    assert!(parse_listing(b"<html>nothing</html>").is_empty());
    assert!(parse_listing(b"<A HREF=\"unterminated").is_empty());
    assert!(parse_listing(b"<A HREF=\"x\"").is_empty());
}

#[test]
fn hrefs_become_urls_and_names() {
    assert_eq!(
        decode("PUBLIC_ARCHIVE%23X%23FILE01"),
        "PUBLIC_ARCHIVE#X#FILE01"
    );
    assert_eq!(decode("a%zzb%2"), "a%zzb%2");
    assert_eq!(decode("MMS%20Data"), "MMS Data");
    assert_eq!(join("https://h/a/b/", "/c/d.zip"), "https://h/c/d.zip");
    assert_eq!(join("https://h/a/", "d.zip"), "https://h/a/d.zip");
    assert_eq!(join("https://h/a/", "https://o/d.zip"), "https://o/d.zip");
    assert_eq!(join("h", "/d.zip"), "h/d.zip");
    assert_eq!(
        month_root("https://h/", YearMonth::new(2024, 1).unwrap()),
        "https://h/Data_Archive/Wholesale_Electricity/MMSDM/2024/MMSDM_2024_01/MMSDM_Historical_Data_SQLLoader/"
    );
}
