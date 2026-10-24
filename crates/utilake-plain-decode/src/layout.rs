//! Where a downloaded file is kept: one local folder per report folder.
//!
//! AEMO lists a report folder's files under more than one listing: nemweb
//! shows a 5-minute file under `Reports/CURRENT/{folder}/` for a few days,
//! then the day's zip of them under `Reports/ARCHIVE/{folder}/`; the WA
//! host shows a WEMDE dataset's day under `{dataset}/current/`, then under
//! `{dataset}/previous/`. The local path names the report folder, not the
//! listing: the listing segment is dropped, so every file of one folder is
//! held in one place, once, whichever listing it came from. The listing
//! stays in the file's URL, which its sidecar keeps.
//!
//! Everything else is the URL as a path, as the archive has always kept
//! it: host first, each segment percent-decoded with the characters no
//! file system takes replaced, the fragment dropped, a query folded into
//! the file name as `@{16 hex digits of its SHA-256}`.

use sha2::{Digest, Sha256};

/// One rule: on `host`, the segment that follows the segments `before`
/// (`*` matches any one) is a listing when it is one of `segments` (any
/// case).
struct Listing {
    host: &'static str,
    before: &'static [&'static str],
    segments: &'static [&'static str],
}

/// The listings of every host the archive holds.
const LISTINGS: &[Listing] = &[
    Listing {
        host: "www.nemweb.com.au",
        before: &["Reports"],
        segments: &["CURRENT", "ARCHIVE"],
    },
    Listing {
        host: "nemweb.com.au",
        before: &["Reports"],
        segments: &["CURRENT", "ARCHIVE"],
    },
    Listing {
        host: "data.wa.aemo.com.au",
        before: &["public", "market-data", "wemde", "*"],
        segments: &["current", "previous"],
    },
];

/// `%XX` escapes decoded, as the archive always decoded them.
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let hex = |b: u8| (b as char).to_digit(16);
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let (Some(h), Some(l)) = (hex(bytes[i + 1]), hex(bytes[i + 2]))
        {
            out.push(u8::try_from(h * 16 + l).unwrap_or(b'%'));
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// One path segment of a URL as a file or directory name: unescaped, with
/// the characters no file system takes, and the names that would climb out
/// of the archive, replaced.
fn segment(s: &str) -> String {
    let cleaned: String = percent_decode(s)
        .chars()
        .map(|c| {
            if c.is_control() || matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') {
                '_'
            } else {
                c
            }
        })
        .collect();
    match cleaned.as_str() {
        "" | "." | ".." => "_".to_owned(),
        _ => cleaned,
    }
}

/// The index, among `path` (segments below the host), of the listing
/// segment `host` drops, when there is one.
fn listing_at(host: &str, path: &[String]) -> Option<usize> {
    LISTINGS.iter().find_map(|l| {
        if !l.host.eq_ignore_ascii_case(host) {
            return None;
        }
        let at = l.before.len();
        let before_matches = l
            .before
            .iter()
            .zip(path)
            .all(|(want, got)| *want == "*" || want.eq_ignore_ascii_case(got));
        let listed = path
            .get(at)
            .is_some_and(|s| l.segments.iter().any(|w| w.eq_ignore_ascii_case(s)));
        // The listing segment is a folder: something follows it.
        (path.len() > at + 1 && before_matches && listed).then_some(at)
    })
}

/// The host and the cleaned path segments of `url`, the last with its
/// query folded in.
fn parts(url: &str) -> (String, Vec<String>) {
    let url = url.split_once('#').map_or(url, |(u, _)| u);
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    let (location, query) = match rest.split_once('?') {
        Some((l, q)) => (l, Some(q)),
        None => (rest, None),
    };
    let mut segs: Vec<String> = location
        .split('/')
        .filter(|s| !s.is_empty())
        .map(segment)
        .collect();
    if segs.is_empty() {
        segs.push("_".to_owned());
    }
    if let Some(q) = query {
        let digest = Sha256::digest(q.as_bytes());
        let hex: String = digest[..8].iter().map(|b| format!("{b:02x}")).collect();
        let last = segs.last_mut().expect("at least one segment");
        *last = format!("{last}@{hex}");
    }
    let host = segs.remove(0);
    (host, segs)
}

