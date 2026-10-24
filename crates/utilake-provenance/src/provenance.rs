//! The provenance of one archived file: what its bytes are, and every place
//! AEMO served them.
//!
//! A file is held once. Its bytes never change, so [`Content`] never
//! changes; only the list of [`Address`]es grows, as the mirror finds the
//! same bytes served elsewhere (the other listing of a report folder, an
//! entry of a day's zip) and notes when a listing stops naming one. The
//! first address is how the bytes were obtained.
//!
//! A fact another fact gives is not written: a listing's size when it is
//! the file's, a listing's time when it is the server's `Last-Modified` cut
//! to the minute (IIS lists a file's last write time). [`Provenance::listed`]
//! gives them back.
//!
//! The written form is format 2. Format 1, the flat sidecar of the mirror
//! and the lake before it, with or without `also`, `gone_at`,
//! `extracted_from` and `crc32`, is read too and never written.

use serde::{Deserialize, Serialize};

use crate::content::{Content, Sha256};
use crate::layout;
use crate::stamp::Stamp;

/// One listing line as a folder page showed it. A field equal to what it
/// derives from is left out (see [`Provenance::listed`]).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Listed {
    /// The size the listing gave.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    /// The date and time the listing gave, in the host's clock.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<Stamp>,
}

/// What the server's response said of the bytes.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Served {
    /// The `ETag`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub etag: Option<String>,
    /// The `Last-Modified`: when AEMO published the bytes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub modified: Option<Stamp>,
}

/// Where in a zip an entry fetched alone sits, as the zip's central
/// directory said, so the bytes trace back to the server's.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Span {
    /// The zip's URL.
    pub zip: String,
    /// The entry's name in the zip.
    pub entry: String,
    /// The zip's size.
    pub zip_size: u64,
    /// Where the entry's local header starts in the zip.
    pub offset: u64,
    /// The entry's compressed size.
    pub compressed: u64,
    /// Its compression method (0 stored, 8 deflated).
    pub method: u16,
}

/// How an address came to serve the bytes. Each way carries what only it
/// knows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "via", rename_all = "snake_case")]
pub enum Via {
    /// Downloaded whole from the address.
    Fetched {
        /// The listing line it was downloaded under.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        listed: Option<Listed>,
        /// The response.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        served: Option<Served>,
    },
    /// One entry of a zip, fetched alone by its byte span.
    Range {
        /// Where in the zip.
        span: Span,
        /// The zip's listing line.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        listed: Option<Listed>,
        /// The response for the zip.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        served: Option<Served>,
    },
    /// The address served the same bytes (SHA-256 equal).
    Same {
        /// Its listing line when it was found the same, so a later version
        /// under it is told apart.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        listed: Option<Listed>,
    },
    /// The address is `{zip URL}!{entry}`, an entry with the file's name,
    /// size and CRC-32.
    Entry,
}

impl Via {
    /// Whether the bytes were obtained this way, not found again.
    #[must_use]
    pub const fn obtains(&self) -> bool {
        matches!(self, Self::Fetched { .. } | Self::Range { .. })
    }

    /// The listing line this way keeps.
    #[must_use]
    pub const fn listed(&self) -> Option<&Listed> {
        match self {
            Self::Fetched { listed, .. } | Self::Range { listed, .. } | Self::Same { listed } => {
                listed.as_ref()
            }
            Self::Entry => None,
        }
    }

    /// The response this way keeps.
    #[must_use]
    pub const fn served(&self) -> Option<&Served> {
        match self {
            Self::Fetched { served, .. } | Self::Range { served, .. } => served.as_ref(),
            Self::Same { .. } | Self::Entry => None,
        }
    }

    fn listed_mut(&mut self) -> Option<&mut Option<Listed>> {
        match self {
            Self::Fetched { listed, .. } | Self::Range { listed, .. } | Self::Same { listed } => {
                Some(listed)
            }
            Self::Entry => None,
        }
    }
}

/// One place the bytes are served.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Address {
    /// A file's URL, or `{zip URL}!{entry}` for an entry of a zip.
    pub url: String,
    /// When the bytes were fetched from it, or found to be served there.
    pub seen: Stamp,
    /// When the listing stopped naming it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gone: Option<Stamp>,
    /// How.
    #[serde(flatten)]
    pub via: Via,
}

