//! A WEMDE document as blocks of rows.
//!
//! ```text
//! {"transactionId": "…", "data": {"tradingDay": "2026-10-21",
//!   "referenceTradingPrices": [{"tradingInterval": "2026-10-21T08:00:00+08:00",
//!                               "referenceTradingPrice": 93.85, "isPublished": true}, …]}}
//! ```
//!
//! Every array of objects under `data` is a block named by its path; each
//! element is a row. The scalars beside the arrays (`tradingDay`) and
//! beside `data` (`transactionId`) are columns of every row. A nested
//! object's keys join with `_`; an array of objects one level down is a
//! child block, named `{array}.{sub}`, whose rows carry the parent row's
//! scalars with the child's keys prefixed by the child's name; anything
//! deeper, and any array that is not of objects, is kept as JSON text.
//! Columns are in name order, so a document's key order never matters.
//! Numbers are written as the document writes them, booleans as `true` and
//! `false`, `null` as an empty field, and a string that is a date in the
//! MMS form.

use std::collections::BTreeMap;
use std::io::{self, BufReader, Read};

use serde_json::Value;

use crate::csv::{Header, PlainError, PlainOptions, PlainSink, Rows};
use crate::names::json_name;
use utilake_mms_decode::Pos;

/// Counts from one document.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct JsonStats {
    /// Blocks: arrays of objects, children included.
    pub blocks: u64,
    /// Rows.
    pub rows: u64,
    /// Fields rewritten into the MMS date form.
    pub rewritten: u64,
}

