use super::*;

const T: i64 = 1_707_812_820; // 2024-02-13T08:27:00Z

fn s(text: &str) -> String {
    Stamp::parse(text).unwrap().to_string()
}

#[test]
fn formats_with_the_offset() {
    assert_eq!(Stamp::new(T, 0).rfc3339(), "2024-02-13T08:27:00Z");
    assert_eq!(Stamp::new(T, AEST).rfc3339(), "2024-02-13T18:27:00+10:00");
    assert_eq!(Stamp::new(T, AWST).rfc3339(), "2024-02-13T16:27:00+08:00");
    assert_eq!(
        Stamp::new(T, -3600 * 5 - 1800).rfc3339(),
        "2024-02-13T02:57:00-05:30"
    );
    assert_eq!(Stamp::from_unix(0).to_string(), "1970-01-01T00:00:00Z");
    assert_eq!(Stamp::from_unix(-1).to_string(), "1969-12-31T23:59:59Z");
    let frac = Stamp {
        secs_utc: T,
        nanos: 500_000,
        offset_secs: 0,
    };
    assert_eq!(frac.rfc3339(), "2024-02-13T08:27:00.0005Z");
    assert_eq!(Stamp::now().to_string().len(), 20);
}

#[test]
fn compact_for_a_file_name_and_http_dates() {
    let t = |offset| {
        Stamp::at(2026, 10, 21, 11, 53, 0, offset)
            .unwrap()
            .compact()
    };
    assert_eq!(t(36_000), "20261021T115300+1000");
    assert_eq!(t(0), "20261021T115300Z");
    assert_eq!(t(-18_000), "20261021T115300-0500");
    assert_eq!(
        Stamp::new(T, AEST).http_date(),
        "Tue, 13 Feb 2024 08:27:00 GMT"
    );
    let april = Stamp::parse("2026-04-15T04:29:11Z").unwrap();
    assert_eq!(april.http_date(), "Wed, 15 Apr 2026 04:29:11 GMT");
    // Every weekday and month, and the round trip.
    for day in 0..400 {
        let s = Stamp::from_unix(T + day * 86_400 + 37);
        assert_eq!(Stamp::from_http_date(&s.http_date()), Some(s));
    }
}

#[test]
fn the_instant_moves_between_offsets_and_cuts_to_the_minute() {
    let a = Stamp::parse("2026-10-17T23:16:18Z").unwrap();
    let b = a.in_offset(AEST);
    assert_eq!(b.to_string(), "2026-10-18T09:16:18+10:00");
    assert!(a.same_instant(b));
    assert_ne!(a, b);
    assert_eq!((b.unix(), b.offset()), (a.unix(), AEST));
    assert_eq!(b.to_minute().to_string(), "2026-10-18T09:16:00+10:00");
    assert_eq!(Stamp::from_unix(-1).to_minute(), Stamp::from_unix(-60));
    let frac = Stamp { nanos: 5, ..a };
    assert!(!frac.same_instant(a));
    assert_eq!(frac.to_minute(), a.to_minute());
    assert!(a < frac && frac < Stamp::from_unix(a.unix() + 1));
}

#[test]
fn reads_rfc3339_only_with_an_offset() {
    assert_eq!(s("1970-01-01T00:00:00Z"), "1970-01-01T00:00:00Z");
    assert_eq!(s("2024-02-29T12:34:56.123z"), "2024-02-29T12:34:56.123Z");
    assert_eq!(s("2026-10-12T10:45:00+10:00"), "2026-10-12T10:45:00+10:00");
    assert_eq!(s("2026-10-12T10:45:00-03:30"), "2026-10-12T10:45:00-03:30");
    assert_eq!(s("2026-10-12t10:45:00+00:00"), "2026-10-12T10:45:00Z");
    assert_eq!(
        Stamp::parse("2024-02-13 08:27:00.25z").unwrap(),
        Stamp {
            secs_utc: T,
            nanos: 250_000_000,
            offset_secs: 0
        }
    );
    let t = Stamp::parse("2026-10-12T10:45:00+10:00").unwrap();
    assert_eq!((t.unix(), t.offset()), (1_791_765_900, AEST));
    for off in [0, AEST, AWST] {
        let text = Stamp::new(T, off).rfc3339();
        assert_eq!(Stamp::parse(&text), Ok(Stamp::new(T, off)));
    }
    for bad in [
        "2026-10-12 10:45:00",
        "2026-10-12T10:45:00",
        "2026-10-12T10:45:00.",
        "2026-10-12T10:45:00.1234567890Z",
        "2026-10-12T10:45:00+10",
        "2026-10-12T10:45:00+1000",
        "2026-10-12T10:45:00+1:00",
        "2026-10-12T10:45:00+10:0",
        "2026-10-12T10:45:00+24:00",
        "2026-10-12T10:45:00+10:60",
        "2026-10-12T10:45:00*10:00",
        "2026-13-12T10:45:00Z",
        "2024-02-30T08:27:00Z",
        "2026-10-12T25:45:00Z",
        "2026-10-12T10:60:00Z",
        "2026-10-12T10:45:61Z",
        "2026-1-12T10:45:00Z",
        "2026-10-12X10:45:00Z",
        "Monday, October 12, 2026 10:45 AM",
        "t",
        "",
    ] {
        let e = Stamp::parse(bad).unwrap_err();
        assert_eq!(e.0, bad, "{bad}");
    }
    assert_eq!(
        Stamp::parse(&"x".repeat(100))
            .unwrap_err()
            .to_string()
            .len(),
        64 + "`` is not a time in RFC 3339 form with an offset".len()
    );
    // Seconds may be left out, as the lake's readers have always allowed.
    assert_eq!(s("2026-10-12T10:45Z"), "2026-10-12T10:45:00Z");
    assert!(Stamp::from_http_date("Mon, 12 Oct 2026 00:32 GMT").is_some());
    assert!(Stamp::from_naive("2017-01-12 12:24", 0).is_some());
    // A leap second reads as the last second of the minute.
    assert!(Stamp::parse("2016-12-31T23:59:60Z").is_ok());
    assert!(Stamp::at(2026, 10, 12, 0, 0, 0, 86_400).is_none());
    assert!(Stamp::at(2026, 10, 32, 0, 0, 0, 0).is_none());
}

