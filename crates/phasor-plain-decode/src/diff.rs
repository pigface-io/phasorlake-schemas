//! The rows a version of a file adds to the version before it.
//!
//! AEMO rewrites many files in place: GBB's cumulative files grow by a day
//! and revise weeks back, STTM and DWGM restate a window with every
//! publish, WEM's current month is written again each morning. Each version
//! is the previous one plus the new and revised rows. A [`RowSet`] holds
//! the rows of the previous version as hashes; a row of the next version
//! is new when the set does not hold it. The columns that carry the file's
//! own publish time are left out of the comparison, so a row republished
//! unchanged at a later time is not new.
//!
//! The rows compared are those every reader sees: fields as the decoders
//! hand them on, dates in the MMS form. A pure function of the two files,
//! whichever order they were read in.

use std::collections::HashSet;
use std::io::Read;

use phasor_decode::{Flow, Never, Pos, Row};
use xxhash_rust::xxh3::Xxh3;

use crate::csv::{self, Header, PlainError, PlainOptions, PlainRowError, PlainSink};

/// The rows of one version of a file, as hashes.
#[derive(Debug, Clone, Default)]
pub struct RowSet {
    seen: HashSet<u64>,
    /// Fields compared start here: 0 for a plain row, 4 past an MMS row's
    /// prefix.
    from: usize,
    /// Field indexes (from 0) left out of the comparison, sorted.
    skip: Vec<usize>,
}

impl RowSet {
    /// An empty set over rows whose fields start at `from`, leaving out
    /// the fields at `skip`.
    pub fn new(from: usize, skip: impl IntoIterator<Item = usize>) -> Self {
        let mut skip: Vec<usize> = skip.into_iter().collect();
        skip.sort_unstable();
        skip.dedup();
        Self {
            seen: HashSet::new(),
            from,
            skip,
        }
    }

    /// The indexes in `header` of the columns called `names`; a name the
    /// header lacks is simply absent.
    pub fn skip_of(header: &Header, names: &[&str]) -> Vec<usize> {
        names.iter().filter_map(|n| header.index_of(n)).collect()
    }

    /// The hash of a row's compared fields.
    pub fn hash(&self, row: &Row<'_>) -> u64 {
        let mut h = Xxh3::new();
        let mut count = 0u32;
        for i in self.from..row.len() {
            if self.skip.binary_search(&i).is_ok() {
                continue;
            }
            let f = row.get(i).unwrap_or_default();
            h.update(&(f.len() as u32).to_le_bytes());
            h.update(f);
            count += 1;
        }
        h.update(&count.to_le_bytes());
        h.digest()
    }

    /// Records a row; `true` when it was not in the set.
    pub fn insert(&mut self, row: &Row<'_>) -> bool {
        self.seen.insert(self.hash(row))
    }

    /// Whether the set holds this row.
    pub fn contains(&self, row: &Row<'_>) -> bool {
        self.seen.contains(&self.hash(row))
    }

    /// Distinct rows held.
    pub fn len(&self) -> usize {
        self.seen.len()
    }

    /// Whether no row is held.
    pub fn is_empty(&self) -> bool {
        self.seen.is_empty()
    }

    /// The set of every row of the plain CSV in `reader`, with the columns
    /// named `time_columns` left out, and the file's header.
    pub fn collect_csv<R: Read>(
        reader: R,
        opts: &PlainOptions<'_>,
        time_columns: &[&str],
    ) -> Result<(Header, Self), PlainError<Never>> {
        struct Collect<'a> {
            time_columns: &'a [&'a str],
            header: Header,
            set: RowSet,
        }
        impl PlainSink for Collect<'_> {
            type Fatal = Never;
            fn header(&mut self, _: Option<&str>, h: &Header, _: Pos) -> Result<(), Never> {
                self.set = RowSet::new(0, RowSet::skip_of(h, self.time_columns));
                self.header = h.clone();
                Ok(())
            }
            fn row(&mut self, row: &Row<'_>, _: Pos) -> Result<(), Never> {
                self.set.insert(row);
                Ok(())
            }
            fn bad_row(&mut self, _: &[u8], _: Pos, _: &PlainRowError) -> Flow {
                Flow::Continue
            }
        }
        let mut c = Collect {
            time_columns,
            header: Header::default(),
            set: Self::default(),
        };
        csv::scan(reader, &mut c, opts)?;
        Ok((c.header, c.set))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use phasor_decode::fields::Splitter;

    const V1: &[u8] = b"gas_date,hub,price,report_datetime\n01 Oct 2026,ADL,10.98,30 Sep 2026 11:32:27\n01 Oct 2026,BRI,10.95,30 Sep 2026 11:32:27\n";
    const V2: &[u8] = b"gas_date,hub,price,report_datetime\n01 Oct 2026,ADL,10.98,01 Oct 2026 11:32:27\n01 Oct 2026,BRI,10.96,01 Oct 2026 11:32:27\n02 Oct 2026,ADL,11.09,01 Oct 2026 11:32:27\n";

    #[test]
    fn the_rows_a_version_adds() {
        let opts = PlainOptions::default();
        let (h1, v1) = RowSet::collect_csv(V1, &opts, &["REPORT_DATETIME"]).unwrap();
        assert_eq!(h1.len(), 4);
        assert_eq!(v1.len(), 2);
        assert!(!v1.is_empty());
        let (_, v2) = RowSet::collect_csv(V2, &opts, &["REPORT_DATETIME"]).unwrap();
        let mut splitter = Splitter::default();
        let rows: Vec<&[u8]> = vec![
            b"2026/10/01,ADL,10.98,2026/10/01 11:32:27",
            b"2026/10/01,BRI,10.96,2026/10/01 11:32:27",
            b"2026/10/02,ADL,11.09,2026/10/01 11:32:27",
        ];
        let new: Vec<bool> = rows
            .iter()
            .map(|r| !v1.contains(&splitter.split(r)))
            .collect();
        assert_eq!(new, [false, true, true]);
        assert!(rows.iter().all(|r| v2.contains(&splitter.split(r))));
        let (_, strict) = RowSet::collect_csv(V1, &opts, &[]).unwrap();
        assert!(!strict.contains(&splitter.split(rows[0])));
        assert!(RowSet::collect_csv(&b""[..], &opts, &[]).is_err());
        let lenient = PlainOptions {
            max_bad_rows: 1,
            ..opts
        };
        let (_, set) = RowSet::collect_csv(&b"a,b\n1,2,3\n4,5\n"[..], &lenient, &[]).unwrap();
        assert_eq!(set.len(), 1);
    }

    #[test]
    fn hashes_tell_fields_apart() {
        let set = RowSet::new(0, [1, 1]);
        let mut s = Splitter::default();
        let a = set.hash(&s.split(b"x,skip,y"));
        let mut s2 = Splitter::default();
        assert_eq!(a, set.hash(&s2.split(b"x,other,y")));
        assert_ne!(a, set.hash(&s2.split(b"x,other,y,")));
        assert_ne!(a, set.hash(&s2.split(b"xy,other,")));
        assert_ne!(set.hash(&s2.split(b"ab,c")), set.hash(&s2.split(b"a,bc")));
        let mms = RowSet::new(4, []);
        assert_eq!(
            mms.hash(&s2.split(b"D,T,S,1,a,b")),
            mms.hash(&s2.split(b"D,T,S,2,a,b"))
        );
        let mut m = RowSet::new(0, []);
        assert!(m.insert(&s2.split(b"a")));
        assert!(!m.insert(&s2.split(b"a")));
        assert!(RowSet::default().is_empty());
    }
}
