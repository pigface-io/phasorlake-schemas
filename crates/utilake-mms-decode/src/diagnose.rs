//! The diagnostics of this crate's errors: the scanner's own faults with
//! the codes of the MMS format, and a sink's refusals and failures by the
//! sink's own [`Diagnose`].

use std::fmt::Display;

use utilake_diag::{Category, Diagnose, Diagnostic};

use crate::irow::IRowError;
use crate::shape::{ShapeIdError, TypeClassError};
use crate::{RowError, RowFailure, ScanError};

/// A row the scanner or a sink refused: the scanner's own faults with
/// their codes, a value's by the value.
impl<Q: Diagnose + Display> Diagnose for RowError<Q> {
    fn diagnostic(&self) -> Diagnostic {
        let msg = self.to_string();
        match self {
            Self::FieldCount { declared, observed } => {
                Diagnostic::error("csv.field_count_mismatch", Category::CSV, msg)
                    .expected_observed(format!("{declared} fields"), format!("{observed} fields"))
            }
            Self::BlockMismatch { .. } => {
                Diagnostic::error("csv.block_mismatch", Category::CSV, msg)
            }
            Self::TooShort(_) => Diagnostic::error("csv.row_too_short", Category::CSV, msg),
            Self::UnknownRecord(_) => Diagnostic::error("csv.unknown_record", Category::CSV, msg),
            Self::Value(v) => v.diagnostic(),
        }
    }
}

/// What a sink returned from a row: the bad row's, or the sink's failure.
impl<Q: Diagnose + Display, F: Diagnose + Display> Diagnose for RowFailure<Q, F> {
    fn diagnostic(&self) -> Diagnostic {
        match self {
            Self::Row(r) => r.diagnostic(),
            Self::Sink(f) => f.diagnostic(),
        }
    }
}

/// A scan that ended: the scanner's own failures with their codes, the
/// sink's by the sink; each with the line it names.
impl<Q: Diagnose + Display, F: Diagnose + Display> Diagnose for ScanError<Q, F> {
    fn diagnostic(&self) -> Diagnostic {
        let msg = self.to_string();
        let mut d = match self {
            Self::Io { source, .. } => {
                Diagnostic::error("csv.read_failed", Category::CSV, msg).caused_by(source)
            }
            Self::LineTooLong { .. } => Diagnostic::error("csv.line_too_long", Category::CSV, msg),
            Self::DataBeforeHeader { .. } => {
                Diagnostic::error("csv.data_before_header", Category::CSV, msg)
            }
            Self::BadHeader { .. } => Diagnostic::error("csv.header_malformed", Category::CSV, msg),
            Self::TooManyBadRows { first, .. } => {
                // The refusal says exactly what the first bad row had
                // wrong: its column, what was expected and what was there.
                let row = first.diagnostic();
                let mut d = Diagnostic::error("csv.too_many_bad_rows", Category::CSV, msg);
                d.column = row.column;
                d.expected = row.expected;
                d.observed = row.observed;
                d.cause = vec![format!("{} [{}]", row.message, row.code)];
                d
            }
            Self::Sink(e) => e.diagnostic(),
            Self::Stopped { .. } => Diagnostic::error("csv.stopped_fail_fast", Category::CSV, msg),
        };
        if let Some(l) = self.line() {
            d.line = Some(l);
        }
        d
    }
}

/// An I row that does not read.
impl Diagnose for IRowError {
    fn diagnostic(&self) -> Diagnostic {
        Diagnostic::error(
            "csv.header_malformed",
            Category::CSV,
            format!("the I row is malformed: {self}"),
        )
    }
}

/// Text that is not a type class.
impl Diagnose for TypeClassError {
    fn diagnostic(&self) -> Diagnostic {
        Diagnostic::error("parse.type_class", Category::PARSE, self.to_string())
    }
}

/// Text that is not a shape id.
impl Diagnose for ShapeIdError {
    fn diagnostic(&self) -> Diagnostic {
        Diagnostic::error("parse.shape_id", Category::PARSE, self.to_string())
    }
}

#[cfg(test)]
mod tests {
    use utilake_diag::Severity;

    use super::*;
    use crate::Never;