#[test]
fn reads_http_dates() {
    assert_eq!(
        Stamp::from_http_date("Tue, 13 Feb 2024 08:27:00 GMT"),
        Some(Stamp::new(T, 0))
    );
    assert_eq!(
        Stamp::from_http_date("Mon, 18 May 2026 07:27:50 UTC").map(|t| t.rfc3339()),
        Some("2026-05-18T07:27:50Z".into())
    );
    assert_eq!(
        Stamp::from_http_date("Mon, 18 May 2026 07:27:50.5 GMT").map(|t| t.nanos),
        Some(500_000_000)
    );
    for bad in [
        "Tue, 13 Feb 2024 08:27:00",
        "Tue, 13 Feb 2024 08:27:00 AEST",
        "Xyz, 13 Feb 2024 08:27:00 GMT",
        "Tue, 13 Foo 2024 08:27:00 GMT",
        "Tue, 30 Feb 2024 08:27:00 GMT",
        "Tue, 13 Feb 2024 08:27:00 GMT extra",
        "Tue, 13 Feb 2024 08:27:00Z GMT",
        "Mon, 12 Oct 2026 0:32:30 GMT",
        "Mon, 999 Oct 2026 00:32:30 GMT",
        "Monday, 12-Oct-26 00:32:30 GMT",
        "",
    ] {
        assert_eq!(Stamp::from_http_date(bad), None, "{bad}");
    }
}

#[test]
fn reads_wem_generated_at() {
    assert_eq!(
        Stamp::from_wem_generated("Thu Nov 02 2023 11:35:00Z").map(|t| t.rfc3339()),
        Some("2023-11-02T11:35:00Z".into())
    );
    assert_eq!(Stamp::from_wem_generated("Thu Nov 02 2023 11:35:00"), None);
    assert_eq!(Stamp::from_wem_generated("Nov 02 2023 11:35:00Z"), None);
    assert_eq!(
        Stamp::from_wem_generated("Thu Nov 02 2023 11:35:00Z x"),
        None
    );
    assert_eq!(Stamp::from_wem_generated("Thu Nov 31 2023 11:35:00Z"), None);
    assert_eq!(Stamp::from_wem_generated("Thu Nov 02 2023 1:35:00Z"), None);
}

#[test]
fn reads_iis_listings_in_the_offset_given() {
    let at = |t: &str| Stamp::from_listing(t, AEST).unwrap().to_string();
    assert_eq!(
        at("Monday, October 12, 2026 10:45 AM"),
        "2026-10-12T10:45:00+10:00"
    );
    assert_eq!(
        at("Tuesday, December 9, 2025 09:05 PM"),
        "2025-12-09T21:05:00+10:00"
    );
    assert_eq!(
        at("Monday, February 6, 2017 12:23 AM"),
        "2017-02-06T00:23:00+10:00"
    );
    assert_eq!(
        at("Monday, February 6, 2017 12:23 PM"),
        "2017-02-06T12:23:00+10:00"
    );
    assert_eq!(
        at("Sunday, October 18, 2026 9:16 AM"),
        "2026-10-18T09:16:00+10:00"
    );
    assert_eq!(at("2/13/2024  8:27 AM"), "2024-02-13T08:27:00+10:00");
    assert_eq!(at("12/1/2024 12:00 PM"), "2024-12-01T12:00:00+10:00");
    let short = Stamp::from_listing("2/13/2024  8:27 AM", AEST).unwrap();
    assert_eq!(short.secs_utc, T - i64::from(AEST));
    assert_eq!(
        Stamp::from_listing("12/31/2024 12:00 AM", 0)
            .unwrap()
            .rfc3339(),
        "2024-12-31T00:00:00Z"
    );
    assert_eq!(
        Stamp::from_listing("02/03/2024 08:05 am", AWST)
            .unwrap()
            .rfc3339(),
        "2024-02-03T08:05:00+08:00"
    );
    for bad in [
        "",
        "m",
        "2/13/2024 8:27",
        "2/13/2024 8:27 XM",
        "2/13/2024 13:27 PM",
        "2/13/2024 0:27 AM",
        "2/13/2024/1 8:27 AM",
        "2/13/2024 8:27 AM 1",
        "2/13/2024 8:7 AM",
        "2/13/20245 8:27 AM",
        "2/13/2024  8:27AM",
        "2/13/20248:27 AM",
        "2/13 8:27 AM",
        "13/2/2024 8:27 AM",
        "300/13/2024 8:27 AM",
        "Funday, October 12, 2026 10:45 AM",
        "Monday, Octember 12, 2026 10:45 AM",
        "Monday, October 12, 2026 10:45 XM",
        "Monday, October 12, 2026 13:45 PM",
        "Monday, October 12, 2026 0:45 AM",
        "Monday, October 12, 2026 10:45",
        "Monday, October 12, 2026 10:4 AM",
        "Monday, October 12, 2026 100:45 AM",
        "Monday, October 12, 2026 x:45 AM",
        "Monday, October x, 2026 10:45 AM",
        "Monday, October 12, 2026 10:45 AM extra",
        "Monday, October 32, 2026 10:45 AM",
        "2026-10-12T10:45:00+10:00",
    ] {
        assert!(Stamp::from_listing(bad, AEST).is_none(), "{bad}");
    }
}

