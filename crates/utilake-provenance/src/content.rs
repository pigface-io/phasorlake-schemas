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