/// Why a document could not be read.
#[derive(Debug, thiserror::Error)]
pub enum JsonError<F> {
    /// Reading or parsing failed.
    #[error("The document is not JSON: {0}")]
    Syntax(#[from] serde_json::Error),
    /// Not the shape of a WEMDE document.
    #[error("The document is not a WEMDE document: {0}.")]
    Shape(&'static str),
    /// The sink failed.
    #[error("{0}")]
    Sink(F),
}

impl<F> From<PlainError<F>> for JsonError<F> {
    fn from(e: PlainError<F>) -> Self {
        match e {
            PlainError::Sink(f) => Self::Sink(f),
            other => Self::Shape(match other {
                PlainError::Io(_) => "reading failed",
                _ => "a column names nothing",
            }),
        }
    }
}

/// One flattened row: column name to (raw key path, text).
type Flat = BTreeMap<String, (String, String)>;

/// Reads the document in `reader` and delivers each block and its rows to
/// `sink`.
pub fn scan<R: Read, S: PlainSink>(
    reader: R,
    sink: &mut S,
    opts: &PlainOptions<'_>,
) -> Result<JsonStats, JsonError<S::Fatal>> {
    let doc: Value = serde_json::from_reader(BufReader::new(reader))?;
    let Value::Object(top) = doc else {
        return Err(JsonError::Shape("the top level is not an object"));
    };
    let Some(Value::Object(data)) = top.get("data") else {
        return Err(JsonError::Shape("`data` is missing or not an object"));
    };
    let mut context = Flat::new();
    for (k, v) in &top {
        if k != "data" && !matches!(v, Value::Object(_) | Value::Array(_)) {
            put_scalar(&mut context, k, k, v);
        }
    }
    let mut arrays: Vec<(&String, Vec<&Map>)> = Vec::new();
    for (k, v) in data {
        match v {
            Value::Array(items) if let Some(objects) = objects(items) => arrays.push((k, objects)),
            Value::Object(o) => flatten_object(&mut context, k, &json_name(k), o, None),
            other => put_scalar(&mut context, k, k, other),
        }
    }
    let mut stats = JsonStats::default();
    let mut rows = Rows::new();
    for (name, items) in arrays {
        let mut block: Vec<Flat> = Vec::with_capacity(items.len());
        let mut children: BTreeMap<String, Vec<Flat>> = BTreeMap::new();
        for o in items {
            let mut row = context.clone();
            let mut subs: Vec<(&String, Vec<&Map>)> = Vec::new();
            flatten_object(&mut row, "", "", o, Some(&mut subs));
            for (sub, sub_items) in subs {
                let rows_of = children.entry(sub.clone()).or_default();
                for so in sub_items {
                    let mut child = row.clone();
                    flatten_object(&mut child, sub, &json_name(sub), so, None);
                    rows_of.push(child);
                }
            }
            block.push(row);
        }
        emit(sink, opts, &mut rows, &mut stats, name, &block)?;
        for (sub, child_rows) in children {
            emit(
                sink,
                opts,
                &mut rows,
                &mut stats,
                &format!("{name}.{sub}"),
                &child_rows,
            )?;
        }
    }
    Ok(stats)
}

type Map = serde_json::Map<String, Value>;

/// The objects of a non-empty array of objects; `None` for any other array.
fn objects(items: &[Value]) -> Option<Vec<&Map>> {
    if items.is_empty() {
        return None;
    }
    items
        .iter()
        .map(|v| match v {
            Value::Object(o) => Some(o),
            _ => None,
        })
        .collect()
}

/// Puts a scalar (or an array that is not of objects, as text) into a row.
fn put_scalar(row: &mut Flat, raw: &str, name: &str, v: &Value) {
    let text = match v {
        Value::Null => String::new(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        Value::String(s) => s.clone(),
        other => other.to_string(),
    };
    row.entry(json_name(name))
        .or_insert_with(|| (raw.to_owned(), text));
}

/// Flattens `o` into `row` under `raw_prefix`/`name_prefix` (empty at the
/// top). Arrays of objects go to `subs` when given, else to text.
fn flatten_object<'v>(
    row: &mut Flat,
    raw_prefix: &str,
    name_prefix: &str,
    o: &'v Map,
    mut subs: Option<&mut Vec<(&'v String, Vec<&'v Map>)>>,
) {
    for (k, v) in o {
        let raw = if raw_prefix.is_empty() {
            k.clone()
        } else {
            format!("{raw_prefix}.{k}")
        };
        let name = if name_prefix.is_empty() {
            k.clone()
        } else {
            format!("{name_prefix}_{k}")
        };
        match v {
            Value::Object(inner) => flatten_object(row, &raw, &name, inner, None),
            Value::Array(items)
                if let Some(s) = subs.as_deref_mut()
                    && let Some(objects) = objects(items) =>
            {
                s.push((k, objects));
            }
            other => put_scalar(row, &raw, &name, other),
        }
    }
}

fn emit<S: PlainSink>(
    sink: &mut S,
    opts: &PlainOptions<'_>,
    rows: &mut Rows,
    stats: &mut JsonStats,
    array: &str,
    block: &[Flat],
) -> Result<(), JsonError<S::Fatal>> {
    let mut columns: BTreeMap<&str, &str> = BTreeMap::new();
    for row in block {
        for (name, (raw, _)) in row {
            columns.entry(name).or_insert(raw);
        }
    }
    let names: Vec<String> = columns.keys().map(|n| (*n).to_owned()).collect();
    let header = Header {
        raw: columns.values().map(|r| (*r).to_owned()).collect(),
        names,
    };
    stats.blocks += 1;
    sink.header(Some(array), &header, Pos::default())
        .map_err(JsonError::Sink)?;
    let empty = String::new();
    for (i, row) in block.iter().enumerate() {
        let fields = header
            .names
            .iter()
            .map(|n| row.get(n).map_or(empty.as_bytes(), |(_, t)| t.as_bytes()));
        let (built, rewritten) = rows.build(fields, header.len(), opts.offset);
        stats.rewritten += rewritten;
        let pos = Pos {
            line: i as u64 + 1,
            byte_offset: 0,
        };
        sink.row(&built, pos).map_err(JsonError::Sink)?;
        stats.rows += 1;
    }
    Ok(())
}

/// Reads a document from bytes, for callers that already hold them.
pub fn scan_bytes<S: PlainSink>(
    bytes: &[u8],
    sink: &mut S,
    opts: &PlainOptions<'_>,
) -> Result<JsonStats, JsonError<S::Fatal>> {
    scan(io::Cursor::new(bytes), sink, opts)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::csv::tests::Collect;
    use utilake_mms_decode::Never;

    #[test]
    fn the_fixture_document_reads() {
        let bytes = std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/testdata/wemde_reference_trading_price_2026-10-21.json"
        ))
        .unwrap();
        let mut c = Collect::default();
        let stats = scan_bytes(
            &bytes,
            &mut c,
            &PlainOptions {
                offset: "+08:00",
                ..PlainOptions::default()
            },
        )
        .unwrap();
        assert_eq!((stats.blocks, stats.rows), (1, 2));
        assert_eq!(c.rows[1][3], "2026/10/21 08:30:00");
    }

