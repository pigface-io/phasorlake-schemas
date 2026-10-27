//! The tables this repository declares, one hand-kept file each.
//!
//! ```text
//! tables/DISPATCH_UNIT_SCADA.toml
//! ```
//!
//! A declaration says which report pairs a table is published under, its
//! key and time column, its temporal roles when a person set them, how it is
//! loaded, its visibility, and any column whose type is set by hand
//! (`[[override]]`, applied by the fold). The census asks it for the table
//! of a report pair and for that table's time column; a pair no table
//! declares is recorded under `{TYPE}_{SUBTYPE}` (`{TYPE}` when the
//! subtype is empty).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use phasor_iceberg::IcebergType;
use phasor_iceberg::Market;
use serde::{Deserialize, Serialize};

/// Why the declarations cannot be read or written.
#[derive(Debug, thiserror::Error)]
pub enum CatalogError {
    /// A file or directory that cannot be read.
    #[error("the table declarations at {path} cannot be read: {message}")]
    Read {
        /// The file or directory.
        path: String,
        /// What is wrong.
        message: String,
    },
    /// A file that is not a declaration.
    #[error("the table declaration {path} is not valid: {message}")]
    Invalid {
        /// The file.
        path: String,
        /// What is wrong.
        message: String,
    },
    /// A report pair two tables declare with the same number of
    /// `with_columns`, so a block could be either's.
    #[error("the report pair {pair} is declared by both {first} and {second}")]
    Duplicate {
        /// The pair, `TYPE/SUBTYPE`.
        pair: String,
        /// The file that declared it first.
        first: String,
        /// The file that declared it again.
        second: String,
    },
    /// A file that cannot be written.
    #[error("the table declaration {path} could not be written")]
    Write {
        /// The file.
        path: String,
        /// Cause.
        #[source]
        source: std::io::Error,
    },
}

/// How a table's files are loaded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LoadMode {
    /// Every file adds rows.
    Append,
    /// Every file is the whole table as of its time.
    Snapshot,
}

/// A column whose type is set by hand, whatever its values say.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Override {
    /// The column.
    pub column: String,
    /// Its Iceberg type, as Iceberg writes it (`long`, `decimal(16, 6)`).
    #[serde(rename = "type")]
    pub ty: String,
    /// Why.
    pub reason: String,
}

impl Override {
    /// The type; `None` when it is not one (a file that holds such an
    /// override is refused by [`Catalog::read`]).
    pub fn iceberg_type(&self) -> Option<IcebergType> {
        IcebergType::parse(&self.ty)
    }
}

/// A column AEMO renamed: the same column under a new name from some
/// release on, so one field whose name changes (Iceberg's rename), not a
/// column dropped and another added.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rename {
    /// The name it had.
    pub from: String,
    /// The name it has.
    pub to: String,
    /// What says it is a rename: an upgrade script's `RENAME COLUMN`, a
    /// release note.
    pub evidence: String,
}

/// Columns that look like a rename and are not: one dropped and another
/// of its type added at once, each its own column (a split, a
/// redefinition, a replacement, or two names one block holds together).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Distinct {
    /// The columns.
    pub columns: Vec<String>,
    /// Why they are not one renamed column.
    pub reason: String,
}

/// One table's declaration, as written in `tables/{name}.toml`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TableDecl {
    /// The table.
    pub name: String,
    /// The package of AEMO's data model it is in (`DISPATCH`), as the
    /// MMS Data Model Report places it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub package: Option<String>,
    /// The report pairs (type, subtype) its blocks are published under.
    pub report_pairs: Vec<[String; 2]>,
    /// The columns a block of those pairs must hold to be this table's:
    /// empty for every block of them. Where AEMO publishes two kinds of
    /// row under one pair (a region's and an area's), the table of the
    /// narrower kind names the column that marks it (`AREAID`), and a
    /// block goes to the most specific table whose columns it holds.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub with_columns: Vec<String>,
    /// Its key, in order; empty for none.
    pub primary_key: Vec<String>,
    /// The column a block's time span is of.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time_column: Option<String>,
    /// How its files are loaded.
    pub load_mode: LoadMode,
    /// Who may see it, as AEMO's control files say (`PUBLIC`).
    pub visibility: String,
    /// Its temporal roles (`valid`, `entity`, `order`, `cutoff`) when a
    /// person set them, as written; absent, the publisher derives them
    /// from the key and the columns ([`phasor_iceberg::derive()`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temporal: Option<toml::Table>,
    /// Columns typed by hand.
    #[serde(default, rename = "override", skip_serializing_if = "Vec::is_empty")]
    pub overrides: Vec<Override>,
    /// Columns AEMO renamed, each one field under both names.
    #[serde(default, rename = "rename", skip_serializing_if = "Vec::is_empty")]
    pub renames: Vec<Rename>,
    /// Columns that look renamed and are not.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub distinct: Vec<Distinct>,
    /// The tables of AEMO's data model whose columns say what this
    /// table's are, when the table's own name is not in the model (a
    /// published file that is a view of model tables).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub model: Vec<String>,
}

