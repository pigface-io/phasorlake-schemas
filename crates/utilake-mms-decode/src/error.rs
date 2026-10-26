//! Errors of scanning MMS rows. A row of the wrong structure for its block
//! is refused and the scan goes on; a sink's own refusals and failures are
//! its own types, carried through here untouched.

pub use utilake_decode::{Never, Pos};

/// What is wrong with a row: its structure, which the scanner sees, or
/// its values, which the sink sees (`Q`, the sink's [`crate::Sink::Reject`]).
/// Such a row is quarantined and the scan goes on.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RowError<Q> {
    /// More non-empty fields than the I row declared.
    #[error("the D row has {observed} fields but the I row declared {declared}")]
    FieldCount {
        /// Declared data columns.
        declared: usize,
        /// Observed data fields.
        observed: usize,
    },
    /// The D row's report prefix differs from its block's I row.
    #[error(
        "the D row belongs to ({report_type}, {report_subtype}) version {version} but the current block is ({block_type}, {block_subtype}) version {block_version}"
    )]
    BlockMismatch {
        /// Row's report type.
        report_type: String,
        /// Row's subtype.
        report_subtype: String,
        /// Row's version.
        version: String,
        /// Block's report type.
        block_type: String,
        /// Block's subtype.
        block_subtype: String,
        /// Block's version.
        block_version: u16,
    },
    /// Fewer than four fields: no report prefix.
    #[error("the D row has only {0} fields; a row starts with D, report type, subtype and version")]
    TooShort(usize),
    /// A line that starts with neither C, I nor D.
    #[error("the row starts with {0:?}; a row of an MMS file starts with C, I or D")]
    UnknownRecord(char),
    /// A value the sink refused.
    #[error("{0}")]
    Value(Q),
}

/// What a sink returns from `row`: a bad row (quarantined, scan continues)
/// or a sink failure (the scan ends). Boxed: errors are rare and the hot
/// path's `Result` must stay small.
#[derive(Debug, thiserror::Error)]
pub enum RowFailure<Q, F> {
    /// The row is bad.
    #[error(transparent)]
    Row(Box<RowError<Q>>),
    /// The sink failed, for example a batch could not be written.
    #[error("{0}")]
    Sink(Box<F>),
}

impl<Q, F> RowFailure<Q, F> {
    /// A row the sink refuses for its values.
    pub fn reject(q: Q) -> Self {
        Self::Row(Box::new(RowError::Value(q)))
    }

    /// A failure that ends the scan.
    pub fn fatal(f: F) -> Self {
        Self::Sink(Box::new(f))
    }
}

impl<Q, F> From<RowError<Q>> for RowFailure<Q, F> {
    fn from(e: RowError<Q>) -> Self {
        Self::Row(Box::new(e))
    }
}

impl<Q, F> From<Box<RowError<Q>>> for RowFailure<Q, F> {
    fn from(e: Box<RowError<Q>>) -> Self {
        Self::Row(e)
    }
}

/// A failure that ends the scan of one file; `F` is the sink's
/// [`crate::Sink::Fatal`].
#[derive(Debug, thiserror::Error)]
pub enum ScanError<Q, F> {
    /// Reading the input failed.
    #[error("reading the CSV failed after {bytes} bytes")]
    Io {
        /// Bytes read before the failure.
        bytes: u64,
        /// Cause.
        #[source]
        source: std::io::Error,
    },
    /// One line exceeded the limit.
    #[error(
        "line {line} is longer than {limit} bytes; an MMS row never is, so the file is not a CSV or is corrupt"
    )]
    LineTooLong {
        /// Line.
        line: u64,
        /// Limit.
        limit: usize,
    },
    /// A D row before any I row.
    #[error("a D row appears at line {line} before any I row has declared the columns")]
    DataBeforeHeader {
        /// Line.
        line: u64,
    },
    /// An I row that could not be read.
    #[error("the I row at line {line} is malformed: {message}")]
    BadHeader {
        /// Line.
        line: u64,
        /// What is wrong.
        message: String,
    },
    /// Too many rows quarantined.
    #[error(
        "{bad} row(s) could not be read, more than the limit of {limit}, so the file is refused; the first is line {line}: {first}"
    )]
    TooManyBadRows {
        /// Bad rows seen.
        bad: u64,
        /// Limit.
        limit: u64,
        /// Line of the first bad row.
        line: u64,
        /// What was wrong with the first bad row.
        first: Box<RowError<Q>>,
    },
    /// The sink failed.
    #[error("{0}")]
    Sink(F),
    /// The sink asked to stop (fail-fast).
    #[error("the scan stopped at line {line} because the run is failing fast")]
    Stopped {
        /// Line.
        line: u64,
    },
}

impl<Q, F> From<F> for ScanError<Q, F> {
    fn from(f: F) -> Self {
        Self::Sink(f)
    }
}

impl<Q, F> ScanError<Q, F> {
    /// The line the error refers to, when it has one.
    pub fn line(&self) -> Option<u64> {
        match self {
            Self::LineTooLong { line, .. }
            | Self::DataBeforeHeader { line }
            | Self::BadHeader { line, .. }
            | Self::TooManyBadRows { line, .. }
            | Self::Stopped { line } => Some(*line),
            Self::Io { .. } | Self::Sink(_) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_error_says_where_and_what() {
        let short: RowError<Never> = RowError::TooShort(2);
        assert_eq!(
            short.to_string(),
            "the D row has only 2 fields; a row starts with D, report type, subtype and version"
        );
        let value: RowError<String> = RowError::Value("odd".into());
        assert_eq!(value.to_string(), "odd");
        let f: RowFailure<String, String> = RowFailure::reject("odd".into());
        assert_eq!(f.to_string(), "odd");
        let f: RowFailure<String, String> = RowFailure::fatal("down".into());
        assert_eq!(f.to_string(), "down");
        let f: RowFailure<Never, Never> = RowError::UnknownRecord('x').into();
        assert!(f.to_string().starts_with("the row starts with 'x'"));
        let f: RowFailure<Never, Never> = Box::new(RowError::<Never>::TooShort(1)).into();
        assert!(matches!(f, RowFailure::Row(_)));
        let e: ScanError<Never, String> = "down".to_owned().into();
        assert_eq!((e.to_string(), e.line()), ("down".into(), None));
        let io: ScanError<Never, Never> = ScanError::Io {
            bytes: 3,
            source: std::io::Error::other("x"),
        };
        assert_eq!(io.line(), None);
        assert!(io.to_string().contains("after 3 bytes"));
        let errs: [ScanError<Never, Never>; 5] = [
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
                first: Box::new(RowError::TooShort(0)),
            },
            ScanError::Stopped { line: 5 },
        ];
        let lines: Vec<Option<u64>> = errs.iter().map(ScanError::line).collect();
        assert_eq!(lines, [Some(1), Some(2), Some(3), Some(4), Some(5)]);
        assert!(errs[3].to_string().contains("; the first is line 4"));
        let mismatch: RowError<Never> = RowError::BlockMismatch {
            report_type: "A".into(),
            report_subtype: "B".into(),
            version: "1".into(),
            block_type: "C".into(),
            block_subtype: "D".into(),
            block_version: 2,
        };
        assert!(mismatch.to_string().contains("(A, B) version 1"));
        let count: RowError<Never> = RowError::FieldCount {
            declared: 1,
            observed: 2,
        };
        assert!(
            count
                .to_string()
                .contains("2 fields but the I row declared 1")
        );
    }
}