/// Where the bytes of `url` are kept, as path segments below the archive:
/// the host, then the URL's path with its listing segment dropped.
#[must_use]
pub fn local_segments(url: &str) -> Vec<String> {
    let (host, mut path) = parts(url);
    if let Some(at) = listing_at(&host, &path) {
        path.remove(at);
    }
    std::iter::once(host).chain(path).collect()
}

/// The same, `/`-joined: `www.nemweb.com.au/Reports/Dispatch_SCADA/x.zip`.
#[must_use]
pub fn local_rel(url: &str) -> String {
    local_segments(url).join("/")
}

/// The listing `url` was listed under, upper-cased (`ARCHIVE`, `CURRENT`,
/// `PREVIOUS`), when its host lists a folder under more than one.
#[must_use]
pub fn listing(url: &str) -> Option<String> {
    let (host, path) = parts(url);
    listing_at(&host, &path).map(|at| path[at].to_ascii_uppercase())
}

/// A root under `host`, as AEMO lists it (`Reports/CURRENT/GBB`), as the
/// archive keeps it (`Reports/GBB`).
#[must_use]
pub fn local_root(host: &str, root: &str) -> String {
    let segs: Vec<String> = local_segments(&format!("https://{host}/{root}/_"))
        .into_iter()
        .skip(1)
        .collect();
    segs[..segs.len() - 1].join("/")
}

