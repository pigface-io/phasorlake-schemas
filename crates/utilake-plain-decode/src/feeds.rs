//! Every feed that is not the MMSDM archive: where it is, how its files
//! are read, and the rules its files follow.
//!
//! A feed is one folder tree on one host whose files share a decoder, a
//! block-naming rule, a release-month rule and a dedupe rule. The census
//! walks a feed's roots in the mirror; the lake's sources list and claim
//! the same roots. Both read this table, so the two agree on every file.

use utilake_mms_decode::YearMonth;

use crate::names::{name_month, stamp_month};

/// How a feed's files are read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decoder {
    /// MMS C/I/D rows: the I row names the block.
    Mms,
    /// A header row, then data rows: the file name names the block.
    Csv,
    /// A WEMDE JSON document: one block per array under `data`.
    Json,
}

/// Which month a file was released in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Release {
    /// The date in the file's name first (`PUBLIC_X_20260717.zip`,
    /// `facility-scada-2023-10.csv`), else its stamp.
    NameMonth,
    /// The file's publish stamp first (`~20261001113227`, a mirror
    /// version's `@20261021T221100+1000`), else a date in its name. What
    /// neither gives, the file's sidecar does.
    StampMonth,
}

/// How a later file of the same block relates to the one before it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dedupe {
    /// Every row is new: files do not restate each other.
    None,
    /// A file restates the one before it: only the rows absent from the
    /// previous file of the same block are new. The columns in
    /// [`Feed::time_columns`] are not compared.
    VersionDiff,
}

/// One feed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Feed {
    /// The market: `nem`, `wem` or `gas`.
    pub market: &'static str,
    /// The feed's id within the market: `reports`, `gsh`, `gbb`, `sttm`,
    /// `dwgm`, `datafiles`, `wemde`, `market-data`.
    pub id: &'static str,
    /// The host the feed is published on.
    pub host: &'static str,
    /// The folders under the host, each a root the census and the mirror
    /// walk.
    pub roots: &'static [&'static str],
    /// How its files are read.
    pub decoder: Decoder,
    /// The report type of every block a plain file carries; empty for an
    /// MMS feed, whose I rows say.
    pub family: &'static str,
    /// Which month a file was released in.
    pub release: Release,
    /// How a later file relates to the one before it.
    pub dedupe: Dedupe,
    /// Column names (as [`crate::column_name`] writes them) that carry the
    /// file's own publish time, so are left out when rows are compared.
    pub time_columns: &'static [&'static str],
    /// Folder names under the roots that are not this feed's.
    pub ignore: &'static [&'static str],
    /// The market's offset from UTC, in seconds.
    pub offset_secs: i32,
}

impl Feed {
    /// The market's offset as `+10:00`.
    pub fn offset_text(&self) -> &'static str {
        match self.offset_secs {
            28_800 => "+08:00",
            _ => "+10:00",
        }
    }

    /// The release month a file's name gives under this feed's rule, or
    /// `None` when only its sidecar can say.
    pub fn release_month(&self, name: &str) -> Option<YearMonth> {
        let file = name.rsplit('/').next().unwrap_or(name);
        match self.release {
            Release::NameMonth => name_month(file).or_else(|| stamp_month(name)),
            Release::StampMonth => stamp_month(name),
        }
    }

    /// Whether `folder` (one path segment) is a folder this feed skips.
    pub fn ignores(&self, folder: &str) -> bool {
        self.ignore.iter().any(|f| f.eq_ignore_ascii_case(folder))
    }
}

const NEMWEB: &str = "www.nemweb.com.au";
const WA: &str = "data.wa.aemo.com.au";
const AEST: i32 = 10 * 3600;
const AWST: i32 = 8 * 3600;

/// The gas folders under nemweb's report roots, which the NEM reports feed
/// leaves to the gas feeds.
pub const GAS_FOLDERS: &[&str] = &[
    "GBB",
    "GSH",
    "STTM",
    "VicGas",
    "DWGM",
    "Gas_Supply_Guarantee",
    "ECGS",
    "MMSDataModelReport",
];

