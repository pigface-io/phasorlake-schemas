//! Splitting one line into fields without allocating per row.
//!
//! Most of AEMO's lines contain quotes (an MMS timestamp is always quoted),
//! so the quote-aware path is the only one: each line is walked once over its
//! commas and quotes together, eight bytes at a time, and only a field with a
//! doubled quote (`""`) is unescaped into a scratch buffer the splitter
//! reuses across rows. Field boundaries go into a reusable `Vec<Span>`.

/// Where a field's bytes live.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    start: u32,
    end: u32,
    /// `false`: into the line; `true`: into the scratch buffer.
    scratch: bool,
}

/// Reusable split state.
#[derive(Debug, Default)]
pub struct Splitter {
    spans: Vec<Span>,
    scratch: Vec<u8>,
}

/// One split line: the fields of a row.
#[derive(Debug, Clone, Copy)]
pub struct Row<'a> {
    line: &'a [u8],
    scratch: &'a [u8],
    spans: &'a [Span],
}

impl Splitter {
    /// A splitter with room for `fields` fields.
    pub fn with_capacity(fields: usize) -> Self {
        Self {
            spans: Vec::with_capacity(fields),
            scratch: Vec::with_capacity(256),
        }
    }

    /// Splits `line` (no trailing newline) into fields, in one pass over
    /// its commas and quotes together. A field that starts with a quote
    /// runs to its closing quote, `""` standing for one; anything after it
    /// up to the next comma is dropped. Unterminated, it takes the rest of
    /// the line.
    pub fn split<'a>(&'a mut self, line: &'a [u8]) -> Row<'a> {
        self.spans.clear();
        self.scratch.clear();
        let mut start = 0;
        let mut field = Field::at(line, start);
        // Commas and quotes before `from` are inside a field already read:
        // its opening quote, or the second quote of a `""`.
        let mut from = usize::from(matches!(field, Field::Open { .. }));
        for i in Hits::new(line) {
            if i < from {
                continue;
            }
            match (field, line[i]) {
                (Field::Plain, b',') => {
                    // Most fields: no quote to look past.
                    self.spans.push(Span {
                        start: start as u32,
                        end: i as u32,
                        scratch: false,
                    });
                    start = i + 1;
                    if line.get(start) == Some(&b'"') {
                        field = Field::Open { escaped: false };
                        from = start + 1;
                    }
                }
                (Field::Closed { .. }, b',') => {
                    self.push(line, start, i, field);
                    start = i + 1;
                    field = Field::at(line, start);
                    from = start + usize::from(matches!(field, Field::Open { .. }));
                }
                (Field::Open { .. }, b'"') if line.get(i + 1) == Some(&b'"') => {
                    field = Field::Open { escaped: true };
                    from = i + 2;
                }
                (Field::Open { escaped }, b'"') => field = Field::Closed { end: i, escaped },
                _ => {}
            }
        }
        self.push(line, start, line.len(), field);
        Row {
            line,
            scratch: &self.scratch,
            spans: &self.spans,
        }
    }

    /// The field from `start` to the comma or line end at `end`.
    fn push(&mut self, line: &[u8], start: usize, end: usize, field: Field) {
        let (start, end, escaped) = match field {
            Field::Plain => (start, end, false),
            Field::Open { escaped } => (start + 1, end, escaped),
            Field::Closed { end, escaped } => (start + 1, end, escaped),
        };
        if !escaped {
            self.spans.push(Span {
                start: start as u32,
                end: end as u32,
                scratch: false,
            });
            return;
        }
        let s = self.scratch.len();
        let mut k = start;
        while k < end {
            self.scratch.push(line[k]);
            k += if line[k] == b'"' && line.get(k + 1) == Some(&b'"') {
                2
            } else {
                1
            };
        }
        self.spans.push(Span {
            start: s as u32,
            end: self.scratch.len() as u32,
            scratch: true,
        });
    }
}

