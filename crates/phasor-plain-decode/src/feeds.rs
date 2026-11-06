//! Every feed that is not the MMSDM archive: where it is, how its files
//! are read, and the rules its files follow.
//!
//! A feed is one folder tree on one host whose files share a decoder, a
//! block-naming rule, a release-month rule and a dedupe rule. The census
//! walks a feed's roots in the mirror; the lake's sources list and claim
//! the same roots. Both read this table, so the two agree on every file.

use phasor_decode::YearMonth;

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

/// How a feed's host lists a folder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Listing {
    /// An IIS directory page: a `<pre>` of names, sizes and times (AEMO's
    /// hosts).
    Iis,
    /// Azure Blob Storage: `?restype=container&comp=list&prefix=...&delimiter=/`,
    /// an XML page of blobs and prefixes, 5,000 a page (the Electricity
    /// Authority's `emidatasets` account).
    AzureBlob,
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
    /// The market: `nem`, `wem`, `gas` or `nz`.
    pub market: &'static str,
    /// The feed's id within the market: `reports`, `gsh`, `gbb`, `sttm`,
    /// `dwgm`, `datafiles`, `wemde`, `market-data`, `emi`.
    pub id: &'static str,
    /// The host the feed is published on.
    pub host: &'static str,
    /// The folders under the host, each a root the census and the mirror
    /// walk.
    pub roots: &'static [&'static str],
    /// How the host lists a folder.
    pub listing: Listing,
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
    /// Blocks whose header holds a column per date or per period: read
    /// with those columns unpivoted into two, as each says.
    pub unpivot: &'static [(&'static str, Unpivot)],
    /// The feed's spellings of a missing value, read as empty (`NULL` in
    /// the EMI's files of 2015); none for a feed that leaves a cell empty.
    pub nulls: &'static [&'static str],
}

/// How a block's wide header is read as rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unpivot {
    /// Header fields that are dates (a column per forecast day, so the
    /// header changes every day) are not columns: each row is one row per
    /// such field, with the date in `date` and the field's value in
    /// `value`.
    Dated {
        /// The date column.
        date: &'static str,
        /// The value column.
        value: &'static str,
    },
    /// Header fields `{prefix}1`, `{prefix}2`, ... (a column per trading
    /// period) are not columns: each row is one row per such field up to
    /// the last one the row fills, with the number in `number` and the
    /// field's value in `value`. A day of 46 periods fills 46 of 50.
    Numbered {
        /// The fields' common prefix.
        prefix: &'static str,
        /// The number column.
        number: &'static str,
        /// The value column.
        value: &'static str,
    },
}

impl Unpivot {
    /// The two columns the unpivoted fields become: the key, then the
    /// value.
    pub fn columns(self) -> (&'static str, &'static str) {
        match self {
            Self::Dated { date, value } => (date, value),
            Self::Numbered { number, value, .. } => (number, value),
        }
    }
}