/// Every feed.
pub const FEEDS: &[Feed] = &[
    Feed {
        market: "nem",
        id: "reports",
        host: NEMWEB,
        roots: &["Reports/ARCHIVE", "Reports/CURRENT"],
        decoder: Decoder::Mms,
        family: "",
        release: Release::NameMonth,
        dedupe: Dedupe::None,
        time_columns: &[],
        ignore: GAS_FOLDERS,
        offset_secs: AEST,
    },
    Feed {
        market: "gas",
        id: "gsh",
        host: NEMWEB,
        roots: &["Reports/ARCHIVE/GSH", "Reports/CURRENT/GSH"],
        decoder: Decoder::Mms,
        family: "",
        release: Release::NameMonth,
        dedupe: Dedupe::VersionDiff,
        time_columns: &[],
        ignore: &[],
        offset_secs: AEST,
    },
    Feed {
        market: "gas",
        id: "gbb",
        host: NEMWEB,
        roots: &["Reports/CURRENT/GBB"],
        decoder: Decoder::Csv,
        family: "GBB",
        release: Release::StampMonth,
        dedupe: Dedupe::VersionDiff,
        time_columns: &[],
        ignore: &[],
        offset_secs: AEST,
    },
    Feed {
        market: "gas",
        id: "sttm",
        host: NEMWEB,
        roots: &["Reports/CURRENT/STTM"],
        decoder: Decoder::Csv,
        family: "STTM",
        release: Release::StampMonth,
        dedupe: Dedupe::VersionDiff,
        time_columns: &["REPORT_DATETIME"],
        ignore: &["MOS Estimates", "Contingency_Gas"],
        offset_secs: AEST,
    },
    Feed {
        market: "gas",
        id: "dwgm",
        host: NEMWEB,
        roots: &["Reports/CURRENT/VicGas"],
        decoder: Decoder::Csv,
        family: "DWGM",
        release: Release::StampMonth,
        dedupe: Dedupe::VersionDiff,
        time_columns: &["CURRENT_DATE", "CURRENT_DATETIME"],
        ignore: &[],
        offset_secs: AEST,
    },
    Feed {
        market: "wem",
        id: "datafiles",
        host: WA,
        roots: &["public/public-data/datafiles"],
        decoder: Decoder::Csv,
        family: "WEM",
        release: Release::NameMonth,
        dedupe: Dedupe::VersionDiff,
        time_columns: &["EXTRACTED_AT"],
        ignore: &["temp"],
        offset_secs: AWST,
    },
    Feed {
        market: "wem",
        id: "wemde",
        host: WA,
        roots: &["public/market-data/wemde"],
        decoder: Decoder::Json,
        family: "WEMDE",
        release: Release::NameMonth,
        dedupe: Dedupe::None,
        time_columns: &[],
        ignore: &["current"],
        offset_secs: AWST,
    },
    Feed {
        market: "wem",
        id: "market-data",
        host: WA,
        roots: &["public/market-data"],
        decoder: Decoder::Json,
        family: "WEMMD",
        release: Release::NameMonth,
        dedupe: Dedupe::None,
        time_columns: &[],
        ignore: &["wemde", "current"],
        offset_secs: AWST,
    },
];

/// The feed with this id.
pub fn feed(id: &str) -> Option<&'static Feed> {
    FEEDS.iter().find(|f| f.id == id)
}

/// The feeds of a market, in table order.
pub fn feeds_of(market: &str) -> impl Iterator<Item = &'static Feed> {
    FEEDS.iter().filter(move |f| f.market == market)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_unique_and_markets_known() {
        let mut ids: Vec<_> = FEEDS.iter().map(|f| f.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), FEEDS.len());
        assert!(
            FEEDS
                .iter()
                .all(|f| matches!(f.market, "nem" | "wem" | "gas"))
        );
        assert_eq!(feeds_of("gas").count(), 4);
        assert_eq!(feeds_of("wem").count(), 3);
        assert_eq!(feeds_of("nem").count(), 1);
        assert!(feed("x").is_none());
    }

    #[test]
    fn plain_feeds_name_a_family_and_mms_feeds_do_not() {
        for f in FEEDS {
            assert_eq!(f.family.is_empty(), f.decoder == Decoder::Mms, "{}", f.id);
        }
    }

    #[test]
    fn offsets() {
        assert_eq!(feed("gbb").unwrap().offset_text(), "+10:00");
        assert_eq!(feed("wemde").unwrap().offset_text(), "+08:00");
    }

    #[test]
    fn release_months() {
        let gsh = feed("gsh").unwrap();
        assert_eq!(
            gsh.release_month(
                "Reports/CURRENT/GSH/Benchmark_Price/PUBLIC_X_20260717_0000000527946058.zip"
            ),
            YearMonth::new(2026, 7)
        );
        assert_eq!(
            gsh.release_month("PUBLIC_X.zip@20261021T191200+1000"),
            YearMonth::new(2026, 10)
        );
        let gbb = feed("gbb").unwrap();
        assert_eq!(
            gbb.release_month("GasBBActualFlowStorage.zip@20261021T221100+1000"),
            YearMonth::new(2026, 10)
        );
        assert_eq!(
            gbb.release_month("GasBB2PSensitivies_20230705.csv"),
            YearMonth::new(2023, 7)
        );
        assert_eq!(gbb.release_month("GasBBActualFlowStorage.zip"), None);
        let sttm = feed("sttm").unwrap();
        assert_eq!(
            sttm.release_month("Day01.zip!int651_v1_x_rpt_1~20260930113227.csv"),
            YearMonth::new(2026, 9)
        );
        let wem = feed("datafiles").unwrap();
        assert_eq!(
            wem.release_month("datafiles/facility-scada/facility-scada-2023-10.csv"),
            YearMonth::new(2023, 10)
        );
        assert_eq!(
            wem.release_month("MarketClearingPrices-2024.csv@20261021T083000+0800"),
            YearMonth::new(2026, 10)
        );
    }

    #[test]
    fn ignored_folders() {
        assert!(feed("reports").unwrap().ignores("vicgas"));
        assert!(!feed("reports").unwrap().ignores("Dispatch_SCADA"));
        assert!(feed("wemde").unwrap().ignores("current"));
    }
}
