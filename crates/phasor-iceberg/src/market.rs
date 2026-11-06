//! The markets published: AEMO's three and New Zealand's, each with its
//! own clock.

use std::fmt;

use serde::{Deserialize, Serialize};

/// A market whose tables are published: the directory its tables sit in
/// (`nem/`, `wem/`, `gas/`, `nz/`) and the clock its files' times are in.
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
    /// New Zealand's wholesale electricity market, as the Electricity
    /// Authority publishes it: New Zealand Standard Time, +12:00. Its
    /// files key a row by trading date and trading period, and carry
    /// instants with their own offset (+12:00 or +13:00 in daylight time).
    Nz,
}

impl Market {
    /// Every market, in publication order.
    pub const ALL: [Self; 4] = [Self::Nem, Self::Wem, Self::Gas, Self::Nz];

    /// The market's id: its directory under the published root.
    pub const fn id(self) -> &'static str {
        match self {
            Self::Nem => "nem",
            Self::Wem => "wem",
            Self::Gas => "gas",
            Self::Nz => "nz",
        }
    }

    /// The market's offset from UTC, in seconds: the standard time its
    /// tables' timestamps are displayed in. AEMO publishes in standard
    /// time, never daylight time; New Zealand's files are read into NZST.
    pub const fn offset_secs(self) -> i32 {
        match self {
            Self::Nem | Self::Gas => 10 * 3600,
            Self::Wem => 8 * 3600,
            Self::Nz => 12 * 3600,
        }
    }

    /// The offset as RFC 3339 writes it: `+10:00`.
    pub const fn offset_text(self) -> &'static str {
        match self {
            Self::Nem | Self::Gas => "+10:00",
            Self::Wem => "+08:00",
            Self::Nz => "+12:00",
        }
    }

    /// Whether every timestamp column of the market's tables has a sibling
    /// `{COLUMN}_UTC_US`, the same instant as microseconds since 1970 UTC
    /// (a `long`). Every market's do: New Zealand's from the start, AEMO's
    /// since the release that followed (their files are read again for it).
    pub const fn derives_utc_siblings(self) -> bool {
        match self {
            Self::Nem | Self::Wem | Self::Gas | Self::Nz => true,
        }
    }

    /// The market with this id.
    pub fn parse(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|m| m.id() == id)
    }
}

impl std::str::FromStr for Market {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s).ok_or_else(|| format!("{s:?} is not a market: nem, wem, gas or nz"))
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
        assert_eq!("gas".parse::<Market>(), Ok(Market::Gas));
        assert!("x".parse::<Market>().unwrap_err().contains("not a market"));
        assert_eq!(Market::default(), Market::Nem);
        assert_eq!(Market::Wem.offset_secs(), 28_800);
        assert_eq!(Market::Nz.offset_secs(), 43_200);
        assert_eq!("nz".parse::<Market>(), Ok(Market::Nz));
        assert!(Market::ALL.iter().all(|m| m.derives_utc_siblings()));
    }
}