#[test]
fn reads_a_naive_time_in_the_offset_given() {
    assert_eq!(
        Stamp::from_naive("2017-01-12 12:24:16", AEST)
            .unwrap()
            .rfc3339(),
        "2017-01-12T12:24:16+10:00"
    );
    assert_eq!(
        Stamp::from_naive("2017/01/12T12:24:16.5", 0)
            .unwrap()
            .rfc3339(),
        "2017-01-12T12:24:16.5Z"
    );
    for bad in [
        "2017-01-12T12:24:16Z",
        "2017-01-12",
        "2017-01-12 12:24:16 x",
        "2017-00-12 12:24:16",
        "2017-01-12x12:24:16",
        "m",
    ] {
        assert_eq!(Stamp::from_naive(bad, 0), None, "{bad}");
    }
}

#[test]
fn reads_any_form() {
    let read = |t: &str| Stamp::read(t, AEST).map(|s| s.rfc3339());
    assert_eq!(
        read("Tue, 13 Feb 2024 08:27:00 GMT").as_deref(),
        Some("2024-02-13T08:27:00Z")
    );
    assert_eq!(
        read(" 2024-02-13T08:27:00Z ").as_deref(),
        Some("2024-02-13T08:27:00Z")
    );
    assert_eq!(
        read("Thu Nov 02 2023 11:35:00Z").as_deref(),
        Some("2023-11-02T11:35:00Z")
    );
    assert_eq!(
        read("Monday, May 18, 2026 05:27 PM").as_deref(),
        Some("2026-05-18T17:27:00+10:00")
    );
    assert_eq!(
        read("2017-01-12 12:24:16").as_deref(),
        Some("2017-01-12T12:24:16+10:00")
    );
    assert_eq!(read("yesterday"), None);
    assert_eq!(read(""), None);
}

#[test]
fn serde_is_the_rfc3339_text() {
    let s = Stamp::new(T, AEST);
    let json = serde_json::to_string(&s).unwrap();
    assert_eq!(json, "\"2024-02-13T18:27:00+10:00\"");
    assert_eq!(serde_json::from_str::<Stamp>(&json).unwrap(), s);
    let err = serde_json::from_str::<Stamp>("\"2024-02-13T18:27:00\"").unwrap_err();
    assert!(
        err.to_string().contains("RFC 3339 form with an offset"),
        "{err}"
    );
    assert!(serde_json::from_str::<Stamp>("7").is_err());
}

#[test]
fn helpers_reject_what_is_not_theirs() {
    assert_eq!(number(""), None);
    assert_eq!(number("1234567"), None);
    assert_eq!(number("12a"), None);
    assert_eq!(month_number("Sept"), None);
    assert_eq!(month_number("DEC"), Some(12));
    assert_eq!(month_number("january"), Some(1));
    assert!(is_weekday("SUNDAY"));
    assert!(is_weekday("thu"));
    assert!(!is_weekday("Sun."));
    assert_eq!(time_of_day(b"8:27"), None);
    assert_eq!(time_of_day(b"08-27"), None);
    assert_eq!(time_of_day(b"08:27:0"), None);
    assert_eq!(time_of_day(b"08"), None);
    assert_eq!(iis_time_of_day(b"8:27"), Some((8, 27, 0, 0, &b""[..])));
    assert_eq!(iis_time_of_day(b"08:27"), Some((8, 27, 0, 0, &b""[..])));
    assert_eq!(iis_time_of_day(b":27"), None);
    assert_eq!(iis_time_of_day(b"x:27"), None);
    assert_eq!(twelve_hour(8, b"x", "AM"), None);
    assert_eq!(local_seconds(2023, 2, 29, 0, 0, 0), None);
    assert_eq!(civil_date(b"2024-02-1", b'-'), None);
}
