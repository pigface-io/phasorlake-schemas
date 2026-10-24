use super::*;

const SHA: &str = "fc1b15966cebdfefef2d677d28630548de16efbf0309478b15131b92162d6b68";
const CURRENT: &str = "https://www.nemweb.com.au/Reports/CURRENT/Dispatch_SCADA/PUBLIC_DISPATCHSCADA_202610180920_0000000543501903.zip";
const DAILY: &str = "https://www.nemweb.com.au/Reports/ARCHIVE/Dispatch_SCADA/PUBLIC_DISPATCHSCADA_20261018.zip!PUBLIC_DISPATCHSCADA_202610180920_0000000543501903.zip";

fn at(t: &str) -> Stamp {
    Stamp::parse(t).unwrap()
}

fn read(json: &str) -> Provenance {
    Provenance::from_json(json.as_bytes()).unwrap()
}

/// A sidecar as the mirror wrote it on 2026-10-19 (from the volume).
fn mirror_v1() -> String {
    format!(
        r#"{{
  "url": "{CURRENT}",
  "size": 4044,
  "sha256": "{SHA}",
  "crc32": 1063594450,
  "fetched_at": "2026-10-19T23:14:57Z",
  "last_modified": "Sat, 17 Oct 2026 23:16:18 GMT",
  "last_modified_at": "2026-10-17T23:16:18Z",
  "listed_size": 4044,
  "listed_modified": "Sunday, October 18, 2026 09:16 AM",
  "listed_modified_at": "2026-10-18T09:16:00+10:00",
  "tool": "nemweb-mirror/0.1.0"
}}"#
    )
}

#[test]
fn a_mirror_sidecar_reads_and_keeps_only_what_nothing_else_gives() {
    let p = read(&mirror_v1());
    assert_eq!(p.content.size, 4044);
    assert_eq!(p.content.sha256.hex(), SHA);
    assert_eq!(p.content.crc32, Some(1_063_594_450));
    assert_eq!(p.tool, "nemweb-mirror/0.1.0");
    assert_eq!(p.url(), CURRENT);
    assert_eq!(p.fetched_at(), at("2026-10-19T23:14:57Z"));
    assert_eq!(p.modified(), Some(at("2026-10-17T23:16:18Z")));
    assert_eq!(p.etag(), None);
    // The listing's size is the file's and its time the server's cut to
    // the minute: neither is written, both are given back.
    assert_eq!(
        p.origin().via,
        Via::Fetched {
            listed: Some(Listed::default()),
            served: Some(Served {
                etag: None,
                modified: Some(at("2026-10-17T23:16:18Z")),
            }),
        }
    );
    let l = p.listed(CURRENT).unwrap();
    assert_eq!(l.size, Some(4044));
    assert_eq!(
        l.at,
        Some(at("2026-10-18T09:16:00+10:00")),
        "in the listing's clock"
    );
    assert!(p.listed_as(CURRENT, Some(4044), Some(at("2026-10-18T09:16:00+10:00"))));
    assert!(p.listed_as(CURRENT, None, None));
    assert!(!p.listed_as(CURRENT, Some(4045), None));
    assert!(!p.listed_as(CURRENT, None, Some(at("2026-10-18T09:17:00+10:00"))));
    assert!(!p.listed_as("https://elsewhere/x.zip", None, None));
    let json = p.to_json();
    assert!(
        !json.contains("listed_modified") && !json.contains("\"at\""),
        "{json}"
    );
    assert_eq!(read(&json), p);
}

