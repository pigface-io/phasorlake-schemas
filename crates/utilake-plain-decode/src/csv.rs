//! A header-row CSV as one block of rows.
//!
//! The first non-blank line names the columns; every line after it is a
//! row. Rows reach the sink as the MMS scanner's [`Row`], rebuilt with each
//! date field in the MMS form and exactly as many fields as the header has
//! (a short row is padded with empty fields; a long one whose extra fields
//! are empty is trimmed, otherwise it is a bad row). A byte-order mark on
//! the first line, `\r\n` line ends and quoted fields that span lines are
//! read; blank lines are skipped.

use std::fmt;
use std::io::{self, BufRead, BufReader, Read};

use memchr::memchr_iter;
use utilake_mms_decode::fields::Splitter;
pub use utilake_mms_decode::{Flow, Pos, Row};

use crate::dates;
use crate::names::column_name;

/// What a scan takes.
#[derive(Debug, Clone, Copy)]
pub struct PlainOptions<'a> {
    /// The market's offset, as `+10:00`: the one an RFC 3339 instant may
    /// carry and still be rewritten.
    pub offset: &'a str,
    /// Longest record accepted (a quoted field may span lines); longer is a
    /// corrupt file.
    pub max_record_bytes: usize,
    /// Bad rows tolerated before the file fails.
    pub max_bad_rows: u64,
}

impl Default for PlainOptions<'static> {
    fn default() -> Self {
        Self {
            offset: "+10:00",
            max_record_bytes: 64 << 20,
            max_bad_rows: 0,
        }
    }
}

/// The columns a block has: the header as written and as named.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Header {
    /// The header fields as written, trailing empty ones dropped.
    pub raw: Vec<String>,
    /// The column names, one per raw field: [`column_name`], with `_2`,
    /// `_3`... on a repeat.
    pub names: Vec<String>,
}

impl Header {
    /// The header of these raw fields. `Err(i)` names the first field that
    /// names no column.
    pub fn from_raw<'a>(raw: impl Iterator<Item = &'a str>) -> Result<Self, usize> {
        let mut raw: Vec<String> = raw.map(str::to_owned).collect();
        while raw.last().is_some_and(|r| r.trim().is_empty()) {
            raw.pop();
        }
        let mut names: Vec<String> = Vec::with_capacity(raw.len());
        for (i, r) in raw.iter().enumerate() {
            let base = column_name(r).ok_or(i)?;
            let mut name = base.clone();
            let mut n = 1;
            while names.contains(&name) {
                n += 1;
                name = format!("{base}_{n}");
            }
            names.push(name);
        }
        Ok(Self { raw, names })
    }

    /// Number of columns.
    pub fn len(&self) -> usize {
        self.names.len()
    }

    /// Whether there are no columns.
    pub fn is_empty(&self) -> bool {
        self.names.is_empty()
    }

    /// The index of the column with this name.
    pub fn index_of(&self, name: &str) -> Option<usize> {
        self.names.iter().position(|n| n == name)
    }
}

/// Receives the blocks and rows of one file.
pub trait PlainSink {
    /// What ends the scan.
    type Fatal: fmt::Debug + fmt::Display;
    /// A block begins: `array` is the JSON array's path for a WEMDE
    /// document, `None` for a CSV.
    fn header(&mut self, array: Option<&str>, header: &Header, pos: Pos)
    -> Result<(), Self::Fatal>;
    /// A row of the current block: exactly `header.len()` fields, dates in
    /// the MMS form.
    fn row(&mut self, row: &Row<'_>, pos: Pos) -> Result<(), Self::Fatal>;
    /// A row the scanner refused.
    fn bad_row(&mut self, raw: &[u8], pos: Pos, err: &PlainRowError) -> Flow;
}

/// Why a row was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PlainRowError {
    /// More non-empty fields than the header has columns.
    #[error("The row has {observed} fields but the header has {declared}.")]
    TooManyFields {
        /// Header columns.
        declared: usize,
        /// Non-empty fields observed.
        observed: usize,
    },
}