impl TableDecl {
    /// What is wrong with the declaration on its own, if anything.
    fn problem(&self) -> Option<String> {
        if self.name.is_empty() || self.report_pairs.is_empty() {
            return Some("it needs a name and at least one report pair".into());
        }
        let mut seen = Vec::new();
        for o in &self.overrides {
            if o.iceberg_type().is_none() {
                return Some(format!(
                    "the override of {} has type {:?}, which is not an Iceberg type",
                    o.column, o.ty
                ));
            }
            if o.reason.trim().is_empty() || seen.contains(&&o.column) {
                return Some(format!(
                    "the override of {} needs a reason and may appear once",
                    o.column
                ));
            }
            seen.push(&o.column);
        }
        if let Some(d) = self
            .distinct
            .iter()
            .find(|d| d.columns.len() < 2 || d.reason.trim().is_empty())
        {
            return Some(format!(
                "the distinct columns {:?} need two or more columns and a reason",
                d.columns
            ));
        }
        for r in &self.renames {
            if r.from.is_empty()
                || r.to.is_empty()
                || r.from == r.to
                || r.evidence.trim().is_empty()
            {
                return Some(format!(
                    "the rename of {} to {} needs two different names and its evidence",
                    r.from, r.to
                ));
            }
        }
        None
    }

    /// The declaration as its file holds it.
    pub fn to_toml(&self) -> String {
        format!(
            "# The declaration of {}: kept by hand.\n{}",
            self.name,
            toml::to_string(self).expect("a declaration serialises")
        )
    }
}

/// Every declared table, by name, and the table of each report pair.
#[derive(Debug, Clone, Default)]
pub struct Catalog {
    tables: Vec<TableDecl>,
    /// The tables of each pair, the most `with_columns` first.
    by_pair: HashMap<(String, String), Vec<usize>>,
}

impl Catalog {
    /// The declarations, each with the file it came from (for an error),
    /// in name order. Two tables of one pair with as many `with_columns`
    /// as each other are refused: a block could be either's.
    pub fn new(tables: Vec<(PathBuf, TableDecl)>) -> Result<Self, CatalogError> {
        let mut tables = tables;
        tables.sort_by(|a, b| a.1.name.cmp(&b.1.name));
        let mut by_pair: HashMap<(String, String), Vec<usize>> = HashMap::new();
        for (i, (path, t)) in tables.iter().enumerate() {
            for [a, b] in &t.report_pairs {
                let of_pair = by_pair.entry((a.clone(), b.clone())).or_default();
                if let Some(&j) = of_pair
                    .iter()
                    .find(|&&j| tables[j].1.with_columns.len() == t.with_columns.len())
                {
                    return Err(CatalogError::Duplicate {
                        pair: format!("{a}/{b}"),
                        first: tables[j].0.display().to_string(),
                        second: path.display().to_string(),
                    });
                }
                of_pair.push(i);
                of_pair.sort_by_key(|&j| std::cmp::Reverse(tables[j].1.with_columns.len()));
            }
        }
        Ok(Self {
            tables: tables.into_iter().map(|(_, t)| t).collect(),
            by_pair,
        })
    }

