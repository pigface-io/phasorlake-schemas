//! The diagnostics of this crate's errors.

use utilake_diag::{Category, Diagnose, Diagnostic};

use crate::{FingerprintError, Never, YearMonthError};

/// Text that is not a month.
impl Diagnose for YearMonthError {
    fn diagnostic(&self) -> Diagnostic {
        let observed = match self {
            Self::Shape(s) | Self::Month(s, _) => s.clone(),
        };
        Diagnostic::error("parse.month", Category::PARSE, self.to_string())
            .expected_observed("a month YYYY-MM", observed)
    }
}

/// Text that is not a fingerprint.
impl Diagnose for FingerprintError {
    fn diagnostic(&self) -> Diagnostic {
        Diagnostic::error("parse.fingerprint", Category::PARSE, self.to_string())
            .expected_observed("xxh3:<32 hex digits>", self.text().to_owned())
    }
}

/// What cannot fail has no diagnostic.
impl Diagnose for Never {
    fn diagnostic(&self) -> Diagnostic {
        match *self {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_error_has_its_code() {
        let d = "2024-13"
            .parse::<crate::YearMonth>()
            .unwrap_err()
            .diagnostic();
        assert_eq!(
            (d.code.as_ref(), d.observed.as_deref()),
            ("parse.month", Some("2024-13"))
        );
        let d = "x".parse::<crate::YearMonth>().unwrap_err().diagnostic();
        assert_eq!(d.observed.as_deref(), Some("x"));
        let d = "xxh3:1"
            .parse::<crate::Fingerprint>()
            .unwrap_err()
            .diagnostic();
        assert_eq!(
            (d.code.as_ref(), d.observed.as_deref()),
            ("parse.fingerprint", Some("xxh3:1"))
        );
    }
}
