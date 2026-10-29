//! The diagnostics of this crate's errors, with stable codes: the network,
//! listing and cache faults of a fetch under the categories every
//! repository shares, the rest under this repository's own.

use phasor_diag::{Category, Diagnose, Diagnostic};

use crate::catalog::CatalogError;
use crate::census::CensusError;
use crate::ctl::CtlError;
use crate::ddl::DdlError;
use crate::export::ExportError;
use crate::fetch::FetchError;
use crate::fold::FoldError;
use crate::history::HistoryError;
use crate::models::ModelError;
use crate::published::PublishedError;
use crate::sources::SourcesError;
use crate::sql::SqlError;

/// A month's census: reading and writing it, and the directory it reads.
pub const CENSUS: Category = Category::new("census");
/// The hand-kept table declarations under `tables/`.
pub const CATALOG: Category = Category::new("catalog");
/// The data-model releases and the DDL they carry.
pub const MODELS: Category = Category::new("models");
/// A table's history: folding the census into it, and its promises.
pub const HISTORY: Category = Category::new("history");
/// Planning a read of a published table.
pub const SQL: Category = Category::new("sql");
/// What a git revision published.
pub const PUBLISHED: Category = Category::new("published");
/// Writing the market directories.
pub const EXPORT: Category = Category::new("export");
/// AEMO's SQL*Loader control files.
pub const CTL: Category = Category::new("ctl");

/// Every category this repository's codes are under, in the order its
/// catalogue lists them: the shared ones, then its own.
pub const CATEGORIES: [&str; 22] = [
    "config",
    "net",
    "listing",
    "cache",
    "zip",
    "csv",
    "json",
    "parse",
    "schema",
    "store",
    "manifest",
    "verify",
    "query",
    "internal",
    "census",
    "catalog",
    "models",
    "history",
    "sql",
    "published",
    "export",
    "ctl",
];

impl Diagnose for CensusError {
    fn diagnostic(&self) -> Diagnostic {
        let msg = self.to_string();
        match self {
            Self::Read { path, .. } => {
                Diagnostic::error("census.read_failed", CENSUS, msg).with_file(path.clone())
            }
            Self::Write { path, source } => Diagnostic::error("census.write_failed", CENSUS, msg)
                .with_file(path.clone())
                .caused_by(source),
            Self::Empty { path } => {
                Diagnostic::error("census.empty_dir", CENSUS, msg).with_file(path.clone())
            }
        }
    }
}

impl Diagnose for CatalogError {
    fn diagnostic(&self) -> Diagnostic {
        let msg = self.to_string();
        match self {
            Self::Read { path, .. } => {
                Diagnostic::error("catalog.read_failed", CATALOG, msg).with_file(path.clone())
            }
            Self::Invalid { path, .. } => {
                Diagnostic::error("catalog.invalid", CATALOG, msg).with_file(path.clone())
            }
            Self::Duplicate { second, .. } => {
                Diagnostic::error("catalog.duplicate_pair", CATALOG, msg).with_file(second.clone())
            }
            Self::Write { path, source } => Diagnostic::error("catalog.write_failed", CATALOG, msg)
                .with_file(path.clone())
                .caused_by(source),
        }
    }
}

impl Diagnose for FetchError {
    fn diagnostic(&self) -> Diagnostic {
        let msg = self.to_string();
        match self {
            Self::Http { url, source } => Diagnostic::error("net.connect", Category::NET, msg)
                .with_url(url.clone())
                .caused_by(source),
            Self::Refused { url, status } => Diagnostic::error("net.blocked", Category::NET, msg)
                .with_url(url.clone())
                .expected_observed("200", status.to_string()),
            Self::NotFound { url } => Diagnostic::error("net.http_status", Category::NET, msg)
                .with_url(url.clone())
                .expected_observed("200", "404"),
            Self::Status { url, status } => {
                Diagnostic::error("net.http_status", Category::NET, msg)
                    .with_url(url.clone())
                    .expected_observed("200", status.to_string())
            }
            Self::MonthMissing { url, .. } => {
                Diagnostic::error("listing.month_missing", Category::LISTING, msg)
                    .with_url(url.clone())
            }
            Self::ListingParse { url } => {
                Diagnostic::error("listing.parse_failed", Category::LISTING, msg)
                    .with_url(url.clone())
            }
            Self::Write { path, source } | Self::Read { path, source } => {
                Diagnostic::error("cache.io", Category::CACHE, msg)
                    .with_file(path.clone())
                    .caused_by(source)
            }
            Self::Sources { path, .. } => {
                Diagnostic::error("census.sources_read_failed", CENSUS, msg).with_file(path.clone())
            }
        }
    }
}

