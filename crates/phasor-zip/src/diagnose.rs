//! The diagnostics of this crate's errors, with the codes of the zip
//! category.

use phasor_diag::{Category, Diagnose, Diagnostic};

use crate::zipdir::DirError;
use crate::{ArchiveError, WalkError};

/// An archive that could not be walked, with the file and entry it names.
impl Diagnose for ArchiveError {
    fn diagnostic(&self) -> Diagnostic {
        let msg = self.to_string();
        match self {
            Self::Open { path, .. } => {
                Diagnostic::error("zip.open_failed", Category::ZIP, msg).with_file(path.clone())
            }
            Self::Entry { path, entry, .. } => {
                Diagnostic::error("zip.bad_entry", Category::ZIP, msg)
                    .with_file(path.clone())
                    .with_entry(entry.clone())
            }
            Self::TooDeep { path, entry, .. } => {
                Diagnostic::error("zip.nesting_too_deep", Category::ZIP, msg)
                    .with_file(path.clone())
                    .with_entry(entry.clone())
            }
            Self::NoEntry { path, .. } => {
                Diagnostic::error("zip.no_entry", Category::ZIP, msg).with_file(path.clone())
            }
            Self::Spool {
                path,
                entry,
                source,
                ..
            } => Diagnostic::error("zip.spool_failed", Category::ZIP, msg)
                .with_file(path.clone())
                .with_entry(entry.clone())
                .caused_by(source),
        }
    }
}

/// A walk that ended: the archive's failure, or the callback's by the
/// callback.
impl<E: Diagnose> Diagnose for WalkError<E> {
    fn diagnostic(&self) -> Diagnostic {
        match self {
            Self::Archive(a) => a.diagnostic(),
            Self::Callback(e) => e.diagnostic(),
        }
    }
}

/// A central directory that could not be read.
impl Diagnose for DirError {
    fn diagnostic(&self) -> Diagnostic {
        let code = match self {
            Self::NotAZip(_) => "zip.not_a_zip",
            Self::Damaged(_) | Self::Short { .. } => "zip.bad_directory",
        };
        Diagnostic::error(code, Category::ZIP, self.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn archive_errors_have_codes() {
        let codes: Vec<String> = [
            ArchiveError::Open {
                path: "a.zip".into(),
                message: "m".into(),
            },
            ArchiveError::Entry {
                path: "a.zip".into(),
                entry: "e".into(),
                message: "m".into(),
            },
            ArchiveError::TooDeep {
                path: "a.zip".into(),
                entry: "e".into(),
                depth: 3,
            },
            ArchiveError::NoEntry {
                path: "a.zip".into(),
                wanted: ".CSV".into(),
            },
            ArchiveError::Spool {
                path: "a.zip".into(),
                entry: "e".into(),
                tmp: "/t".into(),
                source: std::io::Error::other("full"),
            },
        ]
        .iter()
        .map(|e| e.diagnostic().code.into_owned())
        .collect();
        assert_eq!(
            codes,
            [
                "zip.open_failed",
                "zip.bad_entry",
                "zip.nesting_too_deep",
                "zip.no_entry",
                "zip.spool_failed"
            ]
        );
        let spool = ArchiveError::Spool {
            path: "a.zip".into(),
            entry: "e".into(),
            tmp: "/t".into(),
            source: std::io::Error::other("full"),
        }
        .diagnostic();
        assert_eq!((spool.entry.as_deref(), spool.cause.len()), (Some("e"), 1));
        let walk: WalkError<ArchiveError> = WalkError::Archive(ArchiveError::NoEntry {
            path: "a.zip".into(),
            wanted: ".CSV".into(),
        });
        assert_eq!(walk.diagnostic().code, "zip.no_entry");
        let walk: WalkError<DirError> = WalkError::Callback(DirError::NotAZip(4));
        assert_eq!(walk.diagnostic().code, "zip.not_a_zip");
        assert_eq!(
            DirError::Damaged("x").diagnostic().code,
            "zip.bad_directory"
        );
        assert_eq!(
            DirError::Short { wanted: 2, got: 1 }.diagnostic().code,
            "zip.bad_directory"
        );
    }
}