    /// A sink's own refusal or failure, diagnosed by the sink.
    #[derive(Debug)]
    struct Mine(&'static str);
    impl Display for Mine {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str(self.0)
        }
    }
    impl Diagnose for Mine {
        fn diagnostic(&self) -> Diagnostic {
            Diagnostic::error("parse.integer", Category::PARSE, self.0)
                .with_column("C")
                .expected_observed("a whole number", "x")
        }
    }

    fn code(d: &impl Diagnose) -> String {
        d.diagnostic().code.into_owned()
    }

    #[test]
    fn rows_have_the_codes_of_the_format() {
        let rows: [RowError<Mine>; 5] = [
            RowError::FieldCount {
                declared: 1,
                observed: 2,
            },
            RowError::BlockMismatch {
                report_type: "A".into(),
                report_subtype: "B".into(),
                version: "1".into(),
                block_type: "C".into(),
                block_subtype: "D".into(),
                block_version: 2,
            },
            RowError::TooShort(2),
            RowError::UnknownRecord('x'),
            RowError::Value(Mine("bad")),
        ];
        let codes: Vec<String> = rows.iter().map(code).collect();
        assert_eq!(
            codes,
            [
                "csv.field_count_mismatch",
                "csv.block_mismatch",
                "csv.row_too_short",
                "csv.unknown_record",
                "parse.integer"
            ]
        );
        let d = rows[0].diagnostic();
        assert_eq!(d.expected.as_deref(), Some("1 fields"));
        assert_eq!(d.severity, Severity::Error);
        let f: RowFailure<Mine, Mine> = RowFailure::reject(Mine("q"));
        assert_eq!(code(&f), "parse.integer");
        let f: RowFailure<Never, Mine> = RowFailure::fatal(Mine("f"));
        assert_eq!(f.diagnostic().message, "f");
    }

    #[test]
    fn scans_have_the_codes_of_the_format_and_their_line() {
        let scans: [ScanError<Mine, Mine>; 7] = [
            ScanError::Io {
                bytes: 3,
                source: std::io::Error::other("gone"),
            },
            ScanError::LineTooLong { line: 1, limit: 2 },
            ScanError::DataBeforeHeader { line: 2 },
            ScanError::BadHeader {
                line: 3,
                message: "m".into(),
            },
            ScanError::TooManyBadRows {
                bad: 1,
                limit: 0,
                line: 4,
                first: Box::new(RowError::Value(Mine("bad value"))),
            },
            ScanError::Sink(Mine("down")),
            ScanError::Stopped { line: 5 },
        ];
        let got: Vec<(String, Option<u64>)> = scans
            .iter()
            .map(|e| {
                let d = e.diagnostic();
                (d.code.into_owned(), d.line)
            })
            .collect();
        let want = [
            ("csv.read_failed", None),
            ("csv.line_too_long", Some(1)),
            ("csv.data_before_header", Some(2)),
            ("csv.header_malformed", Some(3)),
            ("csv.too_many_bad_rows", Some(4)),
            ("parse.integer", None),
            ("csv.stopped_fail_fast", Some(5)),
        ];
        let want: Vec<(String, Option<u64>)> =
            want.iter().map(|(c, l)| ((*c).to_owned(), *l)).collect();
        assert_eq!(got, want);
        assert_eq!(scans[0].diagnostic().cause, ["gone"]);
        let many = scans[4].diagnostic();
        assert_eq!(many.column.as_deref(), Some("C"));
        assert_eq!(many.expected.as_deref(), Some("a whole number"));
        assert_eq!(many.observed.as_deref(), Some("x"));
        assert_eq!(many.cause, ["bad value [parse.integer]"]);
    }

    #[test]
    fn texts_that_do_not_read_have_codes() {
        assert_eq!(code(&IRowError::TooShort(2)), "csv.header_malformed");
        assert!(
            IRowError::EmptyColumn(4)
                .diagnostic()
                .message
                .starts_with("the I row is malformed: ")
        );
        let e = "x".parse::<crate::TypeClass>().unwrap_err();
        assert_eq!(code(&e), "parse.type_class");
        let e = "x".parse::<crate::ShapeId>().unwrap_err();
        assert_eq!(code(&e), "parse.shape_id");
    }
}