#[test]
fn a_wa_listing_is_given_back_in_its_own_clock() {
    let url = "https://data.wa.aemo.com.au/public/market-data/wemde/x/previous/X_20261018.zip";
    let v1 = format!(
        r#"{{"url":"{url}","size":1,"sha256":"{SHA}","fetched_at":"2026-10-19T00:00:00Z",
            "last_modified_at":"2026-10-17T23:16:18Z","listed_modified":"Sunday, October 18, 2026 7:16 AM"}}"#
    );
    let p = read(&v1);
    assert_eq!(p.origin().via.listed(), Some(&Listed::default()));
    assert_eq!(
        p.listed(url).unwrap().at,
        Some(at("2026-10-18T07:16:00+08:00"))
    );
    // The same instant written in another clock is kept as written.
    let other = read(&v1.replace(
        r#""listed_modified":"Sunday, October 18, 2026 7:16 AM""#,
        r#""listed_modified_at":"2026-10-18T09:16:00+10:00""#,
    ));
    assert_eq!(
        other.listed(url).unwrap().at.unwrap().to_string(),
        "2026-10-18T09:16:00+10:00"
    );
}

#[test]
fn a_listing_that_differs_is_kept() {
    let v1 = mirror_v1()
        .replace(r#""listed_size": 4044"#, r#""listed_size": 4000"#)
        .replace("2026-10-18T09:16:00+10:00", "2026-10-18T09:20:00+10:00");
    let p = read(&v1);
    let Via::Fetched { listed, .. } = &p.origin().via else {
        panic!("fetched")
    };
    assert_eq!(
        *listed,
        Some(Listed {
            size: Some(4000),
            at: Some(at("2026-10-18T09:20:00+10:00")),
        })
    );
    assert_eq!(p.listed(CURRENT), *listed);
    assert_eq!(read(&p.to_json()), p);
}

#[test]
fn an_old_sidecar_without_instants_reads_its_text() {
    let v1 = format!(
        r#"{{"url":"{CURRENT}","size":1,"sha256":"{SHA}","fetched_at":"2026-10-01T00:00:00Z",
            "etag":"\"e\"","last_modified":"Thu, 01 Oct 2026 00:00:00 GMT",
            "listed_modified":"Thursday, October 1, 2026 10:05 AM","tool":"nemweb-mirror/0.1.0"}}"#
    );
    let p = read(&v1);
    let Via::Fetched { listed, served } = &p.origin().via else {
        panic!("fetched")
    };
    assert_eq!(served.as_ref().unwrap().etag.as_deref(), Some("\"e\""));
    assert_eq!(p.etag(), Some("\"e\""));
    // 10:05 AEST is not 00:00 UTC cut to the minute: kept.
    assert_eq!(listed.unwrap().at, Some(at("2026-10-01T10:05:00+10:00")));
    assert_eq!(p.content.crc32, None);
    // No listing and no response at all, and no tool.
    let bare = read(&format!(
        r#"{{"url":"{CURRENT}","size":1,"sha256":"{SHA}","fetched_at":"2026-10-01T00:00:00Z"}}"#
    ));
    assert_eq!(
        bare.origin().via,
        Via::Fetched {
            listed: None,
            served: None
        }
    );
    assert_eq!(bare.tool, "");
    assert_eq!(bare.listed(CURRENT), None);
}

#[test]
fn every_address_of_a_layout_sidecar() {
    let v1 = mirror_v1().replace(
        r#""tool": "nemweb-mirror/0.1.0""#,
        &format!(
            r#""tool": "utilake-mirror/0.1.0",
  "gone_at": "2026-10-23T02:00:00Z",
  "also": [
    {{"url": "{DAILY}", "how": "entry", "seen_at": "2026-10-20T03:00:00Z"}},
    {{"url": "https://www.nemweb.com.au/Reports/ARCHIVE/Dispatch_SCADA/PUBLIC_DISPATCHSCADA_202610180920_0000000543501903.zip",
      "how": "same", "seen_at": "2026-10-21T00:00:00Z", "gone_at": "2026-10-22T00:00:00Z",
      "listed_at": "2026-10-18T09:16:00+10:00"}},
    {{"url": "{DAILY}", "how": "entry"}}
  ]"#
        ),
    );
    let p = read(&v1);
    assert_eq!(p.addresses.len(), 3, "the repeat is dropped");
    assert_eq!(p.origin().gone, Some(at("2026-10-23T02:00:00Z")));
    let entry = p.archived_in().unwrap();
    assert_eq!(
        (entry.url.as_str(), entry.seen),
        (DAILY, at("2026-10-20T03:00:00Z"))
    );
    let same = &p.addresses[2];
    assert_eq!(same.gone, Some(at("2026-10-22T00:00:00Z")));
    assert_eq!(
        same.via,
        Via::Same {
            listed: Some(Listed {
                size: None,
                at: Some(at("2026-10-18T09:16:00+10:00")),
            })
        }
    );
    assert_eq!(p.listed(&same.url).unwrap().size, Some(4044));
    assert!(p.listed_as(&same.url, Some(4044), Some(at("2026-10-17T23:16:00Z"))));
    // An entry has no listing of its own: the time never disagrees.
    assert_eq!(p.listed(DAILY), None);
    assert!(p.listed_as(DAILY, Some(4044), Some(at("2026-01-01T00:00:00Z"))));
    assert_eq!(read(&p.to_json()), p);
}

