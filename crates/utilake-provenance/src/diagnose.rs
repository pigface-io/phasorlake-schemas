//! The diagnostics of this crate's errors: the archive's with the codes of
//! the cache category, a text that does not read with those of parse.

use utilake_diag::{Category, Diagnose, Diagnostic};

use crate::archive::Error;
use crate::content::BadSha256;
use crate::provenance::{BadProvenance, ReadError};
use crate::stamp::BadStamp;

/// The archive could not be read or written. A record that does not read
/// is a warning: the file it describes is still there.
impl Diagnose for Error {
    fn diagnostic(&self) -> Diagnostic {
        let msg = self.to_string();
        match self {
            Self::Io { path, source } => Diagnostic::error("cache.io", Category::CACHE, msg)
                .with_file(path.clone())
                .caused_by(source),
            Self::Record { path, source } => {
                Diagnostic::warning("cache.sidecar_corrupt", Category::CACHE, msg)
                    .with_file(path.clone())
                    .caused_by(source)
            }
        }
    }
}

/// A record that does not read.
impl Diagnose for ReadError {
    fn diagnostic(&self) -> Diagnostic {
        Diagnostic::warning("cache.sidecar_corrupt", Category::CACHE, self.to_string())
    }
}

/// A record that breaks the rules of a provenance.
impl Diagnose for BadProvenance {
    fn diagnostic(&self) -> Diagnostic {
        Diagnostic::warning("cache.sidecar_corrupt", Category::CACHE, self.to_string())
    }
}

/// Text that is not a time with an offset.
impl Diagnose for BadStamp {
    fn diagnostic(&self) -> Diagnostic {
        Diagnostic::error("parse.timestamp", Category::PARSE, self.to_string())
            .expected_observed("an RFC 3339 time with an offset", self.0.clone())
    }
}

/// Text that is not a SHA-256.
impl Diagnose for BadSha256 {
    fn diagnostic(&self) -> Diagnostic {
        Diagnostic::error("parse.sha256", Category::PARSE, self.to_string())
            .expected_observed("64 hex digits", self.0.clone())
    }
}

#[cfg(test)]
mod tests {
    use utilake_diag::Severity;

    use super::*;

    #[test]
    fn every_error_has_its_code() {
        let io = Error::Io {
            path: "p".into(),
            source: std::io::Error::other("gone"),
        };
        let d = io.diagnostic();
        assert_eq!(
            (d.code.as_ref(), d.file.as_deref()),
            ("cache.io", Some("p"))
        );
        assert_eq!(d.cause, ["gone"]);
        let record = Error::Record {
            path: "r".into(),
            source: ReadError::Bad(BadProvenance::NoAddress),
        };
        let d = record.diagnostic();
        assert_eq!(d.code, "cache.sidecar_corrupt");
        assert_eq!(d.severity, Severity::Warning);
        assert_eq!(
            ReadError::Bad(BadProvenance::NoAddress).diagnostic().code,
            "cache.sidecar_corrupt"
        );
        assert_eq!(
            BadProvenance::NoAddress.diagnostic().code,
            "cache.sidecar_corrupt"
        );
        let d = BadStamp("x".into()).diagnostic();
        assert_eq!(
            (d.code.as_ref(), d.observed.as_deref()),
            ("parse.timestamp", Some("x"))
        );
        let d = BadSha256("y".into()).diagnostic();
        assert_eq!(
            (d.code.as_ref(), d.observed.as_deref()),
            ("parse.sha256", Some("y"))
        );
    }
}
