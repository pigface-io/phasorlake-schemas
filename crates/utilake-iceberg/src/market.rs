//! The markets AEMO publishes: each with its own clock.

use std::fmt;

use serde::{Deserialize, Serialize};

/// A market whose tables are published: the directory its tables sit in
/// (`nem/`, `wem/`, `gas/`) and the clock its files' times are in.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default, Serialize, Deserialize,
)]
#[serde(rename_all = "lowercase")]
pub enum Market {
    /// The National Electricity Market: eastern Australia, +10:00.
    #[default]
    Nem,
    /// The Wholesale Electricity Market: Western Australia, +08:00.
    Wem,
    /// AEMO's east-coast gas markets (GBB, GSH, STTM, DWGM): +10:00.
    Gas,
}

impl Market {
    /// Every market, in publication order.
    pub const ALL: [Self; 3] = [Self::Nem, Self::Wem, Self::Gas];

    /// The market's id: its directory under the published root.
    pub const fn id(self) -> &'static str {
        match self {
            Self::Nem => "nem",
            Self::Wem => "wem",
            Self::Gas => "gas",
        }
    }

    /// The market's offset from UTC, in seconds. AEMO publishes in
    /// standard time, never daylight time.
    pub const fn offset_secs(self) -> i32 {
        match self {
            Self::Nem | Self::Gas => 10 * 3600,
            Self::Wem => 8 * 3600,
        }
    }

    /// The offset as RFC 3339 writes it: `+10:00`.
    pub const fn offset_text(self) -> &'static str {
        match self {
            Self::Nem | Self::Gas => "+10:00",
            Self::Wem => "+08:00",
        }
    }

    /// The market with this id.
    pub fn parse(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|m| m.id() == id)
    }
}

impl fmt::Display for Market {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.id())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_offsets_and_text() {
        for m in Market::ALL {
            assert_eq!(Market::parse(m.id()), Some(m));
            assert_eq!(m.to_string(), m.id());
            assert_eq!(
                serde_json::to_string(&m).unwrap(),
                format!("\"{}\"", m.id())
            );
            assert_eq!(
                serde_json::from_str::<Market>(&format!("\"{}\"", m.id())).unwrap(),
                m
            );
            let (h, rest) = m.offset_text()[1..].split_once(':').unwrap();
            assert_eq!(h.parse::<i32>().unwrap() * 3600, m.offset_secs());
            assert_eq!(rest, "00");
        }
        assert_eq!(Market::parse("x"), None);
        assert_eq!(Market::default(), Market::Nem);
        assert_eq!(Market::Wem.offset_secs(), 28_800);
    }
}