    /// Every `*.toml` file of `dir`, in name order. A file must be named
    /// for its table.
    pub fn read(dir: &Path) -> Result<Self, CatalogError> {
        let mut tables = Vec::new();
        for path in toml_files(dir)? {
            let text = std::fs::read_to_string(&path).map_err(|e| read_error(&path, &e))?;
            let invalid = |message: String| CatalogError::Invalid {
                path: path.display().to_string(),
                message,
            };
            let t: TableDecl = toml::from_str(&text).map_err(|e| invalid(e.to_string()))?;
            let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned());
            if stem.as_deref() != Some(t.name.as_str()) {
                return Err(invalid(format!(
                    "it declares {}, not its file name",
                    t.name
                )));
            }
            if let Some(p) = t.problem() {
                return Err(invalid(p));
            }
            tables.push((path, t));
        }
        Self::new(tables)
    }

    /// Every table, in name order.
    pub fn tables(&self) -> &[TableDecl] {
        &self.tables
    }

    /// The table of this name.
    pub fn table(&self, name: &str) -> Option<&TableDecl> {
        self.tables.iter().find(|t| t.name == name)
    }

    /// The table that declares a report pair and whose `with_columns` a
    /// block of these `columns` holds, the most specific first.
    pub fn declared_by_pair<S: AsRef<str>>(
        &self,
        report_type: &str,
        report_subtype: &str,
        columns: &[S],
    ) -> Option<&TableDecl> {
        let holds = |c: &String| columns.iter().any(|x| x.as_ref().eq_ignore_ascii_case(c));
        self.by_pair
            .get(&(report_type.to_owned(), report_subtype.to_owned()))?
            .iter()
            .map(|&i| &self.tables[i])
            .find(|t| t.with_columns.iter().all(holds))
    }

    /// The table a block of this report pair is recorded under: the one
    /// declared for the pair, else `{TYPE}_{SUBTYPE}` (`{TYPE}` alone when
    /// the subtype is empty, as a report file's `DREGION` block is) with
    /// any character that is not a letter, digit or underscore written as
    /// `_` (a plain feed's name may carry a hyphen or a dot; an MMS pair
    /// never does).
    pub fn table_of<S: AsRef<str>>(
        &self,
        report_type: &str,
        report_subtype: &str,
        columns: &[S],
    ) -> String {
        self.declared_by_pair(report_type, report_subtype, columns)
            .map_or_else(
                || {
                    let pair = if report_subtype.is_empty() {
                        report_type.to_owned()
                    } else {
                        format!("{report_type}_{report_subtype}")
                    };
                    pair.chars()
                        .map(|c| {
                            if c.is_ascii_alphanumeric() || c == '_' {
                                c
                            } else {
                                '_'
                            }
                        })
                        .collect()
                },
                |t| t.name.clone(),
            )
    }

    /// Writes each declaration to `dir/{name}.toml`; returns how many.
    pub fn write(&self, dir: &Path) -> Result<usize, CatalogError> {
        for t in &self.tables {
            let path = dir.join(format!("{}.toml", t.name));
            crate::files::write_atomic(&path, t.to_toml().as_bytes()).map_err(|source| {
                CatalogError::Write {
                    path: path.display().to_string(),
                    source,
                }
            })?;
        }
        Ok(self.tables.len())
    }
}

fn read_error(path: &Path, e: &std::io::Error) -> CatalogError {
    CatalogError::Read {
        path: path.display().to_string(),
        message: e.to_string(),
    }
}

/// The `*.toml` files directly in `dir`, in name order.
/// The directory of a market's declarations under the repository root:
/// `tables/{market}/`.
pub fn tables_dir(root: &Path, market: Market) -> PathBuf {
    root.join("tables").join(market.id())
}

fn toml_files(dir: &Path) -> Result<Vec<PathBuf>, CatalogError> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(dir).map_err(|e| read_error(dir, &e))? {
        let p = e.map_err(|e| read_error(dir, &e))?.path();
        if p.is_file() && p.extension().is_some_and(|x| x == "toml") {
            out.push(p);
        }
    }
    out.sort();
    Ok(out)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A declaration of `name` for one pair.
    /// A block's columns, where they do not matter.
    const NONE: &[&str] = &[];

    pub(crate) fn decl(name: &str, pair: (&str, &str), time: Option<&str>) -> TableDecl {
        TableDecl {
            name: name.into(),
            package: None,
            report_pairs: vec![[pair.0.into(), pair.1.into()]],
            with_columns: Vec::new(),
            primary_key: vec!["SETTLEMENTDATE".into()],
            time_column: time.map(str::to_owned),
            load_mode: LoadMode::Append,
            visibility: "PUBLIC".into(),
            temporal: None,
            overrides: Vec::new(),
            renames: Vec::new(),
            distinct: Vec::new(),
            model: Vec::new(),
        }
    }

    /// The catalog of one table: DISPATCH_UNIT_SCADA, timed by
    /// SETTLEMENTDATE.
    pub(crate) fn scada() -> Catalog {
        Catalog::new(vec![(
            "DISPATCH_UNIT_SCADA.toml".into(),
            decl(
                "DISPATCH_UNIT_SCADA",
                ("DISPATCH", "UNIT_SCADA"),
                Some("SETTLEMENTDATE"),
            ),
        )])
        .unwrap()
    }

    const SCADA: &str = r#"
