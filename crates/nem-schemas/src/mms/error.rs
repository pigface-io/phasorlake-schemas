//! Errors of scanning and sinking MMS rows.

/// Where in the file something happened.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Pos {
    /// 1-based line.
    pub line: u64,
    /// 0-based byte offset of the line start.
    pub byte_offset: u64,
}

/// What is wrong with a row. A row of the wrong structure for its block
/// (field count, prefix, too short) is quarantined and the scan goes on.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RowError {
    /// More non-empty fields than the I row declared.
    #[error("The D row has {observed} fields but the I row declared {declared}.")]
    FieldCount {
        /// Declared data columns.
        declared: usize,
        /// Observed data fields.
        observed: usize,
    },
    /// The D row's report prefix differs from its block's I row.
    #[error(
        "The D row belongs to ({report_type}, {report_subtype}) version {version} but the current block is ({block_type}, {block_subtype}) version {block_version}."
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
    #[error(
        "The D row has only {0} fields; a row starts with D, report type, subtype and version."
    )]
    TooShort(usize),
    /// A line that starts with neither C, I nor D.
    #[error("The row starts with {0:?}; a row of an MMS file starts with C, I or D.")]
    UnknownRecord(char),
}

/// What a sink returns from `row`: a bad row (quarantined, scan continues)
/// or a sink failure (the scan ends).
#[derive(Debug, thiserror::Error)]
pub enum RowFailure {
    /// The row is bad. Boxed: errors are rare and the hot path's `Result`
    /// must stay small.
    #[error(transparent)]
    Row(Box<RowError>),
    /// The sink failed.
    #[error(transparent)]
    Sink(Box<SinkError>),
}

impl From<RowError> for RowFailure {
    fn from(e: RowError) -> Self {
        Self::Row(Box::new(e))
    }
}

impl From<Box<RowError>> for RowFailure {
    fn from(e: Box<RowError>) -> Self {
        Self::Row(e)
    }
}

impl From<SinkError> for RowFailure {
    fn from(e: SinkError) -> Self {
        Self::Sink(Box::new(e))
    }
}

/// A failure of the thing rows are delivered to; ends the scan.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SinkError {
    /// The sink refused the block its I row opens, or a row of it; the
    /// message says why.
    #[error("The sink refused the block: {0}")]
    Refused(String),
}

/// A failure that ends the scan of one file.
#[derive(Debug, thiserror::Error)]
pub enum ScanError {
    /// Reading the input failed.
    #[error("Reading the CSV failed after {bytes} bytes.")]
    Io {
        /// Bytes read before the failure.
        bytes: u64,
        /// Cause.
        #[source]
        source: std::io::Error,
    },
    /// One line exceeded the limit.
    #[error(
        "Line {line} is longer than {limit} bytes; an MMS row never is, so the file is not a CSV or is corrupt."
    )]
    LineTooLong {
        /// Line.
        line: u64,
        /// Limit.
        limit: usize,
    },
    /// A D row before any I row.
    #[error("A D row appears at line {line} before any I row has declared the columns.")]
    DataBeforeHeader {
        /// Line.
        line: u64,
    },
    /// An I row that could not be read.
    #[error("The I row at line {line} is malformed: {message}")]
    BadHeader {
        /// Line.
        line: u64,
        /// What is wrong.
        message: String,
    },
    /// Too many rows quarantined.
    #[error(
        "{bad} row(s) could not be read, more than the limit of {limit}, so the file is refused. The first is line {line}: {first}"
    )]
    TooManyBadRows {
        /// Bad rows seen.
        bad: u64,
        /// Limit.
        limit: u64,
        /// Line of the first bad row.
        line: u64,
        /// What was wrong with the first bad row.
        first: Box<RowError>,
    },
    /// The sink failed.
    #[error(transparent)]
    Sink(#[from] SinkError),
    /// The sink asked to stop (fail-fast).
    #[error("The scan stopped at line {line} because the run is failing fast.")]
    Stopped {
        /// Line.
        line: u64,
    },
}

impl ScanError {
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
    fn row_errors_say_what_is_wrong() {
        let e = RowError::FieldCount {
            declared: 7,
            observed: 8,
        };
        assert_eq!(
            e.to_string(),
            "The D row has 8 fields but the I row declared 7."
        );
        let e = RowError::BlockMismatch {
            report_type: "A".into(),
            report_subtype: "B".into(),
            version: "1".into(),
            block_type: "C".into(),
            block_subtype: "D".into(),
            block_version: 2,
        };
        assert_eq!(
            e.to_string(),
            "The D row belongs to (A, B) version 1 but the current block is (C, D) version 2."
        );
        assert!(RowError::TooShort(2).to_string().contains("only 2 fields"));
        assert!(RowError::UnknownRecord('x').to_string().contains("'x'"));
        let boxed: RowFailure = Box::new(RowError::TooShort(1)).into();
        assert!(matches!(boxed, RowFailure::Row(_)));
        let sink: RowFailure = SinkError::Refused("no".into()).into();
        assert_eq!(sink.to_string(), "The sink refused the block: no");
    }

    #[test]
    fn scan_errors_carry_lines() {
        assert_eq!(ScanError::LineTooLong { line: 3, limit: 1 }.line(), Some(3));
        assert_eq!(ScanError::DataBeforeHeader { line: 2 }.line(), Some(2));
        let e = ScanError::BadHeader {
            line: 2,
            message: "m".into(),
        };
        assert_eq!(e.line(), Some(2));
        assert_eq!(e.to_string(), "The I row at line 2 is malformed: m");
        // A refusal says exactly what its first bad row had wrong.
        let refused = ScanError::TooManyBadRows {
            bad: 1,
            limit: 0,
            line: 133,
            first: Box::new(RowError::UnknownRecord('x')),
        };
        assert_eq!(refused.line(), Some(133));
        assert!(
            refused.to_string().contains("line 133")
                && refused.to_string().contains("starts with 'x'"),
            "{refused}"
        );
        assert_eq!(ScanError::Stopped { line: 9 }.line(), Some(9));
        let e = ScanError::Io {
            bytes: 10,
            source: std::io::Error::other("boom"),
        };
        assert_eq!(e.line(), None);
        assert_eq!(
            std::error::Error::source(&e).map(ToString::to_string),
            Some("boom".to_owned())
        );
        let e = ScanError::from(SinkError::Refused("No table.".into()));
        assert_eq!(e.line(), None);
        assert_eq!(e.to_string(), "The sink refused the block: No table.");
    }
}