/// The field being read by [`Splitter::split`].
#[derive(Clone, Copy)]
enum Field {
    /// Not quoted: it ends at the next comma.
    Plain,
    /// Quoted, its closing quote not yet found.
    Open { escaped: bool },
    /// Quoted, closed at `end`.
    Closed { end: usize, escaped: bool },
}

/// Where a line's commas and quotes are, in order: eight bytes a word,
/// each tested by arithmetic, so a field costs a few instructions however
/// close the commas are.
struct Hits<'a> {
    line: &'a [u8],
    /// Where the word in `mask` starts.
    base: usize,
    /// The high bit of each byte of the word that is a comma or a quote.
    mask: u64,
}

const ONES: u64 = u64::from_ne_bytes([0x01; 8]);
const LOW7: u64 = u64::from_ne_bytes([0x7f; 8]);

/// The high bit of each byte of `word` that is zero, and of no other.
const fn zero_bytes(word: u64) -> u64 {
    !(((word & LOW7) + LOW7) | word | LOW7)
}

impl<'a> Hits<'a> {
    fn new(line: &'a [u8]) -> Self {
        let mut hits = Self {
            line,
            base: 0,
            mask: 0,
        };
        hits.load();
        hits
    }

    /// The mask of the word at `base`, short words padded with zeros.
    fn load(&mut self) {
        let rest = &self.line[self.base.min(self.line.len())..];
        let word = match rest.first_chunk::<8>() {
            Some(bytes) => u64::from_le_bytes(*bytes),
            None => {
                let mut bytes = [0u8; 8];
                bytes[..rest.len()].copy_from_slice(rest);
                u64::from_le_bytes(bytes)
            }
        };
        self.mask = zero_bytes(word ^ (ONES * u64::from(b',')))
            | zero_bytes(word ^ (ONES * u64::from(b'"')));
    }
}

impl Iterator for Hits<'_> {
    type Item = usize;

    #[inline]
    fn next(&mut self) -> Option<usize> {
        while self.mask == 0 {
            self.base += 8;
            if self.base >= self.line.len() {
                return None;
            }
            self.load();
        }
        let i = self.base + (self.mask.trailing_zeros() / 8) as usize;
        self.mask &= self.mask - 1;
        Some(i)
    }
}

impl Field {
    /// The field that starts at `start`.
    fn at(line: &[u8], start: usize) -> Self {
        if line.get(start) == Some(&b'"') {
            Self::Open { escaped: false }
        } else {
            Self::Plain
        }
    }
}

impl<'a> Row<'a> {
    /// Number of fields.
    pub fn len(&self) -> usize {
        self.spans.len()
    }

    /// Whether the row has no fields (never, for a non-empty line).
    pub fn is_empty(&self) -> bool {
        self.spans.is_empty()
    }