name = "DISPATCH_UNIT_SCADA"
report_pairs = [["DISPATCH", "UNIT_SCADA"]]
primary_key = ["SETTLEMENTDATE", "DUID"]
time_column = "SETTLEMENTDATE"
load_mode = "append"
visibility = "PUBLIC"

[temporal]
entity = ["DUID"]
valid = { kind = "interval_end", column = "SETTLEMENTDATE" }
order = ["LASTCHANGED"]
cutoff = "LASTCHANGED"

[[override]]
column = "SCADAVALUE"
type = "decimal(16, 6)"
reason = "Six places are what AEMO measures."
"#;

    #[test]
    fn declarations_are_read_and_answer_for_their_pairs() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("DISPATCH_UNIT_SCADA.toml"), SCADA).unwrap();
        let price = decl("DISPATCHPRICE", ("DISPATCH", "PRICE"), None);
        std::fs::write(dir.path().join("DISPATCHPRICE.toml"), price.to_toml()).unwrap();
        std::fs::write(dir.path().join("README.md"), "not a table").unwrap();
        std::fs::create_dir(dir.path().join("sub.toml")).unwrap();
        let c = Catalog::read(dir.path()).unwrap();
        assert_eq!(c.tables().len(), 2);
        let t = c.declared_by_pair("DISPATCH", "UNIT_SCADA", NONE).unwrap();
        assert_eq!(t.time_column.as_deref(), Some("SETTLEMENTDATE"));
        assert_eq!(t.primary_key, ["SETTLEMENTDATE", "DUID"]);
        let roles = t.temporal.as_ref().unwrap();
        assert_eq!(roles["cutoff"].as_str(), Some("LASTCHANGED"));
        assert!(
            c.table("DISPATCHPRICE").unwrap().temporal.is_none(),
            "no block: the publisher derives the roles"
        );
        assert_eq!(
            t.overrides[0].iceberg_type(),
            IcebergType::decimal(16, 6),
            "{:?}",
            t.overrides
        );
        assert_eq!(c.table("DISPATCHPRICE"), Some(&price));
        assert!(c.table("NOBODY").is_none());
        assert_eq!(c.table_of("DISPATCH", "PRICE", NONE), "DISPATCHPRICE");
        assert_eq!(c.table_of("DISPATCH", "OTHER", NONE), "DISPATCH_OTHER");
        assert_eq!(
            c.table_of("WEM", "FACILITY-SCADA", NONE),
            "WEM_FACILITY_SCADA"
        );
        assert_eq!(c.table_of("WEMDE", "A.B", NONE), "WEMDE_A_B");
        assert_eq!(c.table_of("DREGION", "", NONE), "DREGION");
        // Written back, a declaration reads as it was.
        let out = dir.path().join("out");
        assert_eq!(c.write(&out).unwrap(), 2);
        let again = Catalog::read(&out).unwrap();
        assert_eq!(again.tables(), c.tables());
        let text = std::fs::read_to_string(out.join("DISPATCH_UNIT_SCADA.toml")).unwrap();
        assert!(
            text.starts_with("# The declaration of DISPATCH_UNIT_SCADA"),
            "{text}"
        );
        assert!(text.contains("[[override]]"), "{text}");
    }

    #[test]
    fn a_bad_declaration_names_its_file() {
        let dir = tempfile::tempdir().unwrap();
        let err = |name: &str, text: &str| {
            let d = dir.path().join(name.replace('.', "_"));
            std::fs::create_dir_all(&d).unwrap();
            std::fs::write(d.join(name), text).unwrap();
            Catalog::read(&d).unwrap_err().to_string()
        };
        let e = err("X.toml", "name = 1");
        assert!(e.contains("X.toml is not valid"), "{e}");
        let e = err("Y.toml", &SCADA.replace("DISPATCH_UNIT_SCADA\"", "Z\""));
        assert!(e.contains("it declares Z, not its file name"), "{e}");
        let named = |s: &str| s.replace("\"DISPATCH_UNIT_SCADA\"", "\"T\"");
        let e = err("T.toml", &named(SCADA).replace("decimal(16, 6)", "money"));
        assert!(e.contains("\"money\", which is not an Iceberg type"), "{e}");
        let e = err(
            "T.toml",
            &named(SCADA).replace("Six places are what AEMO measures.", " "),
        );
        assert!(e.contains("needs a reason"), "{e}");
        let twice = format!(
            "{}\n[[override]]\ncolumn = \"SCADAVALUE\"\ntype = \"long\"\nreason = \"r\"\n",
            named(SCADA)
        );
        assert!(err("T.toml", &twice).contains("may appear once"));
        let e = err(
            "T.toml",
            &named(SCADA).replace("[[\"DISPATCH\", \"UNIT_SCADA\"]]", "[]"),
        );
        assert!(e.contains("at least one report pair"), "{e}");
        let e = Catalog::read(&dir.path().join("none")).unwrap_err();
        assert!(matches!(e, CatalogError::Read { .. }), "{e}");
        let unreadable = dir.path().join("bad");
        std::fs::create_dir_all(&unreadable).unwrap();
        std::fs::write(unreadable.join("A.toml"), [0xff, 0xfe]).unwrap();
        let e = Catalog::read(&unreadable).unwrap_err().to_string();
        assert!(e.contains("A.toml cannot be read"), "{e}");
        let e = Catalog::new(vec![
            ("a.toml".into(), decl("A", ("P", "Q"), None)),
            ("b.toml".into(), decl("B", ("P", "Q"), None)),
        ])
        .unwrap_err();
        assert_eq!(
            e.to_string(),
            "the report pair P/Q is declared by both a.toml and b.toml"
        );
        let blocked = dir.path().join("blocked");
        std::fs::write(&blocked, "a file").unwrap();
        let e = scada().write(&blocked).unwrap_err().to_string();
        assert!(
            e.contains("DISPATCH_UNIT_SCADA.toml could not be written"),
            "{e}"
        );
    }

    #[test]
    fn a_pair_of_two_kinds_goes_to_the_table_whose_columns_a_block_holds() {
        let region = decl("ROOFTOP_PV_ACTUAL", ("ROOFTOP", "ACTUAL"), None);
        let area = TableDecl {
            with_columns: vec!["AREAID".into()],
            ..decl("ROOFTOP_PV_ACTUAL_AREA", ("ROOFTOP", "ACTUAL"), None)
        };
        let c = Catalog::new(vec![
            ("ROOFTOP_PV_ACTUAL.toml".into(), region.clone()),
            ("ROOFTOP_PV_ACTUAL_AREA.toml".into(), area.clone()),
        ])
        .unwrap();
        let regional = ["INTERVAL_DATETIME", "REGIONID", "POWER"];
        let areas = ["INTERVAL_DATETIME", "areaid", "POWER"];
        assert_eq!(
            c.table_of("ROOFTOP", "ACTUAL", &regional),
            "ROOFTOP_PV_ACTUAL"
        );
        assert_eq!(
            c.table_of("ROOFTOP", "ACTUAL", &areas),
            "ROOFTOP_PV_ACTUAL_AREA"
        );
        assert!(area.to_toml().contains("with_columns = [\"AREAID\"]"));
        // Two tables a block could equally be is a catalog error.
        let other = decl("ROOFTOP_OTHER", ("ROOFTOP", "ACTUAL"), None);
        let e = Catalog::new(vec![
            ("ROOFTOP_PV_ACTUAL.toml".into(), region),
            ("ROOFTOP_OTHER.toml".into(), other),
        ])
        .unwrap_err();
        assert!(matches!(e, CatalogError::Duplicate { .. }), "{e}");
    }

    #[test]
    fn a_rename_or_distinct_columns_without_their_reason_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let bad = |extra: &str| {
            let text = format!(
                "{}{extra}",
                decl("FPP_USAGE", ("FPP", "USAGE"), None).to_toml()
            );
            std::fs::write(dir.path().join("FPP_USAGE.toml"), text).unwrap();
            Catalog::read(dir.path()).unwrap_err().to_string()
        };
        let e = bad("\n[[distinct]]\ncolumns = [\"USAGE\"]\nreason = \"x\"\n");
        assert!(e.contains("two or more columns and a reason"), "{e}");
        let e = bad("\n[[rename]]\nfrom = \"USAGE\"\nto = \"USAGE\"\nevidence = \"x\"\n");
        assert!(e.contains("two different names and its evidence"), "{e}");
        let e = bad("\n[[rename]]\nfrom = \"USAGE\"\nto = \"USAGE_VALUE\"\nevidence = \" \"\n");
        assert!(e.contains("its evidence"), "{e}");
    }
}