/// Why a scan ended early.
#[derive(Debug, thiserror::Error)]
pub enum PlainError<F> {
    /// Reading failed.
    #[error("Reading failed: {0}")]
    Io(#[from] io::Error),
    /// A record longer than the limit.
    #[error("Line {line} is longer than {limit} bytes.")]
    RecordTooLong {
        /// Line.
        line: u64,
        /// The limit.
        limit: usize,
    },
    /// No header: the file is empty or blank.
    #[error("The file has no header row.")]
    HeaderMissing,
    /// A header field that names no column.
    #[error("Header field {column} on line {line} ({text:?}) names no column.")]
    BadHeader {
        /// Line.
        line: u64,
        /// 0-based field.
        column: usize,
        /// The field as written.
        text: String,
    },
    /// More bad rows than tolerated.
    #[error("{bad} bad rows, more than the {limit} tolerated; the first at line {line}: {first}")]
    TooManyBadRows {
        /// Bad rows seen.
        bad: u64,
        /// The limit.
        limit: u64,
        /// The first bad row's line.
        line: u64,
        /// Why it was bad.
        first: PlainRowError,
    },
    /// The sink asked to stop at a bad row.
    #[error("Stopped at line {line}.")]
    Stopped {
        /// Line.
        line: u64,
    },
    /// The sink failed.
    #[error("{0}")]
    Sink(F),
}

/// Counts from one scan.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PlainStats {
    /// Lines, blank ones included.
    pub lines: u64,
    /// Bytes consumed.
    pub bytes: u64,
    /// Rows accepted.
    pub rows: u64,
    /// Rows refused.
    pub bad_rows: u64,
    /// Blank lines skipped.
    pub blank_lines: u64,
    /// Fields rewritten into the MMS date form.
    pub rewritten: u64,
}

/// Rebuilds rows: fields in, a [`Row`] of exactly `n` fields out, every
/// date field in the MMS form. One buffer, reused across rows.
#[derive(Debug, Default)]
pub struct Rows {
    line: Vec<u8>,
    splitter: Splitter,
    scratch: Vec<u8>,
}

impl Rows {
    /// An empty rebuilder.
    pub fn new() -> Self {
        Self::default()
    }

    /// The row of these fields, `n` wide: missing fields are empty, extra
    /// fields are dropped (check them first). Returns the row and how many
    /// fields were rewritten.
    pub fn build<'a, 'f>(
        &'a mut self,
        fields: impl Iterator<Item = &'f [u8]>,
        n: usize,
        offset: &str,
    ) -> (Row<'a>, u64) {
        self.line.clear();
        let mut rewritten = 0;
        let mut count = 0;
        for field in fields.take(n) {
            if count > 0 {
                self.line.push(b',');
            }
            count += 1;
            let value = if dates::to_mms(field, offset, &mut self.scratch) {
                rewritten += 1;
                self.scratch.as_slice()
            } else {
                field
            };
            write_field(&mut self.line, value);
        }
        if count < n {
            // Pad with empty fields: one separator per missing field, the
            // first of them only when a field already stands before it.
            let missing = n - count;
            let separators = if count == 0 { missing - 1 } else { missing };
            self.line.extend(std::iter::repeat_n(b',', separators));
        }
        (self.splitter.split(&self.line), rewritten)
    }
}

/// `value` as one CSV field: quoted, with `"` doubled, when it holds a
/// comma, a quote or a line break.
fn write_field(line: &mut Vec<u8>, value: &[u8]) {
    if value
        .iter()
        .any(|b| matches!(b, b',' | b'"' | b'\n' | b'\r'))
    {
        line.push(b'"');
        for &b in value {
            if b == b'"' {
                line.push(b'"');
            }
            line.push(b);
        }
        line.push(b'"');
    } else {
        line.extend_from_slice(value);
    }
}

const BUF: usize = 1 << 20;