    const RTP: &str = r#"{"transactionId":"8b1c","data":{"tradingDay":"2026-10-21","referenceTradingPrices":[
        {"tradingInterval":"2026-10-21T08:00:00+08:00","referenceTradingPrice":93.85,"isPublished":true},
        {"tradingInterval":"2026-10-21T08:30:00+08:00","referenceTradingPrice":null,"isPublished":false,"note":"late"}]}}"#;

    fn run(text: &str, offset: &str) -> (Collect, Result<JsonStats, JsonError<Never>>) {
        let mut c = Collect::default();
        let r = scan_bytes(
            text.as_bytes(),
            &mut c,
            &PlainOptions {
                offset,
                ..PlainOptions::default()
            },
        );
        (c, r)
    }

    #[test]
    fn a_reference_trading_price_document() {
        let (c, r) = run(RTP, "+08:00");
        let stats = r.unwrap();
        assert_eq!((stats.blocks, stats.rows, stats.rewritten), (1, 2, 4));
        let (array, h) = &c.blocks[0];
        assert_eq!(array.as_deref(), Some("referenceTradingPrices"));
        assert_eq!(
            h.names,
            [
                "IS_PUBLISHED",
                "NOTE",
                "REFERENCE_TRADING_PRICE",
                "TRADING_DAY",
                "TRADING_INTERVAL",
                "TRANSACTION_ID"
            ]
        );
        assert_eq!(h.raw[5], "transactionId");
        assert_eq!(
            c.rows[0],
            [
                "true",
                "",
                "93.85",
                "2026/10/21",
                "2026/10/21 08:00:00",
                "8b1c"
            ]
        );
        assert_eq!(
            c.rows[1],
            [
                "false",
                "late",
                "",
                "2026/10/21",
                "2026/10/21 08:30:00",
                "8b1c"
            ]
        );
    }

    #[test]
    fn a_foreign_offset_is_kept_as_written() {
        let (c, _) = run(RTP, "+10:00");
        assert_eq!(c.rows[0][4], "2026-10-21T08:00:00+08:00");
    }

    #[test]
    fn nested_objects_children_and_text_arrays() {
        let doc = r#"{"data":{"meta":{"run":{"id":7}},"tags":["a","b"],"n":1,
          "facilities":[
            {"code":"F1","limits":{"min":0,"max":10},"prices":[{"t":"2026-10-21","p":1},{"t":"2026-10-22","p":2,"deep":[{"x":1}]}],"raw":[1,2]},
            {"code":"F2","limits":{"min":1,"max":11},"prices":[]}
          ]}}"#;
        let (c, r) = run(doc, "+08:00");
        let stats = r.unwrap();
        assert_eq!((stats.blocks, stats.rows), (2, 4));
        let (array, h) = &c.blocks[0];
        assert_eq!(array.as_deref(), Some("facilities"));
        assert_eq!(
            h.names,
            [
                "CODE",
                "LIMITS_MAX",
                "LIMITS_MIN",
                "META_RUN_ID",
                "N",
                "PRICES",
                "RAW",
                "TAGS"
            ]
        );
        assert_eq!(h.raw[3], "meta.run.id");
        assert_eq!(
            c.rows[0],
            ["F1", "10", "0", "7", "1", "", "[1,2]", "[\"a\",\"b\"]"]
        );
        assert_eq!(
            c.rows[1],
            ["F2", "11", "1", "7", "1", "[]", "", "[\"a\",\"b\"]"]
        );
        let (array, h) = &c.blocks[1];
        assert_eq!(array.as_deref(), Some("facilities.prices"));
        assert_eq!(
            h.names,
            [
                "CODE",
                "LIMITS_MAX",
                "LIMITS_MIN",
                "META_RUN_ID",
                "N",
                "PRICES_DEEP",
                "PRICES_P",
                "PRICES_T",
                "RAW",
                "TAGS"
            ]
        );
        assert_eq!(h.raw[7], "prices.t");
        assert_eq!(
            c.rows[2],
            [
                "F1",
                "10",
                "0",
                "7",
                "1",
                "",
                "1",
                "2026/10/21",
                "[1,2]",
                "[\"a\",\"b\"]"
            ]
        );
        assert_eq!(c.rows[3][5], "[{\"x\":1}]");
    }

    #[test]
    fn shapes_that_are_not_documents() {
        assert!(matches!(run("[1]", "+08:00").1, Err(JsonError::Shape(_))));
        assert!(matches!(
            run(r#"{"x":1}"#, "+08:00").1,
            Err(JsonError::Shape(_))
        ));
        assert!(matches!(
            run(r#"{"data":[]}"#, "+08:00").1,
            Err(JsonError::Shape(_))
        ));
        let e = run("{nope", "+08:00").1.unwrap_err();
        assert!(matches!(e, JsonError::Syntax(_)));
        assert!(e.to_string().starts_with("The document is not JSON"));
        assert_eq!(
            JsonError::<Never>::Shape("x").to_string(),
            "The document is not a WEMDE document: x."
        );
        let (c, r) = run(r#"{"data":{"a":1}}"#, "+08:00");
        assert_eq!(r.unwrap(), JsonStats::default());
        assert!(c.blocks.is_empty());
    }

    #[test]
    fn plain_errors_convert() {
        let e: JsonError<Never> = PlainError::<Never>::HeaderMissing.into();
        assert!(matches!(e, JsonError::Shape("a column names nothing")));
        let e: JsonError<Never> = PlainError::<Never>::Io(io::Error::other("x")).into();
        assert!(matches!(e, JsonError::Shape("reading failed")));
        let e: JsonError<&str> = PlainError::Sink("f").into();
        assert!(matches!(e, JsonError::Sink("f")));
    }

    #[test]
    fn a_sink_may_fail() {
        struct Fail(bool);
        impl PlainSink for Fail {
            type Fatal = &'static str;
            fn header(&mut self, _: Option<&str>, _: &Header, _: Pos) -> Result<(), &'static str> {
                if self.0 { Err("header") } else { Ok(()) }
            }
            fn row(&mut self, _: &crate::Row<'_>, _: Pos) -> Result<(), &'static str> {
                Err("row")
            }
            fn bad_row(&mut self, _: &[u8], _: Pos, _: &crate::PlainRowError) -> crate::Flow {
                crate::Flow::Continue
            }
        }
        let opts = PlainOptions::default();
        let err = crate::PlainRowError::TooManyFields {
            declared: 1,
            observed: 2,
        };
        assert_eq!(
            Fail(true).bad_row(b"", Pos::default(), &err),
            crate::Flow::Continue
        );
        assert!(matches!(
            scan_bytes(RTP.as_bytes(), &mut Fail(true), &opts),
            Err(JsonError::Sink("header"))
        ));
        assert!(matches!(
            scan_bytes(RTP.as_bytes(), &mut Fail(false), &opts),
            Err(JsonError::Sink("row"))
        ));
        struct SecondBlock(u8);
        impl PlainSink for SecondBlock {
            type Fatal = &'static str;
            fn header(&mut self, _: Option<&str>, _: &Header, _: Pos) -> Result<(), &'static str> {
                self.0 += 1;
                if self.0 == 2 { Err("child") } else { Ok(()) }
            }
            fn row(&mut self, _: &crate::Row<'_>, _: Pos) -> Result<(), &'static str> {
                Ok(())
            }
            fn bad_row(&mut self, _: &[u8], _: Pos, _: &crate::PlainRowError) -> crate::Flow {
                crate::Flow::Continue
            }
        }
        let err = crate::PlainRowError::TooManyFields {
            declared: 1,
            observed: 2,
        };
        assert_eq!(
            SecondBlock(0).bad_row(b"", Pos::default(), &err),
            crate::Flow::Continue
        );
        let doc = r#"{"data":{"a":[{"k":1,"s":[{"x":1}]}]}}"#;
        assert!(matches!(
            scan_bytes(doc.as_bytes(), &mut SecondBlock(0), &opts),
            Err(JsonError::Sink("child"))
        ));
    }
}
