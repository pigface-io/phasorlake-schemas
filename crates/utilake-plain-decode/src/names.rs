//! What a file's name says: the block it carries, the stamps and months in
//! it; and what a header says: the column it names.
//!
//! An MMS file names its block in its I row. A plain file names it in its
//! file name, by a rule per feed, and this module is the one place that
//! rule lives: the census that records a shape and the lake that ingests
//! it both call [`block_name`], so they can never disagree.
//!
//! A block is `(family, name, version)`, which the MMS rows call
//! `(report_type, report_subtype, version)`: the same three fields,
//! persisted under the same names everywhere.

use utilake_mms_decode::YearMonth;

use crate::feeds::{Decoder, Feed};

/// The block a file carries: what an I row says for MMS, what the name
/// says for everything else.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct BlockName {
    /// The report type: the feed's family (`GBB`, `STTM`, `WEM`, ...).
    pub family: String,
    /// The report subtype: the report's stem, upper snake case.
    pub name: String,
    /// The report version: the `_vN` of a stem, else 1.
    pub version: u16,
}

impl BlockName {
    /// A JSON document's block: this name as the dataset prefix, the
    /// array's path appended.
    pub fn with_array(&self, array: &str) -> Self {
        Self {
            family: self.family.clone(),
            name: format!("{}_{}", self.name, json_name(array)),
            version: self.version,
        }
    }
}

/// The block a file at `path` carries for `feed`, by the feed's rule.
/// `path` is relative to the feed's host and may end in `!entry` for an
/// entry of a zip; a mirror version's `@stamp` and an entry's `~stamp` are
/// taken off first. `None` for an MMS feed (the I row says), or a name the
/// rule cannot read.
pub fn block_name(feed: &Feed, path: &str) -> Option<BlockName> {
    let entry = path.rsplit('!').next().unwrap_or(path);
    let container = path.split('!').next().unwrap_or(path);
    let (unversioned, _) = version_of(container);
    let (entry, _) = version_of(entry);
    let file = entry.rsplit('/').next().unwrap_or(entry);
    let stem = stem_of(file);
    let family = feed.family.to_owned();
    match feed.decoder {
        Decoder::Mms => None,
        Decoder::Csv if feed.id == "gbb" => Some(BlockName {
            family,
            name: gbb_name(stem)?,
            version: 1,
        }),
        Decoder::Csv if feed.id == "sttm" || feed.id == "dwgm" => {
            let (name, version) = int_report(stem)?;
            Some(BlockName {
                family,
                name,
                version,
            })
        }
        Decoder::Csv => {
            // WEM datafiles: the dataset folder names the block.
            let folder = parent_folder(unversioned, feed)?;
            Some(BlockName {
                family,
                name: snake_upper(folder)?,
                version: 1,
            })
        }
        Decoder::Json => {
            // WEMDE and the other market-data folders: the dataset path
            // between the root and `previous/` is the prefix; the JSON
            // reader appends each array's path.
            let dataset = dataset_path(unversioned, feed)?;
            let name = dataset
                .split('/')
                .map(json_name)
                .collect::<Vec<_>>()
                .join("_");
            (!name.is_empty()).then_some(BlockName {
                family,
                name,
                version: 1,
            })
        }
    }
}

/// The file name without its extension, `~stamp` or `-{GUID}-vN` tail.
fn stem_of(file: &str) -> &str {
    let s = file.split_once('~').map_or(file, |(s, _)| s);
    let s = s.split_once("_-{").map_or(s, |(s, _)| s);
    match s.rsplit_once('.') {
        Some((stem, ext)) if is_extension(ext) => stem,
        _ => s,
    }
}

fn is_extension(ext: &str) -> bool {
    matches!(
        ext.to_ascii_lowercase().as_str(),
        "csv" | "zip" | "json" | "txt" | "xml" | "htm" | "html" | "xlsx" | "pdf"
    )
}