impl Feed {
    /// The market's offset as `+10:00`: one of the three clocks a feed is
    /// published in.
    pub fn offset_text(&self) -> &'static str {
        match self.offset_secs {
            AWST => "+08:00",
            NZST => "+12:00",
            _ => "+10:00",
        }
    }

    /// The release month a file's name gives under this feed's rule, or
    /// `None` when only its sidecar can say. `name` may be a zip and an
    /// entry joined with `!`: the entry's name is asked first, then the
    /// zip's.
    pub fn release_month(&self, name: &str) -> Option<YearMonth> {
        let container = name.split('!').next().unwrap_or(name);
        let entry = name.rsplit('!').next().unwrap_or(name);
        let file_of = |p: &str| p.rsplit('/').next().unwrap_or(p).to_owned();
        let (entry_file, container_file) = (file_of(entry), file_of(container));
        let by_name = || {
            name_month(&entry_file).or_else(|| {
                (entry != container)
                    .then(|| name_month(&container_file))
                    .flatten()
            })
        };
        let by_stamp = || {
            stamp_month(&entry_file).or_else(|| {
                (entry != container)
                    .then(|| stamp_month(&container_file))
                    .flatten()
            })
        };
        match self.release {
            Release::NameMonth => by_name().or_else(by_stamp),
            Release::StampMonth => by_stamp().or_else(by_name),
        }
    }

    /// The feed's roots as the archive keeps them ([`phasor_provenance::layout`]): a
    /// root under two listings is one folder (`Reports/ARCHIVE` and
    /// `Reports/CURRENT` are `Reports`), in table order.
    #[must_use]
    pub fn local_roots(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for r in self.roots {
            let local = phasor_provenance::layout::local_root(self.host, r);
            if !out.contains(&local) {
                out.push(local);
            }
        }
        out
    }

    /// Whether `folder` (one path segment) is a folder this feed skips.
    pub fn ignores(&self, folder: &str) -> bool {
        self.ignore.iter().any(|f| f.eq_ignore_ascii_case(folder))
    }

    /// How the block `name`'s wide header unpivots, when it is such a
    /// block.
    pub fn unpivot_of(&self, name: &str) -> Option<Unpivot> {
        self.unpivot
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, how)| *how)
    }
}

const NEMWEB: &str = "www.nemweb.com.au";
const WA: &str = "data.wa.aemo.com.au";
/// The Electricity Authority's blob account; its `publicdata` container
/// is the first path segment.
const EMI: &str = "emidatasets.blob.core.windows.net";
const AEST: i32 = 10 * 3600;
const AWST: i32 = 8 * 3600;
const NZST: i32 = 12 * 3600;

