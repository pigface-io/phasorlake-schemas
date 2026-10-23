//! What a file's first bytes say it is.
//!
//! A feed's files are read by the feed's decoder, but a folder holds the
//! odd file of another kind: the GBB folder carries one MMS C/I/D file
//! (the GSH trades), and the STTM and DWGM folders each carry an HTML
//! fragment named `.csv`. The census and the lake both ask these bytes
//! before reading, so the two agree on what each file is.

use std::io::{self, Read};

/// How many bytes [`sniff`] needs at most.
pub const HEAD: usize = 16;

/// What the first bytes of a file say it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    /// MMS C/I/D rows: the file opens with a `C,` row.
    Mms,
    /// A header row then data rows: anything that is not the other two.
    Csv,
    /// HTML or XML: the file opens with `<`.
    Markup,
}

/// What `head` (the first bytes of a file, a UTF-8 byte-order mark and
/// leading white space allowed) says the file is.
pub fn sniff(head: &[u8]) -> Format {
    let head = head.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(head);
    let start = head.iter().position(|b| !b.is_ascii_whitespace());
    match start.map(|i| &head[i..]) {
        Some([b'C', b',', ..]) => Format::Mms,
        Some([b'<', ..]) => Format::Markup,
        _ => Format::Csv,
    }
}

/// Reads up to [`HEAD`] bytes from `reader` and returns them with a reader
/// that yields them again before the rest, so the file is read whole.
pub fn peek<R: Read>(mut reader: R) -> io::Result<(Vec<u8>, impl Read)> {
    let mut head = vec![0; HEAD];
    let mut got = 0;
    while got < HEAD {
        match reader.read(&mut head[got..]) {
            Ok(0) => break,
            Ok(n) => got += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    head.truncate(got);
    let rest = io::Cursor::new(head.clone()).chain(reader);
    Ok((head, rest))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_bytes_name_the_format() {
        assert_eq!(sniff(b"C,NEMP.WORLD,GSH_GAS_TRADES_WEB"), Format::Mms);
        assert_eq!(sniff(b"\xEF\xBB\xBFC,NEMP"), Format::Mms);
        assert_eq!(sniff(b"<!--\nint685"), Format::Markup);
        assert_eq!(sniff(b"\n  <div>"), Format::Markup);
        assert_eq!(sniff(b"GasDate,FacilityId"), Format::Csv);
        assert_eq!(sniff(b"Column,Other"), Format::Csv);
        assert_eq!(sniff(b"C"), Format::Csv);
        assert_eq!(sniff(b""), Format::Csv);
        assert_eq!(sniff(b"   "), Format::Csv);
    }

    #[test]
    fn peeking_leaves_the_file_whole() {
        let text = b"C,NEMP.WORLD,GSH_GAS_TRADES_WEB,AEMO,PUBLIC\nI,GSH,GAS_TRADES,1\n";
        let (head, mut rest) = peek(&text[..]).unwrap();
        assert_eq!(head, &text[..HEAD]);
        assert_eq!(sniff(&head), Format::Mms);
        let mut all = Vec::new();
        rest.read_to_end(&mut all).unwrap();
        assert_eq!(all, text);
        // Shorter than a head: all of it.
        let (head, mut rest) = peek(&b"a,b\n"[..]).unwrap();
        assert_eq!(head, b"a,b\n");
        let mut all = Vec::new();
        rest.read_to_end(&mut all).unwrap();
        assert_eq!(all, b"a,b\n");
    }

    struct Flaky(usize, &'static [u8]);

    impl Read for Flaky {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            self.0 += 1;
            match self.0 {
                1 => Err(io::Error::from(io::ErrorKind::Interrupted)),
                2 => {
                    let n = self.1.len().min(buf.len()).min(3);
                    buf[..n].copy_from_slice(&self.1[..n]);
                    self.1 = &self.1[n..];
                    Ok(n)
                }
                _ => Err(io::Error::other("gone")),
            }
        }
    }

    #[test]
    fn peeking_retries_an_interruption_and_reports_a_failure() {
        let err = peek(Flaky(0, b"abcdef")).err().map(|e| e.to_string());
        assert_eq!(err.as_deref(), Some("gone"));
    }
}