    /// The bytes of field `i`.
    #[inline]
    pub fn get(&self, i: usize) -> Option<&'a [u8]> {
        let s = self.spans.get(i)?;
        let src = if s.scratch { self.scratch } else { self.line };
        Some(&src[s.start as usize..s.end as usize])
    }

    /// Field `i` as text, if it is UTF-8.
    pub fn str(&self, i: usize) -> Option<&'a str> {
        self.get(i).and_then(|b| std::str::from_utf8(b).ok())
    }

    /// The whole line.
    #[cfg(test)]
    pub(crate) const fn line(&self) -> &'a [u8] {
        self.line
    }

    /// Every field.
    pub fn iter(&self) -> impl Iterator<Item = &'a [u8]> + '_ {
        (0..self.len()).filter_map(move |i| self.get(i))
    }

    /// The number of fields once trailing empty ones are dropped.
    pub fn len_trimmed(&self) -> usize {
        let mut n = self.len();
        while n > 0 && self.get(n - 1).is_some_and(<[u8]>::is_empty) {
            n -= 1;
        }
        n
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fields(line: &[u8]) -> Vec<String> {
        let mut s = Splitter::default();
        let r = s.split(line);
        r.iter()
            .map(|f| String::from_utf8_lossy(f).into_owned())
            .collect()
    }

    #[test]
    fn unquoted() {
        assert_eq!(fields(b"D,a,b,c"), ["D", "a", "b", "c"]);
        assert_eq!(fields(b"a,,b,"), ["a", "", "b", ""]);
        assert_eq!(fields(b""), [""]);
        assert_eq!(fields(b","), ["", ""]);
    }

    #[test]
    fn quoted() {
        assert_eq!(
            fields(br#"D,"2024/01/01 00:05:00",BUTLERSG,9.199999"#),
            ["D", "2024/01/01 00:05:00", "BUTLERSG", "9.199999"]
        );
        assert_eq!(fields(br#"a,"b, c",d"#), ["a", "b, c", "d"]);
        assert_eq!(
            fields(br#"a,"he said ""hi""",d"#),
            ["a", "he said \"hi\"", "d"]
        );
        assert_eq!(fields(br#""only""#), ["only"]);
        assert_eq!(fields(br#"a,"""#), ["a", ""]);
        assert_eq!(fields(br#"a,"b","#), ["a", "b", ""]);
        assert_eq!(fields(br#""a","b""#), ["a", "b"]);
        assert_eq!(fields(br#"x,"a""b""c",y"#), ["x", "a\"b\"c", "y"]);
        assert_eq!(
            fields(br#"C,"END OF REPORT",9"#),
            ["C", "END OF REPORT", "9"]
        );
    }

    /// The rules byte by byte, for comparison.
    fn reference(line: &[u8]) -> Vec<Vec<u8>> {
        let mut out = Vec::new();
        let mut i = 0;
        loop {
            let mut f = Vec::new();
            if line.get(i) == Some(&b'"') {
                i += 1;
                while i < line.len() {
                    if line[i] == b'"' {
                        if line.get(i + 1) == Some(&b'"') {
                            f.push(b'"');
                            i += 2;
                            continue;
                        }
                        i += 1;
                        while i < line.len() && line[i] != b',' {
                            i += 1;
                        }
                        break;
                    }
                    f.push(line[i]);
                    i += 1;
                }
            } else {
                while i < line.len() && line[i] != b',' {
                    f.push(line[i]);
                    i += 1;
                }
            }
            out.push(f);
            if i >= line.len() {
                return out;
            }
            i += 1;
        }
    }

    #[test]
    fn every_short_line_of_commas_quotes_and_letters_splits_as_the_rules_say() {
        let alphabet = *b",\"a";
        let mut s = Splitter::default();
        for len in 0..=7u32 {
            for code in 0..3usize.pow(len) {
                let mut c = code;
                let line: Vec<u8> = (0..len)
                    .map(|_| {
                        let b = alphabet[c % 3];
                        c /= 3;
                        b
                    })
                    .collect();
                let got: Vec<Vec<u8>> = s.split(&line).iter().map(<[u8]>::to_vec).collect();
                // The line rides along, so a failure names it.
                assert_eq!((&line, got), (&line, reference(&line)));
            }
        }
    }

    #[test]
    fn unterminated_quote_takes_rest() {
        let mut s = Splitter::default();
        let r = s.split(br#"a,"open,b"#);
        assert_eq!(r.len(), 2);
        assert_eq!(r.get(1), Some(b"open,b".as_slice()));
    }

    #[test]
    fn accessors() {
        let mut s = Splitter::with_capacity(4);
        let r = s.split(b"D,x,\xff,y,,");
        assert_eq!(r.len(), 6);
        assert!(!r.is_empty());
        assert_eq!(r.str(1), Some("x"));
        assert_eq!(r.str(2), None);
        assert_eq!(r.get(9), None);
        assert_eq!(r.line(), b"D,x,\xff,y,,");
        assert_eq!(r.len_trimmed(), 4);
        let r = s.split(b",,");
        assert_eq!(r.len_trimmed(), 0);
    }
}