/// The folders under the EMI roots the `emi` feed leaves alone: solver
/// inputs and case files (not tables), the frozen pre-real-time-pricing
/// archive, and the zipped forecast schedules.
pub const EMI_IGNORED: &[&str] = &[
    "GDX",
    "CaseFiles",
    "Pre1November2022Archive",
    "ForecastPrices",
];

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
        listing: Listing::Iis,
        decoder: Decoder::Mms,
        family: "",
        release: Release::NameMonth,
        dedupe: Dedupe::None,
        time_columns: &[],
        ignore: GAS_FOLDERS,
        offset_secs: AEST,
        unpivot: &[],
        nulls: &[],
    },
    Feed {
        market: "gas",
        id: "gsh",
        host: NEMWEB,
        roots: &["Reports/ARCHIVE/GSH", "Reports/CURRENT/GSH"],
        listing: Listing::Iis,
        decoder: Decoder::Mms,
        family: "",
        release: Release::NameMonth,
        dedupe: Dedupe::VersionDiff,
        time_columns: &[],
        ignore: &[],
        offset_secs: AEST,
        unpivot: &[],
        nulls: &[],
    },
    Feed {
        market: "gas",
        id: "gbb",
        host: NEMWEB,
        roots: &["Reports/CURRENT/GBB"],
        listing: Listing::Iis,
        decoder: Decoder::Csv,
        family: "GBB",
        release: Release::StampMonth,
        dedupe: Dedupe::VersionDiff,
        time_columns: &[],
        ignore: &[],
        offset_secs: AEST,
        unpivot: &[(
            "FORECAST_UTILISATION",
            Unpivot::Dated {
                date: "FORECAST_DATE",
                value: "QUANTITY",
            },
        )],
        nulls: &[],
    },
    Feed {
        market: "gas",
        id: "sttm",
        host: NEMWEB,
        roots: &["Reports/CURRENT/STTM"],
        listing: Listing::Iis,
        decoder: Decoder::Csv,
        family: "STTM",
        release: Release::StampMonth,
        dedupe: Dedupe::VersionDiff,
        time_columns: &["REPORT_DATETIME"],
        ignore: &["MOS Estimates", "Contingency_Gas"],
        offset_secs: AEST,
        unpivot: &[],
        nulls: &[],
    },
    Feed {
        market: "gas",
        id: "dwgm",
        host: NEMWEB,
        roots: &["Reports/CURRENT/VicGas"],
        listing: Listing::Iis,
        decoder: Decoder::Csv,
        family: "DWGM",
        release: Release::StampMonth,
        dedupe: Dedupe::VersionDiff,
        time_columns: &["CURRENT_DATE", "CURRENT_DATETIME"],
        ignore: &[],
        offset_secs: AEST,
        unpivot: &[],
        nulls: &[],
    },
    Feed {
        market: "wem",
        id: "datafiles",
        host: WA,
        roots: &["public/public-data/datafiles"],
        listing: Listing::Iis,
        decoder: Decoder::Csv,
        family: "WEM",
        release: Release::NameMonth,
        dedupe: Dedupe::VersionDiff,
        time_columns: &["EXTRACTED_AT"],
        ignore: &["temp"],
        offset_secs: AWST,
        unpivot: &[],
        nulls: &[],
    },
    Feed {
        market: "wem",
        id: "wemde",
        host: WA,
        roots: &["public/market-data/wemde"],
        listing: Listing::Iis,
        decoder: Decoder::Json,
        family: "WEMDE",
        release: Release::NameMonth,
        dedupe: Dedupe::None,
        time_columns: &[],
        ignore: &["current"],
        offset_secs: AWST,
        unpivot: &[],
        nulls: &[],
    },
    Feed {
        market: "wem",
        id: "market-data",
        host: WA,
        roots: &["public/market-data"],
        listing: Listing::Iis,
        decoder: Decoder::Json,
        family: "WEMMD",
        release: Release::NameMonth,
        dedupe: Dedupe::None,
        time_columns: &[],
        ignore: &["wemde", "current"],
        offset_secs: AWST,
        unpivot: &[],
        nulls: &[],
    },
    // New Zealand: the Electricity Authority's datasets, plain CSV on blob
    // storage, a file a day or a month, the file's name naming the table.
    // Interim, final and corrected files are distinct files; which row is
    // current is a reader's question, so nothing is deduplicated.
    Feed {
        market: "nz",
        id: "emi",
        host: EMI,
        roots: &[
            "publicdata/Datasets/Wholesale/DispatchAndPricing",
            "publicdata/Datasets/Wholesale/BidsAndOffers",
            "publicdata/Datasets/Wholesale/Generation",
            "publicdata/Datasets/Wholesale/Metered_data",
            "publicdata/Datasets/Wholesale/MappingsAndGeospatial",
            "publicdata/Datasets/Retail",
        ],
        listing: Listing::AzureBlob,
        decoder: Decoder::Csv,
        family: "EMI",
        release: Release::NameMonth,
        dedupe: Dedupe::None,
        time_columns: &[],
        ignore: EMI_IGNORED,
        offset_secs: NZST,
        unpivot: &[
            ("GENERATION_MD", TRADING_PERIODS),
            ("GRID_EXPORT", TRADING_PERIODS),
            ("GRID_IMPORT", TRADING_PERIODS),
            ("HVDC_FLOWS", TRADING_PERIODS),
            ("UNIT_LEVEL_GENERATION", TRADING_PERIODS),
            ("EMBEDDED_GENERATION", TRADING_PERIODS),
            ("REACTIVE_POWER", TRADING_PERIODS),
            ("GRID_EXPORT_TO_TIWAI", TRADING_PERIODS),
        ],
        nulls: &["NULL"],
    },
];