/// A GBB stem: `GasBB`, `GASBB_` or `GBB_` off the front, a date or year
/// span off the back, camel case to upper snake.
fn gbb_name(stem: &str) -> Option<String> {
    let lower = stem.to_ascii_lowercase();
    let body = ["gasbb_", "gasbb", "gbb_"]
        .iter()
        .find(|p| lower.starts_with(*p))
        .map_or(stem, |p| &stem[p.len()..]);
    let body = strip_date_tail(body);
    let name = camel_to_upper_snake(body);
    (!name.is_empty()).then_some(name)
}

/// Trailing `_YYYYMMDDHHMMSS`, `_YYYYMMDD`, `_YYYY_YYYY` and `_YYYY` removed.
fn strip_date_tail(s: &str) -> &str {
    let mut s = s;
    loop {
        let Some((head, tail)) = s.rsplit_once('_') else {
            return s;
        };
        let year_like = |t: &str| t.len() == 4 && t.starts_with(['1', '2']);
        let all_digits = !tail.is_empty() && tail.bytes().all(|b| b.is_ascii_digit());
        if all_digits && (matches!(tail.len(), 8 | 14) || year_like(tail)) && !head.is_empty() {
            s = head;
        } else {
            return s;
        }
    }
}

/// An STTM or DWGM stem: `int651_v1_ex_ante_market_price_rpt_1` is
/// `INT651_EX_ANTE_MARKET_PRICE_RPT_1` version 1; `int157_13` is version 1.
fn int_report(stem: &str) -> Option<(String, u16)> {
    let lower = stem.to_ascii_lowercase();
    if !lower.starts_with("int") || lower.len() < 4 || !lower.as_bytes()[3].is_ascii_digit() {
        return None;
    }
    let mut version = 1u16;
    let mut parts = Vec::new();
    for (i, part) in lower.split('_').enumerate() {
        if i > 0
            && let Some(v) = part.strip_prefix('v')
            && !v.is_empty()
            && v.bytes().all(|b| b.is_ascii_digit())
            && parts.len() == 1
        {
            version = v.parse().ok()?;
            continue;
        }
        parts.push(part);
    }
    let name = parts.join("_").to_ascii_uppercase();
    Some((name, version))
}

/// The folder a file sits in, relative to the feed's root: for
/// `public/public-data/datafiles/facility-scada/facility-scada-2023-10.csv`
/// under root `public/public-data/datafiles`, `facility-scada`.
fn parent_folder<'a>(path: &'a str, feed: &Feed) -> Option<&'a str> {
    let rel = under_root(path, feed)?;
    let (folder, _) = rel.rsplit_once('/')?;
    folder.split('/').next_back().filter(|f| !f.is_empty())
}

/// The dataset path between the feed's root and `previous/`, `current/` or
/// the file: `wemde/dispatchSolution/dispatchData/previous/X.zip` under
/// root `public/market-data/wemde` is `dispatchSolution/dispatchData`.
fn dataset_path<'a>(path: &'a str, feed: &Feed) -> Option<&'a str> {
    let rel = under_root(path, feed)?;
    let (folder, _) = rel.rsplit_once('/')?;
    let folder = ["/previous", "/current", "/schema"]
        .iter()
        .find_map(|s| folder.strip_suffix(s))
        .unwrap_or(folder);
    (!folder.is_empty()).then_some(folder)
}

/// `path` relative to the feed root it sits under.
fn under_root<'a>(path: &'a str, feed: &Feed) -> Option<&'a str> {
    let path = path.trim_start_matches('/');
    feed.roots.iter().find_map(|root| {
        let root = root.trim_matches('/');
        path.strip_prefix(root)
            .and_then(|r| r.strip_prefix('/'))
            .or_else(|| path.split_once(&format!("{root}/")).map(|(_, r)| r))
    })
}

/// Camel case to upper snake: a boundary before an upper-case letter that
/// follows a lower-case one, or that an upper-case run ends in before a
/// lower-case letter (`NTLngFlow` is `NT_LNG_FLOW`). Underscores stay;
/// anything else becomes `_`.
pub fn camel_to_upper_snake(s: &str) -> String {
    let b: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len() + 4);
    for (i, &c) in b.iter().enumerate() {
        if c.is_ascii_uppercase() && i > 0 {
            let prev = b[i - 1];
            let next_lower = b.get(i + 1).is_some_and(char::is_ascii_lowercase);
            if prev.is_ascii_lowercase() || (prev.is_ascii_uppercase() && next_lower) {
                out.push('_');
            }
        }
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_uppercase());
        } else {
            out.push('_');
        }
    }
    collapse(&out)
}

