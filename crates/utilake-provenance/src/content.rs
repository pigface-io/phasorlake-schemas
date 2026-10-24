//! What an archived file's bytes are: their size and digests. Written once,
//! when the bytes are first held, and never changed.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// A SHA-256 digest: 32 bytes, written as 64 lower-case hex digits.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Sha256(pub [u8; 32]);

/// Text that is not 64 hex digits.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("`{0}` is not a SHA-256 in 64 hex digits")]
pub struct BadSha256(pub String);

impl Sha256 {
    /// The digest of `bytes`.
    #[must_use]
    pub fn of(bytes: &[u8]) -> Self {
        use sha2::Digest as _;
        Self(sha2::Sha256::digest(bytes).into())
    }

    /// The digest as 64 lower-case hex digits.
    #[must_use]
    pub fn hex(&self) -> String {
        self.to_string()
    }
}

impl From<[u8; 32]> for Sha256 {
    fn from(b: [u8; 32]) -> Self {
        Self(b)
    }
}

impl fmt::Display for Sha256 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.iter().try_for_each(|b| write!(f, "{b:02x}"))
    }
}

impl fmt::Debug for Sha256 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Sha256({self})")
    }
}

impl FromStr for Sha256 {
    type Err = BadSha256;

    /// 64 hex digits, either case.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let bad = || BadSha256(s.chars().take(80).collect());
        let b = s.as_bytes();
        if b.len() != 64 {
            return Err(bad());
        }
        let nibble = |c: u8| (c as char).to_digit(16);
        let mut out = [0u8; 32];
        for (o, pair) in out.iter_mut().zip(b.chunks_exact(2)) {
            let (Some(h), Some(l)) = (nibble(pair[0]), nibble(pair[1])) else {
                return Err(bad());
            };
            // Two nibbles always fit a byte.
            *o = (h * 16 + l) as u8;
        }
        Ok(Self(out))
    }
}

impl Serialize for Sha256 {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Sha256 {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let text = <std::borrow::Cow<'de, str>>::deserialize(d)?;
        text.parse().map_err(serde::de::Error::custom)
    }
}

/// What a file's bytes are. Never changes once written: a later version
/// under the same name is another file with its own provenance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Content {
    /// Bytes.
    pub size: u64,
    /// SHA-256 of the bytes.
    pub sha256: Sha256,
    /// CRC-32 of the bytes, as zips record their entries'. A file held
    /// before the CRC-32 was kept has none until something reads it whole.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub crc32: Option<u32>,
}

impl Content {
    /// The content of `bytes`.
    #[must_use]
    pub fn of(bytes: &[u8]) -> Self {
        Self {
            size: bytes.len() as u64,
            sha256: Sha256::of(bytes),
            crc32: Some(crc32fast::hash(bytes)),
        }
    }

    /// The content of everything `reader` gives, read once in 1 MiB
    /// pieces so a file of any size takes the same memory. Blocking.
    pub fn of_reader(mut reader: impl std::io::Read) -> std::io::Result<Self> {
        use sha2::Digest as _;
        let (mut sha, mut crc) = (sha2::Sha256::new(), crc32fast::Hasher::new());
        let mut buf = vec![0u8; 1 << 20];
        let mut size = 0u64;
        loop {
            let n = reader.read(&mut buf)?;
            if n == 0 {
                break;
            }
            sha.update(&buf[..n]);
            crc.update(&buf[..n]);
            size += n as u64;
        }
        Ok(Self {
            size,
            sha256: Sha256(sha.finalize().into()),
            crc32: Some(crc.finalize()),
        })
    }

    /// The content of the file at `path`. Blocking.
    pub fn of_file(path: &std::path::Path) -> std::io::Result<Self> {
        Self::of_reader(std::fs::File::open(path)?)
    }

    /// Whether `other` is the same bytes: size and SHA-256 equal, and the
    /// CRC-32 when both have one.
    #[must_use]
    pub fn same(&self, other: &Self) -> bool {
        self.size == other.size
            && self.sha256 == other.sha256
            && match (self.crc32, other.crc32) {
                (Some(a), Some(b)) => a == b,
                _ => true,
            }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ABC: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";

    #[test]
    fn hex_both_ways() {
        let d = Sha256::of(b"abc");
        assert_eq!(d.hex(), ABC);
        assert_eq!(format!("{d:?}"), format!("Sha256({ABC})"));
        assert_eq!(ABC.parse::<Sha256>(), Ok(d));
        assert_eq!(ABC.to_ascii_uppercase().parse::<Sha256>(), Ok(d));
        assert_eq!(Sha256::from(d.0), d);
        for bad in ["", "ab", &ABC[1..], &format!("{}g", &ABC[1..])] {
            assert_eq!(
                bad.parse::<Sha256>(),
                Err(BadSha256(bad.to_owned())),
                "{bad}"
            );
        }
        assert!(BadSha256("x".into()).to_string().contains("64 hex digits"));
    }

    #[test]
    fn content_of_bytes_a_reader_and_a_file() {
        let c = Content::of(b"abc");
        assert_eq!(
            (c.size, c.sha256.hex().as_str(), c.crc32),
            (3, ABC, Some(0x3524_41c2))
        );
        assert_eq!(Content::of_reader(&b"abc"[..]).unwrap(), c);
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("f");
        std::fs::write(&p, b"abc").unwrap();
        assert_eq!(Content::of_file(&p).unwrap(), c);
        assert!(Content::of_file(&dir.path().join("none")).is_err());
        assert!(c.same(&Content { crc32: None, ..c }));
        assert!(!c.same(&Content {
            crc32: Some(1),
            ..c
        }));
        assert!(!c.same(&Content::of(b"abd")));
        assert!(!c.same(&Content { size: 4, ..c }));
        // A reader that fails part way.
        struct Broken;
        impl std::io::Read for Broken {
            fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
                Err(std::io::Error::other("broken"))
            }
        }
        assert!(Content::of_reader(Broken).is_err());
    }

    #[test]
    fn content_serialises_flat_and_short() {
        let c = Content {
            size: 3,
            sha256: Sha256::of(b"abc"),
            crc32: None,
        };
        let json = serde_json::to_string(&c).unwrap();
        assert_eq!(json, format!(r#"{{"size":3,"sha256":"{ABC}"}}"#));
        assert_eq!(serde_json::from_str::<Content>(&json).unwrap(), c);
        let with = Content {
            crc32: Some(7),
            ..c
        };
        let json = serde_json::to_string(&with).unwrap();
        assert!(json.ends_with(r#""crc32":7}"#), "{json}");
        assert_eq!(serde_json::from_str::<Content>(&json).unwrap(), with);
        assert!(serde_json::from_str::<Content>(r#"{"size":3,"sha256":"x"}"#).is_err());
        assert!(serde_json::from_str::<Content>(r#"{"size":3,"sha256":1}"#).is_err());
    }
}