#[test]
fn an_entry_fetched_alone_keeps_its_span() {
    let v1 = format!(
        r#"{{"url":"https://h/A.zip!B.zip","size":3,"sha256":"{SHA}","fetched_at":"2026-10-01T00:00:00Z",
            "listed_modified":"Thursday, October 1, 2026 10:05 AM","listed_modified_at":"2026-10-01T10:05:00+10:00",
            "tool":"utilake/0.1.0",
            "extracted_from":{{"url":"https://h/A.zip","entry":"B.zip","zip_size":100,"offset":30,"compressed":3,"method":0,"crc32":7}}}}"#
    );
    let p = read(&v1);
    assert_eq!(p.content.crc32, Some(7));
    assert_eq!(
        p.origin().via,
        Via::Range {
            span: Span {
                zip: "https://h/A.zip".into(),
                entry: "B.zip".into(),
                zip_size: 100,
                offset: 30,
                compressed: 3,
                method: 0,
            },
            listed: Some(Listed {
                size: None,
                at: Some(at("2026-10-01T10:05:00+10:00")),
            }),
            served: None,
        }
    );
    // The zip's listed size is not the entry's: never filled in.
    assert_eq!(p.listed(p.url()).unwrap().size, None);
    // A ranged listing keeps its size even when it equals the entry's.
    let mut q = Provenance::new(
        p.content,
        "t",
        Address {
            via: Via::Range {
                span: Span {
                    zip_size: 3,
                    ..match &p.origin().via {
                        Via::Range { span, .. } => span.clone(),
                        _ => unreachable!(),
                    }
                },
                listed: Some(Listed {
                    size: Some(3),
                    at: None,
                }),
                served: None,
            },
            ..p.origin().clone()
        },
    );
    assert_eq!(q.origin().via.listed().unwrap().size, Some(3));
    assert_eq!(read(&q.to_json()), q);
    assert!(!q.add(q.origin().clone()));
    // A crc32 the sidecar has wins over the span's.
    let both = read(&v1.replace(r#""size":3,"#, r#""size":3,"crc32":9,"#));
    assert_eq!(both.content.crc32, Some(9));
}

#[test]
fn addresses_grow_and_go() {
    let mut p = read(&mirror_v1());
    let other = "https://www.nemweb.com.au/Reports/ARCHIVE/Dispatch_SCADA/x.zip";
    let same = Address {
        url: other.into(),
        seen: at("2026-10-21T00:00:00Z"),
        gone: None,
        via: Via::Same {
            listed: Some(Listed {
                size: Some(4044),
                at: None,
            }),
        },
    };
    assert!(p.add(same.clone()));
    assert!(!p.add(same));
    assert_eq!(p.addresses[1].via.listed().unwrap().size, None, "trimmed");
    assert_eq!(p.addresses[1].via.served(), None);
    assert!(p.names(other) && p.names(CURRENT) && !p.names("x"));
    assert!(p.gone(other, at("2026-10-22T00:00:00Z")));
    assert!(!p.gone(other, at("2026-10-23T00:00:00Z")));
    assert_eq!(
        p.address(other).unwrap().gone,
        Some(at("2026-10-22T00:00:00Z"))
    );
    assert!(!p.gone("x", at("2026-10-23T00:00:00Z")));
    assert!(p.archived_in().is_none());
    assert!(Via::Entry.listed().is_none() && Via::Entry.served().is_none());
    assert!(!Via::Entry.obtains());
}

#[test]
fn the_written_form_is_format_2() {
    let p = read(&mirror_v1());
    let v: serde_json::Value = serde_json::from_str(&p.to_json()).unwrap();
    assert_eq!(
        v,
        serde_json::json!({
            "size": 4044,
            "sha256": SHA,
            "crc32": 1_063_594_450,
            "tool": "nemweb-mirror/0.1.0",
            "addresses": [{
                "url": CURRENT,
                "seen": "2026-10-19T23:14:57Z",
                "via": "fetched",
                "listed": {},
                "served": {"modified": "2026-10-17T23:16:18Z"},
            }],
        })
    );
    // Through serde as well as `from_json`.
    let q: Provenance = serde_json::from_value(v).unwrap();
    assert_eq!(q, p);
}

#[test]
fn records_that_break_the_rules_do_not_read() {
    let bad = |json: String| {
        Provenance::from_json(json.as_bytes())
            .unwrap_err()
            .to_string()
    };
    let v2 = |addresses: &str| {
        format!(r#"{{"size":1,"sha256":"{SHA}","tool":"t","addresses":{addresses}}}"#)
    };
    let fetched = format!(r#"{{"url":"{CURRENT}","seen":"2026-10-01T00:00:00Z","via":"fetched"}}"#);
    let entry = format!(r#"{{"url":"{DAILY}","seen":"2026-10-01T00:00:00Z","via":"entry"}}"#);
    assert!(bad(v2("[]")).contains("no address"));
    assert!(bad(v2(&format!("[{entry}]"))).contains("was not where the bytes were obtained"));
    assert!(bad(v2(&format!("[{fetched},{fetched}]"))).contains("named twice"));
    assert!(
        read(&v2(&format!("[{fetched},{entry}]")))
            .archived_in()
            .is_some()
    );
    // Format 1 with a time that does not read, or a way not known.
    assert!(
        bad(mirror_v1().replace("2026-10-19T23:14:57Z", "soon"))
            .contains("`fetched_at` holds `soon`")
    );
    assert!(bad(mirror_v1().replace("2026-10-19T23:14:57Z", "")).contains("`fetched_at` holds ``"));
    // An instant must say its offset; a text that is not a time is none.
    assert!(
        bad(mirror_v1().replace("2026-10-17T23:16:18Z", "2026-10-17T23:16:18"))
            .contains("`last_modified`")
    );
    let odd = read(
        &mirror_v1()
            .replace(r#""last_modified_at": "2026-10-17T23:16:18Z","#, "")
            .replace("Sat, 17 Oct 2026 23:16:18 GMT", "unknown"),
    );
    assert_eq!(odd.origin().via.served(), None);
    assert!(bad(mirror_v1().replace("2026-10-17T23:16:18Z", "x")).contains("`last_modified`"));
    assert!(
        bad(mirror_v1().replace("2026-10-18T09:16:00+10:00", "x")).contains("`listed_modified`")
    );
    let how = mirror_v1().replace(
        r#""tool""#,
        &format!(r#""also":[{{"url":"{DAILY}","how":"guess"}}],"tool""#),
    );
    assert!(bad(how).contains("neither `same` nor `entry`"));
    let gone = mirror_v1().replace(r#""tool""#, r#""gone_at":"x","tool""#);
    assert!(bad(gone).contains("`gone_at`"));
    for field in ["seen_at", "gone_at", "listed_at"] {
        let also = mirror_v1().replace(
            r#""tool""#,
            &format!(r#""also":[{{"url":"{DAILY}","how":"same","{field}":"x"}}],"tool""#),
        );
        assert!(bad(also).contains(&format!("also.{field}")), "{field}");
    }
    assert!(bad("{}".into()).starts_with("not a provenance record"));
    let e: Result<Provenance, _> = serde_json::from_str(&v2("[]"));
    assert!(e.unwrap_err().to_string().contains("no address"));
}