/// A JSON key or path segment as a column or block name:
/// `referenceTradingPrice` is `REFERENCE_TRADING_PRICE`.
pub fn json_name(key: &str) -> String {
    camel_to_upper_snake(key)
}

/// Any text as `[A-Z0-9_]`: letters upper-cased, every other run one `_`,
/// none at the ends. `None` when nothing is left.
pub fn snake_upper(s: &str) -> Option<String> {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_uppercase());
        } else {
            out.push('_');
        }
    }
    let out = collapse(&out);
    (!out.is_empty()).then_some(out)
}

/// The column a header names: `Load Forecast (MW)` is `LOAD_FORECAST_MW`,
/// `price_bod_gst_ex` is `PRICE_BOD_GST_EX`. A byte-order mark and quotes
/// are not part of the name. `None` for a header that names nothing.
pub fn column_name(raw: &str) -> Option<String> {
    let s = raw.strip_prefix('\u{feff}').unwrap_or(raw).trim();
    let s = s.trim_matches('"').trim();
    snake_upper(s)
}

fn collapse(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut last_us = true;
    for c in s.chars() {
        if c == '_' {
            if !last_us {
                out.push('_');
            }
            last_us = true;
        } else {
            out.push(c);
            last_us = false;
        }
    }
    if out.ends_with('_') {
        out.pop();
    }
    out
}

/// A version's file name and listed time: `x.csv@20261021T191200+1000` is
/// version `20261021T191200+1000` of `x.csv`. An `@` not followed by a
/// compact RFC 3339 time is part of the name.
#[must_use]
pub fn version_of(name: &str) -> (&str, Option<&str>) {
    let stamp = |at: &str| {
        let b = at.as_bytes();
        let digits =
            |r: std::ops::Range<usize>| b.get(r).is_some_and(|d| d.iter().all(u8::is_ascii_digit));
        let zone = match b.get(15..) {
            Some(b"Z") => true,
            Some([b'+' | b'-', ..]) => b.len() == 20 && digits(16..20),
            _ => false,
        };
        digits(0..8) && b.get(8) == Some(&b'T') && digits(9..15) && zone
    };
    match name.rsplit_once('@') {
        Some((file, at)) if stamp(at) => (file, Some(at)),
        _ => (name, None),
    }
}

/// The publish stamp an entry name carries after `~`:
/// `int651_v1_ex_ante_market_price_rpt_1~20261001113227.csv` was published
/// at `20261001113227` (`YYYYMMDDHHMMSS`, market time). `None` when the
/// name has no such tail.
#[must_use]
pub fn tilde_stamp(name: &str) -> Option<&str> {
    let file = name.rsplit('/').next().unwrap_or(name);
    let (_, tail) = file.rsplit_once('~')?;
    let stamp = tail.split_once('.').map_or(tail, |(s, _)| s);
    (stamp.len() == 14 && stamp.bytes().all(|b| b.is_ascii_digit())).then_some(stamp)
}

/// The first month a name gives: the first run of at least six digits
/// that starts with a year from 1990 to 2099 and a month 01 to 12, or a
/// hyphenated `YYYY-MM`.
#[must_use]
pub fn name_month(name: &str) -> Option<YearMonth> {
    let b = name.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if !b[i].is_ascii_digit() {
            i += 1;
            continue;
        }
        let start = i;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
        if i - start >= 6 {
            let year: i32 = name[start..start + 4].parse().ok()?;
            let month: u8 = name[start + 4..start + 6].parse().ok()?;
            if (1990..2100).contains(&year)
                && let Some(m) = YearMonth::new(year, month)
            {
                return Some(m);
            }
        } else if i - start == 4
            && b.get(i) == Some(&b'-')
            && b.get(i + 1).is_some_and(u8::is_ascii_digit)
            && b.get(i + 2).is_some_and(u8::is_ascii_digit)
            && !b.get(i + 3).is_some_and(u8::is_ascii_digit)
        {
            // `facility-scada-2023-10.csv`: a year and a month, hyphenated.
            let year: i32 = name[start..i].parse().ok()?;
            let month: u8 = name[i + 1..i + 3].parse().ok()?;
            if (1990..2100).contains(&year)
                && let Some(m) = YearMonth::new(year, month)
            {
                return Some(m);
            }
        }
    }
    None
}