/// Scans `reader` to the end, delivering the header and every row to `sink`.
pub fn scan<R: Read, S: PlainSink>(
    reader: R,
    sink: &mut S,
    opts: &PlainOptions<'_>,
) -> Result<PlainStats, PlainError<S::Fatal>> {
    let mut rd = BufReader::with_capacity(BUF, reader);
    let mut record = Vec::new();
    let mut rows = Rows::new();
    let mut splitter = Splitter::default();
    let mut stats = PlainStats::default();
    let mut header: Option<Header> = None;
    let mut first_bad: Option<(u64, PlainRowError)> = None;
    let mut first = true;
    loop {
        let pos = Pos {
            line: stats.lines + 1,
            byte_offset: stats.bytes,
        };
        record.clear();
        let (read, lines) =
            read_record(&mut rd, &mut record, opts.max_record_bytes).map_err(|e| match e {
                ReadError::Io(e) => PlainError::Io(e),
                ReadError::TooLong => PlainError::RecordTooLong {
                    line: pos.line,
                    limit: opts.max_record_bytes,
                },
            })?;
        if read == 0 {
            break;
        }
        stats.bytes += read as u64;
        stats.lines += lines;
        let mut line = record.as_slice();
        while let Some((&last, rest)) = line.split_last()
            && matches!(last, b'\n' | b'\r')
        {
            line = rest;
        }
        if first {
            first = false;
            line = line.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(line);
        }
        if line.iter().all(|b| matches!(b, b' ' | b'\t')) {
            stats.blank_lines += 1;
            continue;
        }
        let Some(h) = header.as_ref() else {
            let row = splitter.split(line);
            let fields: Vec<String> = row
                .iter()
                .map(|f| String::from_utf8_lossy(f).into_owned())
                .collect();
            let h = Header::from_raw(fields.iter().map(String::as_str)).map_err(|column| {
                PlainError::BadHeader {
                    line: pos.line,
                    column,
                    text: fields[column].clone(),
                }
            })?;
            sink.header(None, &h, pos).map_err(PlainError::Sink)?;
            header = Some(h);
            continue;
        };
        let row = splitter.split(line);
        let n = h.len();
        if row.len() > n {
            let observed = row.len_trimmed();
            if observed > n {
                stats.bad_rows += 1;
                let err = PlainRowError::TooManyFields {
                    declared: n,
                    observed,
                };
                if sink.bad_row(line, pos, &err) == Flow::Stop {
                    return Err(PlainError::Stopped { line: pos.line });
                }
                let (first_line, first) = first_bad.get_or_insert((pos.line, err));
                if stats.bad_rows > opts.max_bad_rows {
                    return Err(PlainError::TooManyBadRows {
                        bad: stats.bad_rows,
                        limit: opts.max_bad_rows,
                        line: *first_line,
                        first: first.clone(),
                    });
                }
                continue;
            }
        }
        let (rebuilt, rewritten) = rows.build(row.iter(), n, opts.offset);
        stats.rewritten += rewritten;
        sink.row(&rebuilt, pos).map_err(PlainError::Sink)?;
        stats.rows += 1;
    }
    if header.is_none() {
        return Err(PlainError::HeaderMissing);
    }
    Ok(stats)
}

enum ReadError {
    Io(io::Error),
    TooLong,
}

