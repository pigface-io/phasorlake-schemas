//! The area of a system a diagnostic belongs to: the first segment of its
//! code.

use std::borrow::Cow;
use std::fmt;

use serde::{Deserialize, Serialize};

/// The area of the system a diagnostic belongs to, a lower-case snake name
/// such as `csv`. Serialised as that name.
///
/// Categories are open: the ones every repository shares are the consts
/// below, and a repository declares more with [`Category::new`], which
/// refuses a name that is not lower-case snake at compile time when used in
/// a `const`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
pub struct Category(Cow<'static, str>);

impl Category {
    /// Configuration file, flags, environment.
    pub const CONFIG: Self = Self::new("config");
    /// HTTP transport.
    pub const NET: Self = Self::new("net");
    /// Directory listings and file-name resolution.
    pub const LISTING: Self = Self::new("listing");
    /// The local download cache and the records beside its files.
    pub const CACHE: Self = Self::new("cache");
    /// Zip archives.
    pub const ZIP: Self = Self::new("zip");
    /// The C/I/D row structure of an MMS CSV, or the header and rows of a
    /// plain one.
    pub const CSV: Self = Self::new("csv");
    /// A WEMDE JSON document's structure.
    pub const JSON: Self = Self::new("json");
    /// A value failing the type it is read as.
    pub const PARSE: Self = Self::new("parse");
    /// Schemas: resolution, drift, validation.
    pub const SCHEMA: Self = Self::new("schema");
    /// Writing tables: Parquet files and Iceberg metadata.
    pub const STORE: Self = Self::new("store");
    /// The SQLite manifest.
    pub const MANIFEST: Self = Self::new("manifest");
    /// Verification findings.
    pub const VERIFY: Self = Self::new("verify");
    /// Reading the lake.
    pub const QUERY: Self = Self::new("query");
    /// A bug (panic, broken invariant).
    pub const INTERNAL: Self = Self::new("internal");

    /// The categories every repository shares, in the order a catalogue
    /// lists them.
    pub const COMMON: [Self; 14] = [
        Self::CONFIG,
        Self::NET,
        Self::LISTING,
        Self::CACHE,
        Self::ZIP,
        Self::CSV,
        Self::JSON,
        Self::PARSE,
        Self::SCHEMA,
        Self::STORE,
        Self::MANIFEST,
        Self::VERIFY,
        Self::QUERY,
        Self::INTERNAL,
    ];

    /// A category named `name`.
    ///
    /// # Panics
    ///
    /// When `name` is not lower-case snake (`[a-z][a-z0-9_]*`); in a
    /// `const`, that is a compile error.
    #[must_use]
    pub const fn new(name: &'static str) -> Self {
        assert!(is_snake(name), "a category is a lower-case snake name");
        Self(Cow::Borrowed(name))
    }

    /// A category named `name` read at run time, or `None` when the name
    /// is not lower-case snake.
    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        is_snake(name).then(|| Self(Cow::Owned(name.to_owned())))
    }

    /// The name, the first segment of a code.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Whether `s` is `[a-z][a-z0-9_]*`.
pub(crate) const fn is_snake(s: &str) -> bool {
    let b = s.as_bytes();
    if b.is_empty() || !b[0].is_ascii_lowercase() {
        return false;
    }
    let mut i = 1;
    while i < b.len() {
        if !(b[i].is_ascii_lowercase() || b[i].is_ascii_digit() || b[i] == b'_') {
            return false;
        }
        i += 1;
    }
    true
}

impl fmt::Display for Category {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for Category {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let name = String::deserialize(d)?;
        Self::parse(&name)
            .ok_or_else(|| serde::de::Error::custom(format!("{name:?} is not a category")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINE: Category = Category::new("census");

    #[test]
    fn a_category_is_its_name() {
        assert_eq!(MINE.as_str(), "census");
        assert_eq!(Category::CSV.to_string(), "csv");
        assert_eq!(Category::COMMON.len(), 14);
        assert_eq!(Category::parse("a1_b"), Some(Category::new("a1_b")));
        for bad in ["", "Csv", "1a", "a-b", "a.b", "_a"] {
            assert_eq!(Category::parse(bad), None, "{bad}");
        }
        let json = serde_json::to_string(&Category::JSON).unwrap();
        assert_eq!(json, "\"json\"");
        let back: Category = serde_json::from_str(&json).unwrap();
        assert_eq!(back, Category::JSON);
        let err = serde_json::from_str::<Category>("\"Not\"").unwrap_err();
        assert!(err.to_string().contains("is not a category"));
        assert!(serde_json::from_str::<Category>("1").is_err());
    }

    #[test]
    #[should_panic(expected = "lower-case snake")]
    fn a_name_that_is_not_snake_is_refused() {
        let _ = Category::new(String::new().leak());
    }
}