/// What an archived file is and every place it was served: the record
/// `{name}.provenance.json` beside the file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Provenance {
    /// The bytes.
    #[serde(flatten)]
    pub content: Content,
    /// The program and version that first held them, e.g.
    /// `utilake-mirror/0.2.0`.
    pub tool: String,
    /// Every address, the one the bytes were obtained from first; no URL
    /// twice. Grows; never shrinks.
    pub addresses: Vec<Address>,
}

/// A record that breaks the rules of a provenance.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BadProvenance {
    /// No address at all.
    #[error("a provenance with no address")]
    NoAddress,
    /// The first address was found, not fetched.
    #[error("the first address `{0}` was not where the bytes were obtained")]
    NotObtained(String),
    /// A URL twice.
    #[error("the address `{0}` is named twice")]
    Twice(String),
    /// A format-1 time that does not read.
    #[error("`{field}` holds `{text}`, which is not a time")]
    Time {
        /// The field.
        field: &'static str,
        /// What it held.
        text: String,
    },
    /// A format-1 `also` entry found some way not known.
    #[error("`also` names `{0}`, which is neither `same` nor `entry`")]
    How(String),
}

impl Provenance {
    /// A provenance whose bytes were obtained at `origin`.
    #[must_use]
    pub fn new(content: Content, tool: impl Into<String>, mut origin: Address) -> Self {
        trim(&mut origin, &content);
        Self {
            content,
            tool: tool.into(),
            addresses: vec![origin],
        }
    }

    /// The address the bytes were obtained from.
    #[must_use]
    pub fn origin(&self) -> &Address {
        &self.addresses[0]
    }

    /// The URL the bytes were obtained from.
    #[must_use]
    pub fn url(&self) -> &str {
        &self.origin().url
    }

    /// When the bytes were obtained.
    #[must_use]
    pub fn fetched_at(&self) -> Stamp {
        self.origin().seen
    }

    /// What the server said when the bytes were obtained.
    #[must_use]
    pub fn served(&self) -> Option<&Served> {
        self.origin().via.served()
    }

    /// The `ETag` the bytes were obtained with.
    #[must_use]
    pub fn etag(&self) -> Option<&str> {
        self.served()?.etag.as_deref()
    }

    /// The `Last-Modified` the bytes were obtained with: when AEMO
    /// published them.
    #[must_use]
    pub fn modified(&self) -> Option<Stamp> {
        self.served()?.modified
    }

    /// The address `url`, when the bytes are served there.
    #[must_use]
    pub fn address(&self, url: &str) -> Option<&Address> {
        self.addresses.iter().find(|a| a.url == url)
    }

    /// Whether `url` is an address of these bytes.
    #[must_use]
    pub fn names(&self, url: &str) -> bool {
        self.address(url).is_some()
    }

    /// The first address that is an entry of a zip: where the bytes can
    /// still be fetched once their own listing drops them.
    #[must_use]
    pub fn archived_in(&self) -> Option<&Address> {
        self.addresses.iter().find(|a| a.via == Via::Entry)
    }

    /// Adds `address`, unless its URL is an address already. Whether it
    /// was added.
    pub fn add(&mut self, mut address: Address) -> bool {
        if self.names(&address.url) {
            return false;
        }
        trim(&mut address, &self.content);
        self.addresses.push(address);
        true
    }

    /// Notes that the listing no longer names `url`, unless that is noted
    /// already. Whether anything changed.
    pub fn gone(&mut self, url: &str, at: Stamp) -> bool {
        match self.addresses.iter_mut().find(|a| a.url == url) {
            Some(a) if a.gone.is_none() => {
                a.gone = Some(at);
                true
            }
            _ => false,
        }
    }

    /// The listing line of `url` in full: the size, the file's when left
    /// out; the time, the response's `Last-Modified` cut to the minute in
    /// the host's listing clock ([`layout::listing_offset`]) when left out.
    #[must_use]
    pub fn listed(&self, url: &str) -> Option<Listed> {
        let a = self.address(url)?;
        let listed = a.via.listed()?;
        Some(Listed {
            size: listed.size.or(match a.via {
                Via::Range { .. } => None,
                _ => Some(self.content.size),
            }),
            at: listed.at.or_else(|| derived_at(&a.url, a.via.served())),
        })
    }