/// One record into `record`: a line, extended while a quote is open.
/// Returns the bytes read and the lines they span; `(0, 0)` at the end.
fn read_record<R: BufRead>(
    rd: &mut R,
    record: &mut Vec<u8>,
    limit: usize,
) -> Result<(usize, u64), ReadError> {
    let mut total = 0usize;
    let mut lines = 0u64;
    let mut quotes = 0usize;
    loop {
        let n = rd.read_until(b'\n', record).map_err(ReadError::Io)?;
        if n == 0 {
            return Ok((total, lines));
        }
        total += n;
        lines += 1;
        if total > limit {
            return Err(ReadError::TooLong);
        }
        quotes += memchr_iter(b'"', &record[total - n..]).count();
        if quotes.is_multiple_of(2) {
            return Ok((total, lines));
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use utilake_mms_decode::Never;

    /// Collects everything as owned strings.
    #[derive(Default, Debug)]
    pub(crate) struct Collect {
        pub blocks: Vec<(Option<String>, Header)>,
        pub rows: Vec<Vec<String>>,
        pub bad: Vec<(u64, String)>,
        pub stop_on_bad: bool,
    }

    impl PlainSink for Collect {
        type Fatal = Never;
        fn header(&mut self, array: Option<&str>, header: &Header, _pos: Pos) -> Result<(), Never> {
            self.blocks.push((array.map(str::to_owned), header.clone()));
            Ok(())
        }
        fn row(&mut self, row: &Row<'_>, _pos: Pos) -> Result<(), Never> {
            self.rows.push(
                row.iter()
                    .map(|f| String::from_utf8_lossy(f).into_owned())
                    .collect(),
            );
            Ok(())
        }
        fn bad_row(&mut self, raw: &[u8], pos: Pos, _err: &PlainRowError) -> Flow {
            self.bad
                .push((pos.line, String::from_utf8_lossy(raw).into_owned()));
            if self.stop_on_bad {
                Flow::Stop
            } else {
                Flow::Continue
            }
        }
    }

    /// A sink that fails on the first row.
    struct Failing;
    impl PlainSink for Failing {
        type Fatal = &'static str;
        fn header(&mut self, _: Option<&str>, h: &Header, _: Pos) -> Result<(), &'static str> {
            if h.len() > 3 { Err("too wide") } else { Ok(()) }
        }
        fn row(&mut self, _: &Row<'_>, _: Pos) -> Result<(), &'static str> {
            Err("no rows please")
        }
        fn bad_row(&mut self, _: &[u8], _: Pos, _: &PlainRowError) -> Flow {
            Flow::Continue
        }
    }

    fn run(
        text: &[u8],
        opts: &PlainOptions<'_>,
    ) -> (Collect, Result<PlainStats, PlainError<Never>>) {
        let mut c = Collect::default();
        let r = scan(text, &mut c, opts);
        (c, r)
    }

    const GBB: &[u8] = b"\xEF\xBB\xBFGasDate,FacilityName,FacilityId,ActualQuantity,Quality\r\n01/01/2019,Eastern Gas Pipeline,520047,19.400,OK\r\n01/01/2019,\"Horsley, Park\",520047,,\r\n\r\n";

    #[test]
    fn a_gbb_head_with_a_bom_and_dmy_dates() {
        let (c, r) = run(GBB, &PlainOptions::default());
        let stats = r.unwrap();
        assert_eq!(
            c.blocks[0].1.names,
            [
                "GASDATE",
                "FACILITYNAME",
                "FACILITYID",
                "ACTUALQUANTITY",
                "QUALITY"
            ]
        );
        assert_eq!(c.blocks[0].1.raw[0], "GasDate");
        assert_eq!(
            c.rows[0],
            [
                "2019/01/01",
                "Eastern Gas Pipeline",
                "520047",
                "19.400",
                "OK"
            ]
        );
        assert_eq!(c.rows[1], ["2019/01/01", "Horsley, Park", "520047", "", ""]);
        assert_eq!(
            (stats.lines, stats.rows, stats.blank_lines, stats.rewritten),
            (4, 2, 1, 2)
        );
        assert_eq!(stats.bytes as usize, GBB.len());
        assert!(c.blocks[0].0.is_none());
    }

    #[test]
    fn sttm_dates_and_short_rows() {
        let text = b"gas_date,hub_identifier,ex_ante_market_price,report_datetime\n02 Oct 2026,ADL,11.0944,01 Oct 2026 11:32:27\n01 Oct 2026,BRI\n";
        let (c, r) = run(text, &PlainOptions::default());
        r.unwrap();
        assert_eq!(
            c.rows[0],
            ["2026/10/02", "ADL", "11.0944", "2026/10/01 11:32:27"]
        );
        assert_eq!(c.rows[1], ["2026/10/01", "BRI", "", ""]);
        assert_eq!(c.blocks[0].1.index_of("REPORT_DATETIME"), Some(3));
        assert_eq!(c.blocks[0].1.index_of("X"), None);
    }

    #[test]
    fn wem_headers_with_units_and_quoted_dmy_times() {
        let text = b"Dispatch Interval,Energy Clearing Price,Extracted At,\n\"01/01/2024 07:55:00\",61.2,\"02/01/2024 08:30:00\",\n";
        let (c, r) = run(
            text,
            &PlainOptions {
                offset: "+08:00",
                ..PlainOptions::default()
            },
        );
        r.unwrap();
        assert_eq!(
            c.blocks[0].1.names,
            ["DISPATCH_INTERVAL", "ENERGY_CLEARING_PRICE", "EXTRACTED_AT"]
        );
        assert_eq!(
            c.rows[0],
            ["2024/01/01 07:55:00", "61.2", "2024/01/02 08:30:00"]
        );
    }

    #[test]
    fn quoted_fields_span_lines_and_quotes_are_doubled() {
        let text = b"a,b\n1,\"two\nlines, with \"\"quotes\"\"\"\n2,x\n";
        let (c, r) = run(text, &PlainOptions::default());
        let stats = r.unwrap();
        assert_eq!(c.rows[0], ["1", "two\nlines, with \"quotes\""]);
        assert_eq!(c.rows[1], ["2", "x"]);
        assert_eq!(stats.lines, 4);
    }

    #[test]
    fn too_many_fields_is_a_bad_row() {
        let text = b"a,b\n1,2,3\n4,5\n";
        let (c, r) = run(text, &PlainOptions::default());
        assert!(matches!(
            r,
            Err(PlainError::TooManyBadRows {
                bad: 1,
                limit: 0,
                line: 2,
                ..
            })
        ));
        assert_eq!(c.bad, [(2, "1,2,3".to_owned())]);
        let (c, r) = run(
            text,
            &PlainOptions {
                max_bad_rows: 1,
                ..PlainOptions::default()
            },
        );
        let stats = r.unwrap();
        assert_eq!((stats.rows, stats.bad_rows), (1, 1));
        assert_eq!(c.rows[0], ["4", "5"]);
        let mut c = Collect {
            stop_on_bad: true,
            ..Collect::default()
        };
        assert!(matches!(
            scan(&text[..], &mut c, &PlainOptions::default()),
            Err(PlainError::Stopped { line: 2 })
        ));
        let e = PlainError::<Never>::TooManyBadRows {
            bad: 1,
            limit: 0,
            line: 2,
            first: PlainRowError::TooManyFields {
                declared: 2,
                observed: 3,
            },
        };
        assert!(e.to_string().contains("3 fields but the header has 2"));
    }

    #[test]
    fn headers() {
        assert!(matches!(
            run(b"", &PlainOptions::default()).1,
            Err(PlainError::HeaderMissing)
        ));
        assert!(matches!(
            run(b"\n  \n", &PlainOptions::default()).1,
            Err(PlainError::HeaderMissing)
        ));
        let r = run(b"a,(),c\n", &PlainOptions::default()).1;
        assert!(
            matches!(r, Err(PlainError::BadHeader { line: 1, column: 1, ref text }) if text == "()"),
            "{r:?}"
        );
        let (c, r) = run(b"a,A,a,,\n1,2,3\n", &PlainOptions::default());
        r.unwrap();
        assert_eq!(c.blocks[0].1.names, ["A", "A_2", "A_3"]);
        assert_eq!(c.blocks[0].1.len(), 3);
        assert!(!c.blocks[0].1.is_empty());
        assert!(Header::default().is_empty());
        let r = run(b"\xFF\xFE,b\n1,2\n", &PlainOptions::default()).1;
        assert!(
            matches!(r, Err(PlainError::BadHeader { column: 0, ref text, .. }) if text == "\u{fffd}\u{fffd}")
        );
        let (c, r) = run(b"\xFF\xFEa,b\n1,2\n", &PlainOptions::default());
        r.unwrap();
        assert_eq!(c.blocks[0].1.raw[0], "\u{fffd}\u{fffd}a");
        assert_eq!(c.blocks[0].1.names[0], "A");
    }

    #[test]
    fn record_limit_and_io_errors() {
        let r = run(
            b"a,b\n1,\"open quote never closes\n2,3\n",
            &PlainOptions {
                max_record_bytes: 16,
                ..PlainOptions::default()
            },
        )
        .1;
        assert!(matches!(
            r,
            Err(PlainError::RecordTooLong { line: 2, limit: 16 })
        ));
        struct Broken;
        impl Read for Broken {
            fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
                Err(io::Error::other("disk gone"))
            }
        }
        let mut c = Collect::default();
        let r = scan(Broken, &mut c, &PlainOptions::default());
        assert!(matches!(r, Err(PlainError::Io(_))));
        assert!(r.unwrap_err().to_string().contains("disk gone"));
    }

    #[test]
    fn a_sink_may_fail() {
        let mut f = Failing;
        assert!(matches!(
            scan(&b"a,b,c,d\n"[..], &mut f, &PlainOptions::default()),
            Err(PlainError::Sink("too wide"))
        ));
        assert!(matches!(
            scan(&b"a,b\n1,2\n"[..], &mut f, &PlainOptions::default()),
            Err(PlainError::Sink("no rows please"))
        ));
        assert!(matches!(
            scan(&b"a,b\n1,2,3\n"[..], &mut f, &PlainOptions::default()),
            Err(PlainError::TooManyBadRows { .. })
        ));
    }

    #[test]
    fn an_unterminated_quote_at_the_end_still_yields_the_row() {
        let (c, r) = run(b"a,b\n1,\"x", &PlainOptions::default());
        r.unwrap();
        assert_eq!(c.rows[0], ["1", "x"]);
    }

    #[test]
    fn every_fixture_head_reads_with_its_dates_rewritten() {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/testdata");
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .filter(|n| n.ends_with(".csv"))
            .collect();
        names.sort();
        assert_eq!(names.len(), 6);
        for name in names {
            let offset = if name.starts_with("wem") {
                "+08:00"
            } else {
                "+10:00"
            };
            let bytes = std::fs::read(format!("{dir}/{name}")).unwrap();
            let (c, r) = run(
                &bytes,
                &PlainOptions {
                    offset,
                    ..PlainOptions::default()
                },
            );
            let stats = r.unwrap_or_else(|e| panic!("{name}: {e}"));
            assert_eq!(stats.rows, 2, "{name}");
            // The cumulative GBB file already writes the MMS form.
            let already_mms = name.contains("actual_flow_storage");
            assert_eq!(stats.rewritten >= 2, !already_mms, "{name}: {stats:?}");
            for row in &c.rows {
                for f in row {
                    assert!(
                        !dates::is_date(f.as_bytes(), offset) || f.contains('/'),
                        "{name}: {f}"
                    );
                }
            }
        }
    }

    #[test]
    fn rows_rebuild_exactly_n_fields() {
        let mut rows = Rows::new();
        let (r, n) = rows.build(
            [&b"29/09/2018"[..], b"a,b", b"q\"q"].into_iter(),
            4,
            "+10:00",
        );
        assert_eq!(r.len(), 4);
        assert_eq!(r.get(0), Some(&b"2018/09/29"[..]));
        assert_eq!(r.get(1), Some(&b"a,b"[..]));
        assert_eq!(r.get(2), Some(&b"q\"q"[..]));
        assert_eq!(r.get(3), Some(&b""[..]));
        assert_eq!(n, 1);
        let (r, _) = rows.build(std::iter::empty(), 2, "+10:00");
        assert_eq!(r.len(), 2);
        let (r, _) = rows.build([&b"x"[..], b"y", b"z"].into_iter(), 1, "+10:00");
        assert_eq!(r.len(), 1);
    }
}
