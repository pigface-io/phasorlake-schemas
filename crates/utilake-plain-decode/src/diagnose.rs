//! The diagnostics of this crate's errors: a file's own faults with the
//! codes of the CSV and JSON categories, a sink's failure by the sink.

use std::fmt::Display;

use utilake_diag::{Category, Diagnose, Diagnostic};

use crate::{JsonError, PlainError, PlainRowError};

/// A row refused for its structure.
impl Diagnose for PlainRowError {
    fn diagnostic(&self) -> Diagnostic {
        let Self::TooManyFields { declared, observed } = self;
        Diagnostic::error("csv.field_count_mismatch", Category::CSV, self.to_string())
            .expected_observed(format!("{declared} fields"), format!("{observed} fields"))
    }
}

/// A plain scan that ended: the file's faults with their codes and line,
/// the sink's failure by the sink.
impl<F: Diagnose + Display> Diagnose for PlainError<F> {
    fn diagnostic(&self) -> Diagnostic {
        let msg = self.to_string();
        let (code, line) = match self {
            Self::Sink(f) => return f.diagnostic(),
            Self::Io(source) => {
                return Diagnostic::error("csv.read_failed", Category::CSV, msg).caused_by(source);
            }
            Self::RecordTooLong { line, .. } => ("csv.line_too_long", Some(*line)),
            Self::HeaderMissing => ("csv.header_missing", None),
            Self::BadHeader { line, .. } => ("csv.header_malformed", Some(*line)),
            Self::TooManyBadRows { line, .. } => ("csv.too_many_bad_rows", Some(*line)),
            Self::Stopped { line } => ("csv.stopped_fail_fast", Some(*line)),
        };
        let mut d = Diagnostic::error(code, Category::CSV, msg);
        d.line = line;
        d
    }
}

/// A document that did not read: its faults with their codes, the sink's
/// failure by the sink.
impl<F: Diagnose + Display> Diagnose for JsonError<F> {
    fn diagnostic(&self) -> Diagnostic {
        let msg = self.to_string();
        match self {
            Self::Syntax(source) => {
                Diagnostic::error("json.syntax", Category::JSON, msg).caused_by(source)
            }
            Self::Shape(_) => Diagnostic::error("json.not_wemde", Category::JSON, msg),
            Self::Sink(f) => f.diagnostic(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug)]
    struct Down;
    impl Display for Down {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("down")
        }
    }
    impl Diagnose for Down {
        fn diagnostic(&self) -> Diagnostic {
            Diagnostic::error("store.write_failed", Category::STORE, "down")
        }
    }

    #[test]
    fn every_error_has_its_code() {
        let row = PlainRowError::TooManyFields {
            declared: 2,
            observed: 3,
        };
        let d = row.diagnostic();
        assert_eq!(d.code, "csv.field_count_mismatch");
        assert_eq!(d.observed.as_deref(), Some("3 fields"));
        let scans: [PlainError<Down>; 7] = [
            PlainError::Io(std::io::Error::other("gone")),
            PlainError::RecordTooLong { line: 1, limit: 2 },
            PlainError::HeaderMissing,
            PlainError::BadHeader {
                line: 2,
                column: 0,
                text: String::new(),
            },
            PlainError::TooManyBadRows {
                bad: 1,
                limit: 0,
                line: 3,
                first: row,
            },
            PlainError::Stopped { line: 4 },
            PlainError::Sink(Down),
        ];
        let got: Vec<(String, Option<u64>)> = scans
            .iter()
            .map(|e| {
                let d = e.diagnostic();
                (d.code.into_owned(), d.line)
            })
            .collect();
        let want: Vec<(String, Option<u64>)> = [
            ("csv.read_failed", None),
            ("csv.line_too_long", Some(1)),
            ("csv.header_missing", None),
            ("csv.header_malformed", Some(2)),
            ("csv.too_many_bad_rows", Some(3)),
            ("csv.stopped_fail_fast", Some(4)),
            ("store.write_failed", None),
        ]
        .iter()
        .map(|(c, l)| ((*c).to_owned(), *l))
        .collect();
        assert_eq!(got, want);
        assert_eq!(scans[0].diagnostic().cause, ["gone"]);
        let syntax = serde_json::from_str::<u8>("x").unwrap_err();
        let docs: [JsonError<Down>; 3] = [
            JsonError::Syntax(syntax),
            JsonError::Shape("no rows"),
            JsonError::Sink(Down),
        ];
        let codes: Vec<String> = docs
            .iter()
            .map(|e| e.diagnostic().code.into_owned())
            .collect();
        assert_eq!(
            codes,
            ["json.syntax", "json.not_wemde", "store.write_failed"]
        );
        assert_eq!(docs[0].diagnostic().cause.len(), 1);
    }
}