    /// Whether these are the bytes the listing shows at `url` with `size`
    /// and `at`: the same size as the file, and the same listed time when
    /// both have one. nemweb rewrites some files in place under the same
    /// name; a listing that differs is a later version.
    #[must_use]
    pub fn listed_as(&self, url: &str, size: Option<u64>, at: Option<Stamp>) -> bool {
        if !self.names(url) || size.is_some_and(|s| s != self.content.size) {
            return false;
        }
        match (self.listed(url).and_then(|l| l.at), at) {
            (Some(a), Some(b)) => a.same_instant(b),
            _ => true,
        }
    }

    /// Holds the record to the rules: an address, the first one where the
    /// bytes were obtained, no URL twice.
    pub fn check(&self) -> Result<(), BadProvenance> {
        let first = self.addresses.first().ok_or(BadProvenance::NoAddress)?;
        if !first.via.obtains() {
            return Err(BadProvenance::NotObtained(first.url.clone()));
        }
        for (i, a) in self.addresses.iter().enumerate() {
            if self.addresses[..i].iter().any(|b| b.url == a.url) {
                return Err(BadProvenance::Twice(a.url.clone()));
            }
        }
        Ok(())
    }

    /// The record from its JSON, either format, held to the rules.
    pub fn from_json(bytes: &[u8]) -> Result<Self, ReadError> {
        Ok(Self::from_wire(serde_json::from_slice(bytes)?)?)
    }

    fn from_wire(wire: Wire) -> Result<Self, BadProvenance> {
        let p = match wire {
            Wire::V2(v2) => v2.into(),
            Wire::V1(v1) => Self::try_from(*v1)?,
        };
        p.check()?;
        Ok(p)
    }

    /// The record as format 2 JSON.
    #[must_use]
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("a provenance always serialises")
    }
}

/// Why a record does not read.
#[derive(Debug, thiserror::Error)]
pub enum ReadError {
    /// Not JSON of either format.
    #[error("not a provenance record: {0}")]
    Json(#[from] serde_json::Error),
    /// JSON of a format that breaks its rules.
    #[error(transparent)]
    Bad(#[from] BadProvenance),
}

/// The listing time a response gives for `url`: its `Last-Modified` cut to
/// the minute, in the clock the host lists in.
fn derived_at(url: &str, served: Option<&Served>) -> Option<Stamp> {
    let offset = layout::listing_offset(url);
    served?.modified.map(|m| m.to_minute().in_offset(offset))
}

/// Leaves out what `listed` would give back.
fn trim(a: &mut Address, content: &Content) {
    let served_at = derived_at(&a.url, a.via.served());
    let ranged = matches!(a.via, Via::Range { .. });
    if let Some(Some(l)) = a.via.listed_mut() {
        if !ranged && l.size == Some(content.size) {
            l.size = None;
        }
        if l.at.is_some() && l.at == served_at {
            l.at = None;
        }
    }
}

impl<'de> Deserialize<'de> for Provenance {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Self::from_wire(Wire::deserialize(d)?).map_err(serde::de::Error::custom)
    }
}

/// Either format, told apart by shape: format 2 has `addresses`.
#[derive(Deserialize)]
#[serde(untagged)]
enum Wire {
    V2(V2),
    V1(Box<V1>),
}

#[derive(Deserialize)]
struct V2 {
    #[serde(flatten)]
    content: Content,
    #[serde(default)]
    tool: String,
    addresses: Vec<Address>,
}

impl From<V2> for Provenance {
    fn from(v: V2) -> Self {
        Self {
            content: v.content,
            tool: v.tool,
            addresses: v.addresses,
        }
    }
}

/// Format 1: the flat sidecar. Times are text; one without its `_at`
/// instant is read from the text beside it, in the host's listing clock
/// when the text gives no offset.
#[derive(Deserialize)]
struct V1 {
    url: String,
    size: u64,
    sha256: Sha256,
    #[serde(default)]
    crc32: Option<u32>,
    fetched_at: String,
    #[serde(default)]
    etag: Option<String>,
    #[serde(default)]
    last_modified: Option<String>,
    #[serde(default)]
    last_modified_at: Option<String>,
    #[serde(default)]
    listed_size: Option<u64>,
    #[serde(default)]
    listed_modified: Option<String>,
    #[serde(default)]
    listed_modified_at: Option<String>,
    #[serde(default)]
    tool: String,
    #[serde(default)]
    gone_at: Option<String>,
    #[serde(default)]
    also: Vec<AlsoV1>,
    #[serde(default)]
    extracted_from: Option<ExtractedV1>,
}

#[derive(Deserialize)]
struct AlsoV1 {
    url: String,
    how: String,
    #[serde(default)]
    seen_at: Option<String>,
    #[serde(default)]
    gone_at: Option<String>,
    #[serde(default)]
    listed_at: Option<String>,
}

#[derive(Deserialize)]
struct ExtractedV1 {
    url: String,
    entry: String,
    zip_size: u64,
    offset: u64,
    compressed: u64,
    method: u16,
    crc32: u32,
}

/// A format-1 time: the instant when there is one, which must be RFC 3339
/// with an offset; else the text beside it read in any form, in `offset`
/// when it gives none. A text that is not a time is no time.
fn time_v1(
    field: &'static str,
    at: Option<&str>,
    text: Option<&str>,
    offset: i32,
) -> Result<Option<Stamp>, BadProvenance> {
    match at {
        Some(at) => Stamp::parse(at).map(Some).map_err(|_| BadProvenance::Time {
            field,
            text: at.to_owned(),
        }),
        None => Ok(text.and_then(|t| Stamp::read(t, offset))),
    }
}

impl TryFrom<V1> for Provenance {
    type Error = BadProvenance;