/// The EMI's metered files: a column per trading period, `TP1` to `TP50`,
/// read as a row per period: the generation, metered and reactive-power
/// tables of Generation/ and Metered_data/.
const TRADING_PERIODS: Unpivot = Unpivot::Numbered {
    prefix: "TP",
    number: "TRADING_PERIOD",
    value: "QUANTITY",
};

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
                .all(|f| matches!(f.market, "nem" | "wem" | "gas" | "nz"))
        );
        assert_eq!(feeds_of("gas").count(), 4);
        assert_eq!(feeds_of("wem").count(), 3);
        assert_eq!(feeds_of("nem").count(), 1);
        assert_eq!(feeds_of("nz").count(), 1);
        assert!(feed("x").is_none());
        // AEMO's hosts list IIS pages; the Electricity Authority's blob
        // store lists blobs.
        for f in FEEDS {
            assert_eq!(
                f.listing == Listing::AzureBlob,
                f.market == "nz",
                "{}",
                f.id
            );
        }
        let emi = feed("emi").unwrap();
        assert!(emi.ignores("gdx") && emi.ignores("CaseFiles") && !emi.ignores("ByMonth"));
        assert_eq!(emi.local_roots().len(), emi.roots.len());
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
        assert_eq!(feed("emi").unwrap().offset_text(), "+12:00");
        // Every feed's text is its seconds: no clock is silently another's.
        for f in FEEDS {
            let hours: i32 = f.offset_text()[1..3].parse().unwrap();
            assert_eq!(hours * 3600, f.offset_secs, "{}", f.id);
        }
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
    fn local_roots_merge_the_listings() {
        assert_eq!(feed("reports").unwrap().local_roots(), ["Reports"]);
        assert_eq!(feed("gsh").unwrap().local_roots(), ["Reports/GSH"]);
        assert_eq!(feed("gbb").unwrap().local_roots(), ["Reports/GBB"]);
        assert_eq!(feed("dwgm").unwrap().local_roots(), ["Reports/VicGas"]);
        assert_eq!(
            feed("wemde").unwrap().local_roots(),
            ["public/market-data/wemde"]
        );
    }

    #[test]
    fn entries_give_their_month_before_their_zip() {
        let sttm = feed("sttm").unwrap();
        assert_eq!(
            sttm.release_month(
                "Reports/CURRENT/STTM/Day01.zip@20261101T190000+1000!int651_v1_x~20261001113227.csv"
            ),
            YearMonth::new(2026, 10)
        );
        assert_eq!(
            sttm.release_month("Day01.zip@20261101T190000+1000!int651_v1_x.csv"),
            YearMonth::new(2026, 11)
        );
        let gsh = feed("gsh").unwrap();
        assert_eq!(
            gsh.release_month(
                "PUBLIC_X_20260901.zip!PUBLIC_X_20260917_0000000535574168.zip!PUBLIC_X_20260917.CSV"
            ),
            YearMonth::new(2026, 9)
        );
        assert_eq!(gsh.release_month("PUBLIC_X.zip!x.csv"), None);
        let gbb = feed("gbb").unwrap();
        assert_eq!(
            gbb.release_month(
                "GasBBActualFlowStorage.zip@20261021T221100+1000!GasBBActualFlowStorage.CSV"
            ),
            YearMonth::new(2026, 10)
        );
    }

    #[test]
    fn wide_headers_unpivot_where_a_feed_says() {
        let gbb = feed("gbb").unwrap();
        let dated = gbb.unpivot_of("FORECAST_UTILISATION").unwrap();
        assert_eq!(
            dated,
            Unpivot::Dated {
                date: "FORECAST_DATE",
                value: "QUANTITY"
            }
        );
        assert_eq!(dated.columns(), ("FORECAST_DATE", "QUANTITY"));
        assert_eq!(gbb.unpivot_of("ACTUAL_FLOW_STORAGE"), None);
        assert_eq!(
            feed("sttm").unwrap().unpivot_of("FORECAST_UTILISATION"),
            None
        );
        let emi = feed("emi").unwrap();
        let periods = emi.unpivot_of("GENERATION_MD").unwrap();
        assert_eq!(periods, TRADING_PERIODS);
        assert_eq!(periods.columns(), ("TRADING_PERIOD", "QUANTITY"));
        assert_eq!(emi.unpivot_of("FINAL_ENERGY_PRICES"), None);
    }

    #[test]
    fn ignored_folders() {
        assert!(feed("reports").unwrap().ignores("vicgas"));
        assert!(!feed("reports").unwrap().ignores("Dispatch_SCADA"));
        assert!(feed("wemde").unwrap().ignores("current"));
    }
}