impl Diagnose for ModelError {
    fn diagnostic(&self) -> Diagnostic {
        let msg = self.to_string();
        match self {
            Self::Zip { path, .. } => {
                Diagnostic::error("models.zip_unreadable", MODELS, msg).with_file(path.clone())
            }
            Self::Read { path, .. } => {
                Diagnostic::error("models.read_failed", MODELS, msg).with_file(path.clone())
            }
            Self::Write { path, source } => Diagnostic::error("models.write_failed", MODELS, msg)
                .with_file(path.clone())
                .caused_by(source),
        }
    }
}

impl Diagnose for DdlError {
    fn diagnostic(&self) -> Diagnostic {
        Diagnostic::error("models.ddl_statement", MODELS, self.to_string())
            .with_sample(self.sample.as_bytes())
    }
}

impl Diagnose for FoldError {
    fn diagnostic(&self) -> Diagnostic {
        let msg = self.to_string();
        match self {
            Self::Census(e) => e.diagnostic(),
            Self::Models(e) => e.diagnostic(),
            Self::History(e) => e.diagnostic(),
            Self::Published { path, .. } => {
                Diagnostic::error("history.published_unreadable", HISTORY, msg)
                    .with_file(path.clone())
            }
            Self::RenameUndecided { .. } => {
                Diagnostic::error("history.rename_undecided", HISTORY, msg)
            }
            Self::Write { path, source } => Diagnostic::error("history.write_failed", HISTORY, msg)
                .with_file(path.clone())
                .caused_by(source),
        }
    }
}

impl Diagnose for HistoryError {
    fn diagnostic(&self) -> Diagnostic {
        let msg = self.to_string();
        match self {
            Self::Override {
                table,
                column,
                file,
                ..
            } => Diagnostic::error("history.override_too_narrow", HISTORY, msg)
                .with_table(table.clone())
                .with_column(column.clone())
                .with_file(file.clone()),
            Self::Unresolved { table, file, .. } => {
                Diagnostic::error("history.unresolved", HISTORY, msg)
                    .with_table(table.clone())
                    .with_file(file.clone())
            }
            Self::RenameOverlap { table, file, .. } => {
                Diagnostic::error("history.rename_overlap", HISTORY, msg)
                    .with_table(table.clone())
                    .with_file(file.clone())
            }
            Self::Broken { table, .. } => {
                Diagnostic::error("history.breaks_published", HISTORY, msg)
                    .with_table(table.clone())
            }
        }
    }
}

impl Diagnose for SqlError {
    fn diagnostic(&self) -> Diagnostic {
        let msg = self.to_string();
        match self {
            Self::UnknownTable { table, .. } => {
                Diagnostic::error("sql.unknown_table", SQL, msg).with_table(table.clone())
            }
            Self::Metadata { path, .. } => {
                Diagnostic::error("sql.metadata_unreadable", SQL, msg).with_file(path.clone())
            }
            Self::Files { path, .. } => {
                Diagnostic::error("sql.files_unreadable", SQL, msg).with_file(path.clone())
            }
            Self::Time(text) => Diagnostic::error("sql.bad_time", SQL, msg)
                .expected_observed("an RFC 3339 time or a day (YYYY-MM-DD)", text.clone()),
            Self::Empty { .. } => Diagnostic::error("sql.empty_interval", SQL, msg),
            Self::Plan { table, source } => source.diagnostic().with_table(table.clone()),
        }
    }
}

impl Diagnose for PublishedError {
    fn diagnostic(&self) -> Diagnostic {
        let msg = self.to_string();
        match self {
            Self::Git { path, source, .. } => {
                Diagnostic::error("published.git_failed", PUBLISHED, msg)
                    .with_file(path.clone())
                    .caused_by(source)
            }
            Self::Read { path, source } => {
                Diagnostic::error("published.read_failed", PUBLISHED, msg)
                    .with_file(path.clone())
                    .caused_by(source)
            }
            Self::Table { path, .. } => {
                Diagnostic::error("published.no_history", PUBLISHED, msg).with_file(path.clone())
            }
        }
    }
}

impl Diagnose for SourcesError {
    fn diagnostic(&self) -> Diagnostic {
        match self {
            Self::Census(e) => e.diagnostic(),
            Self::Write(e) => e.diagnostic(),
        }
    }
}