/// A folder kept at `local` under `host` (`Reports/Dispatch_SCADA`), as
/// listed under `listing` (`CURRENT`): `Reports/CURRENT/Dispatch_SCADA`.
/// The folder itself when its host has no such listing.
#[must_use]
pub fn listed_path(host: &str, local: &str, listing: &str) -> String {
    let segs: Vec<&str> = local.split('/').filter(|s| !s.is_empty()).collect();
    let rule = LISTINGS.iter().find(|l| {
        l.host.eq_ignore_ascii_case(host)
            && l.segments.iter().any(|s| s.eq_ignore_ascii_case(listing))
            && segs.len() >= l.before.len()
            && l.before
                .iter()
                .zip(&segs)
                .all(|(w, g)| *w == "*" || w.eq_ignore_ascii_case(g))
    });
    let Some(rule) = rule else {
        return segs.join("/");
    };
    let word = rule
        .segments
        .iter()
        .find(|s| s.eq_ignore_ascii_case(listing))
        .expect("the rule was found by this listing");
    let at = rule.before.len();
    segs[..at]
        .iter()
        .copied()
        .chain(std::iter::once(*word))
        .chain(segs[at..].iter().copied())
        .collect::<Vec<_>>()
        .join("/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_listings_of_a_report_folder_are_one_folder() {
        let current = "https://www.nemweb.com.au/Reports/CURRENT/Dispatch_SCADA/PUBLIC_DISPATCHSCADA_202610190005_0000000543611267.zip";
        let archive = "https://www.nemweb.com.au/Reports/ARCHIVE/Dispatch_SCADA/PUBLIC_DISPATCHSCADA_20261019.zip";
        assert_eq!(
            local_rel(current),
            "www.nemweb.com.au/Reports/Dispatch_SCADA/PUBLIC_DISPATCHSCADA_202610190005_0000000543611267.zip"
        );
        assert_eq!(
            local_rel(archive),
            "www.nemweb.com.au/Reports/Dispatch_SCADA/PUBLIC_DISPATCHSCADA_20261019.zip"
        );
        assert_eq!(listing(current).as_deref(), Some("CURRENT"));
        assert_eq!(listing(archive).as_deref(), Some("ARCHIVE"));
        // Any case, nested folders, the gas folders, the other host name.
        assert_eq!(
            local_rel(
                "https://www.nemweb.com.au/Reports/current/Operational_Demand/ACTUAL_HH/x.zip"
            ),
            "www.nemweb.com.au/Reports/Operational_Demand/ACTUAL_HH/x.zip"
        );
        assert_eq!(
            local_rel("https://www.nemweb.com.au/Reports/ARCHIVE/GSH/Benchmark_Price/x.zip"),
            "www.nemweb.com.au/Reports/GSH/Benchmark_Price/x.zip"
        );
        assert_eq!(
            local_rel("http://nemweb.com.au/Reports/CURRENT/GBB/GasBBFacilities.csv"),
            "nemweb.com.au/Reports/GBB/GasBBFacilities.csv"
        );
        // An entry fetched alone keeps its `!` name.
        assert_eq!(
            local_rel("https://www.nemweb.com.au/Reports/ARCHIVE/Dispatch_SCADA/D.zip!E.zip"),
            "www.nemweb.com.au/Reports/Dispatch_SCADA/D.zip!E.zip"
        );
    }

    #[test]
    fn what_is_not_a_listed_folder_is_kept_as_its_url() {
        let mmsdm = "https://www.nemweb.com.au/Data_Archive/Wholesale_Electricity/MMSDM/2024/MMSDM_2024_01/MMSDM_Historical_Data_SQLLoader/DATA/PUBLIC_ARCHIVE%23DISPATCHPRICE%23FILE01%23202401010000.zip";
        assert_eq!(
            local_rel(mmsdm),
            "www.nemweb.com.au/Data_Archive/Wholesale_Electricity/MMSDM/2024/MMSDM_2024_01/MMSDM_Historical_Data_SQLLoader/DATA/PUBLIC_ARCHIVE#DISPATCHPRICE#FILE01#202401010000.zip"
        );
        assert_eq!(listing(mmsdm), None);
        // The listing's own page, and a folder named CURRENT elsewhere.
        assert_eq!(
            local_rel("https://www.nemweb.com.au/Reports/CURRENT/"),
            "www.nemweb.com.au/Reports/CURRENT"
        );
        assert_eq!(
            local_rel("https://other.host/Reports/CURRENT/x.zip"),
            "other.host/Reports/CURRENT/x.zip"
        );
        // Names no file system takes, climbing, an escape at the end, a
        // query, a fragment, a bare host.
        assert_eq!(
            local_rel("https://h/a/../b:c/x%2.zip?y=1#frag"),
            format!(
                "h/a/_/b_c/x%2.zip@{}",
                Sha256::digest(b"y=1")[..8]
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect::<String>()
            )
        );
        assert_eq!(local_rel("https://h/a%2Fb/%3F%e2%82%ac"), "h/a_b/_€");
        // A bare host is a file named after it, as the archive kept it.
        assert_eq!(local_segments("https://h/"), ["h"]);
        assert_eq!(local_segments(""), ["_"]);
    }

    #[test]
    fn a_wemde_day_is_one_folder_whether_current_or_previous() {
        let prev = "https://data.wa.aemo.com.au/public/market-data/wemde/referenceTradingPrice/previous/ReferenceTradingPrice_20261021.zip";
        assert_eq!(
            local_rel(prev),
            "data.wa.aemo.com.au/public/market-data/wemde/referenceTradingPrice/ReferenceTradingPrice_20261021.zip"
        );
        assert_eq!(listing(prev).as_deref(), Some("PREVIOUS"));
        // The datafiles have no listings.
        let file = "https://data.wa.aemo.com.au/public/public-data/datafiles/facility-scada/facility-scada-2023-10.csv";
        assert_eq!(local_rel(file), file.trim_start_matches("https://"));
        assert_eq!(listing(file), None);
    }

    #[test]
    fn roots_and_folders_map_both_ways() {
        let nem = "www.nemweb.com.au";
        assert_eq!(local_root(nem, "Reports/CURRENT/GBB"), "Reports/GBB");
        assert_eq!(local_root(nem, "Reports/ARCHIVE"), "Reports");
        assert_eq!(local_root(nem, "Reports/CURRENT"), "Reports");
        assert_eq!(local_root(nem, "Data_Archive"), "Data_Archive");
        assert_eq!(
            local_root("data.wa.aemo.com.au", "public/public-data/datafiles"),
            "public/public-data/datafiles"
        );
        assert_eq!(
            listed_path(nem, "Reports/Dispatch_SCADA", "CURRENT"),
            "Reports/CURRENT/Dispatch_SCADA"
        );
        assert_eq!(
            listed_path(nem, "Reports/GSH/Benchmark_Price", "archive"),
            "Reports/ARCHIVE/GSH/Benchmark_Price"
        );
        assert_eq!(
            listed_path(
                "data.wa.aemo.com.au",
                "public/market-data/wemde/facilityScada",
                "PREVIOUS"
            ),
            "public/market-data/wemde/facilityScada/previous"
        );
        // A listing the host does not have, or a folder outside its rule.
        assert_eq!(listed_path(nem, "Reports/X", "LATER"), "Reports/X");
        assert_eq!(
            listed_path(nem, "Data_Archive/X", "CURRENT"),
            "Data_Archive/X"
        );
        assert_eq!(listed_path("h", "/a/b/", "CURRENT"), "a/b");
    }
}