/// The month of a `~YYYYMMDDHHMMSS` or `@YYYYMMDDTHHMMSS±HHMM` stamp, or of
/// a name's first dated run: the release month a plain file's name gives.
#[must_use]
pub fn stamp_month(name: &str) -> Option<YearMonth> {
    let (file, at) = version_of(name);
    if let Some(at) = at {
        return name_month(at);
    }
    tilde_stamp(file)
        .and_then(name_month)
        .or_else(|| name_month(file.rsplit('/').next().unwrap_or(file)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::feeds::{FEEDS, feed};

    fn block(feed_id: &str, path: &str) -> Option<(String, String, u16)> {
        block_name(feed(feed_id).unwrap(), path).map(|b| (b.family, b.name, b.version))
    }

    #[test]
    fn gbb_stems() {
        let b = |p: &str| block("gbb", p).unwrap().1;
        assert_eq!(
            b("Reports/CURRENT/GBB/GasBBActualFlowStorage.zip"),
            "ACTUAL_FLOW_STORAGE"
        );
        assert_eq!(
            b(
                "Reports/CURRENT/GBB/GasBBActualFlowStorage.zip@20261021T221100+1000!GasBBActualFlowStorage.CSV"
            ),
            "ACTUAL_FLOW_STORAGE"
        );
        assert_eq!(
            b("GasBBActualFlowStorageLast31.CSV"),
            "ACTUAL_FLOW_STORAGE_LAST31"
        );
        assert_eq!(
            b("GasBBPipelineConnectionFlow_2018_2023.zip"),
            "PIPELINE_CONNECTION_FLOW"
        );
        assert_eq!(
            b("GBB_PIPELINE_CONNECTION_FLOW/GASBB_PIPELINE_CONNECTION_FLOW_2019.zip"),
            "PIPELINE_CONNECTION_FLOW"
        );
        assert_eq!(
            b("PipelineConnectionFlow_History.csv"),
            "PIPELINE_CONNECTION_FLOW_HISTORY"
        );
        assert_eq!(b("GasBB2PSensitivies_20230705.csv"), "2P_SENSITIVIES");
        assert_eq!(b("GasBB2PSensitivies.csv"), "2P_SENSITIVIES");
        assert_eq!(
            b("ForecastUtilisation/GBB_ForecastUtilisation_20210804.CSV"),
            "FORECAST_UTILISATION"
        );
        assert_eq!(
            b("DUPLICATE/GasBBActualFlowStorageLast31_20250930111016.CSV"),
            "ACTUAL_FLOW_STORAGE_LAST31"
        );
        assert_eq!(b("GasBBNTLngFlow.CSV"), "NT_LNG_FLOW");
        assert_eq!(b("GasBBGSHGasTrades.CSV"), "GSH_GAS_TRADES");
        assert_eq!(
            b("GasBBShortTermTransactionsNSW.CSV"),
            "SHORT_TERM_TRANSACTIONS_NSW"
        );
        assert_eq!(b("GasBBLNGShipments.CSV"), "LNG_SHIPMENTS");
        assert_eq!(block("gbb", "GasBB.csv"), None);
        assert_eq!(block("gbb", "_2019.csv").unwrap().1, "2019");
        let b = block("gbb", "GasBBBasins.csv").unwrap();
        assert_eq!((b.0.as_str(), b.2), ("GBB", 1));
    }

    #[test]
    fn int_reports() {
        let b = |p: &str| {
            let (_, n, v) = block("sttm", p).unwrap();
            (n, v)
        };
        assert_eq!(
            b("Reports/CURRENT/STTM/int651_v1_ex_ante_market_price_rpt_1.csv"),
            ("INT651_EX_ANTE_MARKET_PRICE_RPT_1".to_owned(), 1)
        );
        assert_eq!(
            b("Day01.zip!int653_v3_ex_ante_pipeline_price_rpt_1~20261001090014.csv"),
            ("INT653_EX_ANTE_PIPELINE_PRICE_RPT_1".to_owned(), 3)
        );
        assert_eq!(
            b("int659_v1_bid_offer_-{703546CC-BDB3-47B6-B705-A29ADA3AB466}-v5318095.csv"),
            ("INT659_BID_OFFER".to_owned(), 1)
        );
        assert_eq!(
            b("int117_ancillary_payments_rpt_1.csv"),
            ("INT117_ANCILLARY_PAYMENTS_RPT_1".to_owned(), 1)
        );
        let d = block("dwgm", "Reports/CURRENT/VicGas/int111_v5_sdpc_1.csv").unwrap();
        assert_eq!(
            (d.0.as_str(), d.1.as_str(), d.2),
            ("DWGM", "INT111_SDPC_1", 5)
        );
        assert_eq!(block("dwgm", "int157_13.csv").unwrap().1, "INT157_13");
        assert_eq!(
            block("dwgm", "int128_v4_actual_linepack_1~20120908043621.csv")
                .unwrap()
                .2,
            4
        );
        assert_eq!(
            block("dwgm", "int135_v4_uplift_cap~20100817153329.csv")
                .unwrap()
                .1,
            "INT135_UPLIFT_CAP"
        );
        assert_eq!(
            block("dwgm", "int180_v4_op_sched_77.csv").unwrap().1,
            "INT180_OP_SCHED_77"
        );
        assert_eq!(block("sttm", "CurrentDay.zip"), None);
        assert_eq!(block("sttm", "intx.csv"), None);
        assert_eq!(block("sttm", "int.csv"), None);
        assert_eq!(block("sttm", "int1_v99999.csv"), None);
    }

    #[test]
    fn wem_datafiles_take_the_folder() {
        let b = block(
            "datafiles",
            "public/public-data/datafiles/facility-scada/facility-scada-2023-10.csv",
        )
        .unwrap();
        assert_eq!(
            (b.0.as_str(), b.1.as_str(), b.2),
            ("WEM", "FACILITY_SCADA", 1)
        );
        assert_eq!(
            block(
                "datafiles",
                "/public/public-data/datafiles/facility-scada-csv/FacilityScada-2024-01.csv"
            )
            .unwrap()
            .1,
            "FACILITY_SCADA_CSV"
        );
        assert_eq!(
            block("datafiles", "archive/data.wa.aemo.com.au/public/public-data/datafiles/market-clearing-prices-csv/MarketClearingPrices-2024.csv@20261021T083000+0800").unwrap().1,
            "MARKET_CLEARING_PRICES_CSV"
        );
        assert_eq!(
            block("datafiles", "public/public-data/datafiles/x.csv"),
            None
        );
        assert_eq!(block("datafiles", "elsewhere/x.csv"), None);
        assert_eq!(
            block("datafiles", "public/public-data/datafiles/-/x.csv"),
            None
        );
    }

    #[test]
    fn wemde_takes_the_dataset_path_and_appends_the_array() {
        let b = block(
            "wemde",
            "public/market-data/wemde/referenceTradingPrice/previous/ReferenceTradingPrice_20261021.zip!ReferenceTradingPrice_2026-10-21.json",
        )
        .unwrap();
        assert_eq!(
            (b.0.as_str(), b.1.as_str(), b.2),
            ("WEMDE", "REFERENCE_TRADING_PRICE", 1)
        );
        let n = block_name(
            feed("wemde").unwrap(),
            "public/market-data/wemde/dispatchSolution/dispatchData/current/ReferenceDispatchSolution_202610210800.json",
        )
        .unwrap();
        assert_eq!(n.name, "DISPATCH_SOLUTION_DISPATCH_DATA");
        assert_eq!(
            n.with_array("dispatchSolutions").name,
            "DISPATCH_SOLUTION_DISPATCH_DATA_DISPATCH_SOLUTIONS"
        );
        assert_eq!(
            n.with_array("a.b").name,
            "DISPATCH_SOLUTION_DISPATCH_DATA_A_B"
        );
        assert_eq!(block("wemde", "public/market-data/wemde/x.json"), None);
        assert_eq!(
            block("wemde", "public/market-data/wemde/-/schema/x.json"),
            None
        );
        assert_eq!(
            block("market-data", "public/market-data/mt-pasa/current/x.json")
                .unwrap()
                .1,
            "MT_PASA"
        );
    }

    #[test]
    fn mms_feeds_name_their_blocks_in_the_i_row() {
        assert_eq!(
            block(
                "gsh",
                "Reports/CURRENT/GSH/Benchmark_Price/PUBLIC_X_20260717.zip"
            ),
            None
        );
        assert_eq!(
            block("reports", "Reports/CURRENT/Dispatch_SCADA/x.zip"),
            None
        );
        assert!(FEEDS.iter().all(|f| !f.id.is_empty()));
    }

    #[test]
    fn columns_and_cases() {
        assert_eq!(
            column_name("Load Forecast (MW)").unwrap(),
            "LOAD_FORECAST_MW"
        );
        assert_eq!(column_name("\u{feff}GasDate").unwrap(), "GASDATE");
        assert_eq!(column_name("\"Quantity (TJ)\"").unwrap(), "QUANTITY_TJ");
        assert_eq!(column_name("price_bod_gst_ex").unwrap(), "PRICE_BOD_GST_EX");
        assert_eq!(column_name("  "), None);
        assert_eq!(column_name("()"), None);
        assert_eq!(
            json_name("referenceTradingPrice"),
            "REFERENCE_TRADING_PRICE"
        );
        assert_eq!(json_name("isPublished"), "IS_PUBLISHED");
        assert_eq!(json_name("RoCoFPrice"), "RO_CO_F_PRICE");
        assert_eq!(camel_to_upper_snake("already_UPPER"), "ALREADY_UPPER");
        assert_eq!(camel_to_upper_snake("__x__"), "X");
        assert_eq!(snake_upper("mt-pasa").unwrap(), "MT_PASA");
    }

    #[test]
    fn stamps_and_months() {
        assert_eq!(
            version_of("x.csv@20261021T191200+1000"),
            ("x.csv", Some("20261021T191200+1000"))
        );
        assert_eq!(
            version_of("x.csv@20261021T191200Z"),
            ("x.csv", Some("20261021T191200Z"))
        );
        assert_eq!(version_of("a@b.csv"), ("a@b.csv", None));
        assert_eq!(
            version_of("x.csv@20261021T19120+1000"),
            ("x.csv@20261021T19120+1000", None)
        );
        assert_eq!(
            tilde_stamp("d/int651_v1_x_rpt_1~20261001113227.csv"),
            Some("20261001113227")
        );
        assert_eq!(tilde_stamp("int651~2026.csv"), None);
        assert_eq!(tilde_stamp("int651.csv"), None);
        assert_eq!(
            name_month("PUBLIC_X_20260717_0000000527946058.zip"),
            YearMonth::new(2026, 7)
        );
        assert_eq!(name_month("MarketClearingPrices-2024.csv"), None);
        assert_eq!(name_month("x_20261301.csv"), None);
        assert_eq!(name_month("x_18001201.csv"), None);
        assert_eq!(
            stamp_month("GasBB.zip@20261021T221100+1000"),
            YearMonth::new(2026, 10)
        );
        assert_eq!(
            stamp_month("Day01.zip!int651_v1_x~20260930113227.csv"),
            YearMonth::new(2026, 9)
        );
        assert_eq!(
            stamp_month("a/FacilityScada-2024-01.csv"),
            YearMonth::new(2024, 1)
        );
        assert_eq!(
            name_month("facility-scada-2023-10.csv"),
            YearMonth::new(2023, 10)
        );
        assert_eq!(name_month("x-2023-100.csv"), None);
        assert_eq!(name_month("x-2023-13.csv"), None);
        assert_eq!(name_month("x-1800-10.csv"), None);
        assert_eq!(
            stamp_month("a/PUBLIC_X_20251001.zip"),
            YearMonth::new(2025, 10)
        );
    }
}