impl Diagnose for ExportError {
    fn diagnostic(&self) -> Diagnostic {
        let msg = self.to_string();
        match self {
            Self::Catalog(e) => e.diagnostic(),
            Self::Models(e) => e.diagnostic(),
            Self::Baseline(e) => e.diagnostic(),
            Self::Census(e) => e.diagnostic(),
            Self::History { path, .. } => {
                Diagnostic::error("export.history_unreadable", EXPORT, msg).with_file(path.clone())
            }
            Self::Publish { table, source } => source.diagnostic().with_table(table.clone()),
            Self::Io { path, source } => Diagnostic::error("export.io", EXPORT, msg)
                .with_file(path.clone())
                .caused_by(source),
        }
    }
}

impl Diagnose for CtlError {
    fn diagnostic(&self) -> Diagnostic {
        let mut d = Diagnostic::error("ctl.parse", CTL, self.to_string())
            .with_file(self.path.clone())
            .with_sample(self.sample.as_bytes());
        d.line = Some(self.line);
        d
    }
}

#[cfg(test)]
mod tests {
    use phasor_iceberg::{IcebergType, Market, PlanError, PublishError};

    use super::*;
    use crate::month::YearMonth;

    fn io() -> std::io::Error {
        std::io::Error::other("gone")
    }

    fn codes<E: Diagnose>(errors: &[E]) -> Vec<String> {
        errors
            .iter()
            .map(|e| e.diagnostic().code.into_owned())
            .collect()
    }

    fn census() -> CensusError {
        CensusError::Empty { path: "d".into() }
    }

    #[test]
    fn the_census_catalog_and_models_have_codes() {
        let e = [
            CensusError::Read {
                path: "p".into(),
                message: "m".into(),
            },
            CensusError::Write {
                path: "p".into(),
                source: io(),
            },
            census(),
        ];
        assert_eq!(
            codes(&e),
            [
                "census.read_failed",
                "census.write_failed",
                "census.empty_dir"
            ]
        );
        let e = [
            CatalogError::Read {
                path: "p".into(),
                message: "m".into(),
            },
            CatalogError::Invalid {
                path: "p".into(),
                message: "m".into(),
            },
            CatalogError::Duplicate {
                pair: "A B".into(),
                first: "a".into(),
                second: "b".into(),
            },
            CatalogError::Write {
                path: "p".into(),
                source: io(),
            },
        ];
        assert_eq!(
            codes(&e),
            [
                "catalog.read_failed",
                "catalog.invalid",
                "catalog.duplicate_pair",
                "catalog.write_failed"
            ]
        );
        let e = [
            ModelError::Zip {
                path: "p".into(),
                message: "m".into(),
            },
            ModelError::Read {
                path: "p".into(),
                message: "m".into(),
            },
            ModelError::Write {
                path: "p".into(),
                source: io(),
            },
        ];
        assert_eq!(
            codes(&e),
            [
                "models.zip_unreadable",
                "models.read_failed",
                "models.write_failed"
            ]
        );
        let d = DdlError {
            index: 1,
            reason: "r".into(),
            sample: "create".into(),
        }
        .diagnostic();
        assert_eq!(d.code, "models.ddl_statement");
        assert_eq!(d.sample.map(|s| s.text).as_deref(), Some("create"));
        let d = CtlError {
            path: "c.ctl".into(),
            line: 3,
            col: 4,
            message: "m".into(),
            sample: "s".into(),
        }
        .diagnostic();
        assert_eq!((d.code.as_ref(), d.line), ("ctl.parse", Some(3)));
    }

    #[test]
    fn a_fetch_has_the_shared_codes() {
        let e = [
            FetchError::Http {
                url: "u".into(),
                source: io(),
            },
            FetchError::Refused {
                url: "u".into(),
                status: 429,
            },
            FetchError::NotFound { url: "u".into() },
            FetchError::Status {
                url: "u".into(),
                status: 500,
            },
            FetchError::MonthMissing {
                month: YearMonth::new(2024, 1).unwrap(),
                url: "u".into(),
                available: "a".into(),
            },
            FetchError::ListingParse { url: "u".into() },
            FetchError::Write {
                path: "p".into(),
                source: io(),
            },
            FetchError::Read {
                path: "p".into(),
                source: io(),
            },
            FetchError::Sources {
                path: "p".into(),
                message: "m".into(),
            },
        ];
        assert_eq!(
            codes(&e),
            [
                "net.connect",
                "net.blocked",
                "net.http_status",
                "net.http_status",
                "listing.month_missing",
                "listing.parse_failed",
                "cache.io",
                "cache.io",
                "census.sources_read_failed"
            ]
        );
        assert_eq!(e[1].diagnostic().observed.as_deref(), Some("429"));
        let e = [
            SourcesError::Census(census()),
            SourcesError::Write(FetchError::NotFound { url: "u".into() }),
        ];
        assert_eq!(codes(&e), ["census.empty_dir", "net.http_status"]);
    }