    fn try_from(v: V1) -> Result<Self, BadProvenance> {
        let clock = layout::listing_offset(&v.url);
        let seen = Stamp::parse(&v.fetched_at).map_err(|_| BadProvenance::Time {
            field: "fetched_at",
            text: v.fetched_at.clone(),
        })?;
        let modified = time_v1(
            "last_modified",
            v.last_modified_at.as_deref(),
            v.last_modified.as_deref(),
            clock,
        )?;
        let listed_at = time_v1(
            "listed_modified",
            v.listed_modified_at.as_deref(),
            v.listed_modified.as_deref(),
            clock,
        )?;
        let served = (v.etag.is_some() || modified.is_some()).then_some(Served {
            etag: v.etag,
            modified,
        });
        let listed = (v.listed_size.is_some() || listed_at.is_some()).then_some(Listed {
            size: v.listed_size,
            at: listed_at,
        });
        let mut crc32 = v.crc32;
        let via = match v.extracted_from {
            Some(x) => {
                crc32 = crc32.or(Some(x.crc32));
                Via::Range {
                    span: Span {
                        zip: x.url,
                        entry: x.entry,
                        zip_size: x.zip_size,
                        offset: x.offset,
                        compressed: x.compressed,
                        method: x.method,
                    },
                    listed,
                    served,
                }
            }
            None => Via::Fetched { listed, served },
        };
        let content = Content {
            size: v.size,
            sha256: v.sha256,
            crc32,
        };
        let origin = Address {
            url: v.url,
            seen,
            gone: time_v1("gone_at", v.gone_at.as_deref(), None, clock)?,
            via,
        };
        let mut p = Self::new(content, v.tool, origin);
        for a in v.also {
            let via = match a.how.as_str() {
                "same" => Via::Same {
                    listed: time_v1("also.listed_at", a.listed_at.as_deref(), None, clock)?.map(
                        |at| Listed {
                            size: None,
                            at: Some(at),
                        },
                    ),
                },
                "entry" => Via::Entry,
                _ => return Err(BadProvenance::How(a.how)),
            };
            let seen = time_v1("also.seen_at", a.seen_at.as_deref(), None, clock)?.unwrap_or(seen);
            p.add(Address {
                url: a.url,
                seen,
                gone: time_v1("also.gone_at", a.gone_at.as_deref(), None, clock)?,
                via,
            });
        }
        Ok(p)
    }
}

#[cfg(test)]
mod tests;