    #[test]
    fn a_history_and_its_fold_have_codes() {
        let history = || HistoryError::Broken {
            table: "T".into(),
            violations: Vec::new(),
        };
        let e = [
            HistoryError::Override {
                table: "T".into(),
                column: "C".into(),
                ty: IcebergType::Long,
                file: "f".into(),
            },
            HistoryError::Unresolved {
                table: "T".into(),
                file: "f".into(),
                schema: 1,
                resolved: None,
            },
            HistoryError::RenameOverlap {
                table: "T".into(),
                from: "A".into(),
                to: "B".into(),
                file: "f".into(),
            },
            history(),
        ];
        assert_eq!(
            codes(&e),
            [
                "history.override_too_narrow",
                "history.unresolved",
                "history.rename_overlap",
                "history.breaks_published"
            ]
        );
        let e = [
            FoldError::Census(census()),
            FoldError::Models(ModelError::Zip {
                path: "p".into(),
                message: "m".into(),
            }),
            FoldError::History(history()),
            FoldError::Published {
                path: "p".into(),
                message: "m".into(),
            },
            FoldError::RenameUndecided {
                undecided: Vec::new(),
            },
            FoldError::Write {
                path: "p".into(),
                source: io(),
            },
        ];
        assert_eq!(
            codes(&e),
            [
                "census.empty_dir",
                "models.zip_unreadable",
                "history.breaks_published",
                "history.published_unreadable",
                "history.rename_undecided",
                "history.write_failed"
            ]
        );
    }

    #[test]
    fn reads_publications_and_exports_have_codes() {
        let e = [
            SqlError::UnknownTable {
                market: Market::Nem,
                table: "T".into(),
            },
            SqlError::Metadata {
                path: "p".into(),
                message: "m".into(),
            },
            SqlError::Files {
                path: "p".into(),
                message: "m".into(),
            },
            SqlError::Time("x".into()),
            SqlError::Empty {
                from: "b".into(),
                to: "a".into(),
            },
            SqlError::Plan {
                table: "T".into(),
                source: PlanError::UnknownSchema(9),
            },
        ];
        assert_eq!(
            codes(&e),
            [
                "sql.unknown_table",
                "sql.metadata_unreadable",
                "sql.files_unreadable",
                "sql.bad_time",
                "sql.empty_interval",
                "schema.unknown_schema_id"
            ]
        );
        assert_eq!(e[5].diagnostic().table.as_deref(), Some("T"));
        let e = [
            PublishedError::Git {
                rev: "r".into(),
                path: "p".into(),
                source: io(),
            },
            PublishedError::Read {
                path: "p".into(),
                source: io(),
            },
            PublishedError::Table {
                path: "p".into(),
                at: "r".into(),
                message: "m".into(),
            },
        ];
        assert_eq!(
            codes(&e),
            [
                "published.git_failed",
                "published.read_failed",
                "published.no_history"
            ]
        );
        let e = [
            ExportError::Catalog(CatalogError::Invalid {
                path: "p".into(),
                message: "m".into(),
            }),
            ExportError::Models(ModelError::Zip {
                path: "p".into(),
                message: "m".into(),
            }),
            ExportError::Baseline(FoldError::Census(census())),
            ExportError::Census(census()),
            ExportError::History {
                path: "p".into(),
                message: "m".into(),
            },
            ExportError::Publish {
                table: "T".into(),
                source: PublishError::UnknownSchema(1),
            },
            ExportError::Io {
                path: "p".into(),
                source: io(),
            },
        ];
        assert_eq!(
            codes(&e),
            [
                "catalog.invalid",
                "models.zip_unreadable",
                "census.empty_dir",
                "census.empty_dir",
                "export.history_unreadable",
                "store.iceberg_metadata",
                "export.io"
            ]
        );
    }

    #[test]
    fn every_category_is_one_the_catalogue_lists() {
        for c in [
            CENSUS, CATALOG, MODELS, HISTORY, SQL, PUBLISHED, EXPORT, CTL,
        ]
        .iter()
        .chain(Category::COMMON.iter())
        {
            assert!(CATEGORIES.contains(&c.as_str()), "{c}");
        }
    }
}
