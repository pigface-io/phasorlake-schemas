//! Each table's history as an Iceberg table, folded from every census
//! record: its schemas, and the runs of AEMO files written under them.
//!
//! Every block the census recorded is one observation. A table's
//! observations are taken in the order AEMO released them: the month of
//! the release (the MMSDM archive month a file belongs to, a report CSV's
//! month), then where the block's data ends (its last time value, as an
//! instant), then the file's name, then the block's place in it. This is
//! the order an Iceberg table commits them in. It is also the only order
//! that holds the history still when files restate the past: each month's
//! `FPP_USAGE` file holds every interval since 2024-12-09, and the
//! `FPP_CONTRIBUTION_FACTOR` files of 2026-05 on restate 2025 in a new
//! layout; by data alone those interleave with the files before them. A
//! file's publication time cannot order them either: it is when the server
//! last touched the file, and AEMO touched most of the archive again in
//! 2026-04. Each run keeps the exact span of its data. Each observation is
//! given the schema its columns need:
//!
//! - **One field per lineage.** A column's lineage is its observations
//!   until its values turn from a number or a date to text (or between the
//!   two). The lineage is one field: its id is never minted twice, also
//!   across files that leave the column out, so a table that alternates
//!   between two column lists keeps two schemas, not a new one per file. A
//!   new lineage is a new field, recorded as breaking with its file and
//!   time.
//! - **One type per field.** A field's type is [`choose`] over everything
//!   its lineage holds and the widest that the control files and data-model
//!   releases of its months declare. Choosing it from the values seen so far
//!   instead would re-type a column whose first months were all null, and
//!   would turn more digits after the point into a new field (Iceberg
//!   cannot widen a scale); nulls fit any type, and a column's values are
//!   all known when the census is folded.
//! - **A key column is required** until a file holds a row without it, and
//!   optional from then on.
//! - **Schemas are interned:** one with the same columns, order and key as
//!   an earlier one is that one, with its id.
//!
//! Consecutive observations with the same schema, report version and
//! report pair fold into a run, with the kinds of file it came in (the
//! same data published as archive and DVD files is one run, not one per
//! file). A run says what changed at its start (`schema`, `version`,
//! `report`), the Iceberg updates from the run before it, and whether its
//! data overlaps the run before. Nothing in a run is merged away: its
//! first and last file, files and rows are kept, so every change has the
//! file and the time it happened at.
//!
//! Two things set a type beyond the rule:
//!
//! - **A published history is kept.** Every field of the history last
//!   published keeps its id, name and type, every schema its id, and every
//!   file its schema. Each published run covers the observations of its
//!   report pair and version from its first file to its last, and a
//!   column's lineage over a covered observation is the published field
//!   that covers it (`Column::pin`): the field is forced, never minted
//!   again, however much evidence from before or between the published
//!   runs is added later. A lineage from before a published field that the
//!   field's type holds (and, if the field is required, that holds no
//!   null) is that field; one it cannot hold is a new field, which breaks
//!   into the published one where it begins. At the first block a
//!   published type cannot hold ([`holds`]), the column gets a new field,
//!   recorded as breaking with that block's file and time: a split, never
//!   a retype. New fields and schemas take the ids after the published
//!   ones. A fold that cannot keep the promise
//!   ([`utilake_iceberg::compat::check`]) is an error.
//!
//!   Three cases are not kept, and are stated here: the evidence of a file
//!   already published is taken as unchanged (a re-census that changes it
//!   is a new field, named by the check only when the file began a run);
//!   a key column null in a backfill before a published required field is
//!   its own, optional, field; and two blocks of one file with the same
//!   report pair and version are covered from the first to the last,
//!   unless each marks a run of its own, told apart by its data time.
//! - **An override** (`[[override]]` in the table's declaration) is the
//!   type of each of the column's lineages that no published field sets,
//!   and the column's lineage does not end where its values turn to text.
//!   An override that cannot hold a block's values is an error.
//!
//! With no published history and no override, the fold is utilake's.

use std::collections::BTreeMap;

use utilake_iceberg::Market;
use utilake_iceberg::compat::{Violation, check};
use utilake_iceberg::history::{Break, Mark, Run, derive_run};
use utilake_iceberg::{
    Declared, Evidence, Field, History, IcebergType, Kind, Schema, choose, holds,
};

use crate::census::{BlockRecord, FileRecord};
use crate::month::YearMonth;
use crate::shape::TypeClass;

/// What a data-model release declares of a table's numeric columns:
/// precision and scale by column name.
pub type Numeric = BTreeMap<String, (u8, u8)>;

/// What is declared beyond the control files: each table's key and
/// overrides, and the numeric columns of every data-model release by the
/// month it appeared.
#[derive(Debug, Clone, Default)]
pub struct Declarations {
    /// Key columns by table.
    pub keys: BTreeMap<String, Vec<String>>,
    /// Columns typed by hand, by table.
    pub overrides: BTreeMap<String, BTreeMap<String, IcebergType>>,
    /// Renamed columns by table: each new name to the name it had.
    pub renames: BTreeMap<String, BTreeMap<String, String>>,
    /// Releases in the order they appeared: their first month and their
    /// tables' numeric columns.
    pub releases: Vec<(YearMonth, BTreeMap<String, Numeric>)>,
}

impl Declarations {
    /// The numeric columns of `table` in the newest release out by `month`
    /// that declares it, else the first release that does.
    pub(crate) fn numeric(&self, table: &str, month: YearMonth) -> Option<&Numeric> {
        let declaring = || {
            self.releases
                .iter()
                .filter_map(|(m, t)| Some((*m, t.get(table)?)))
        };
        declaring()
            .rfind(|(m, _)| *m <= month)
            .or_else(|| declaring().next())
            .map(|(_, n)| n)
    }
}

/// Why a table's history cannot be folded.
#[derive(Debug, thiserror::Error)]
pub enum HistoryError {
    /// An override whose type cannot hold a block's values.
    #[error(
        "The override of {column} in {table} as {ty} cannot hold the values of {file}; tables/{table}.toml needs another type."
    )]
    Override {
        /// The table.
        table: String,
        /// The column.
        column: String,
        /// The type it is forced to.
        ty: IcebergType,
        /// The first file whose values it cannot hold.
        file: String,
    },
    /// A block the lake would read with another schema than its run's.
    #[error(
        "In {table}, {file} is under schema {schema}, but its report pair, version, columns and month resolve to {}, with other fields; the lake would read it with the wrong schema.",
        resolved.map_or_else(|| "no schema".to_owned(), |s| format!("schema {s}"))
    )]
    Unresolved {
        /// The table.
        table: String,
        /// The block's file.
        file: String,
        /// The schema the fold gave it.
        schema: u32,
        /// The schema it resolves to.
        resolved: Option<u32>,
    },
    /// A rename whose two names one block holds: they are two columns.
    #[error(
        "{table} renames {from} to {to}, but {file} holds both: they are two columns, not one renamed."
    )]
    RenameOverlap {
        /// The table.
        table: String,
        /// The name it had.
        from: String,
        /// The name it has.
        to: String,
        /// A file holding both.
        file: String,
    },
    /// A fold that breaks what the published history promised.
    #[error(
        "The history of {table} breaks what nem/{table}/table.json published: {}.",
        violations.iter().map(ToString::to_string).collect::<Vec<_>>().join("; ")
    )]
    Broken {
        /// The table.
        table: String,
        /// Every broken promise.
        violations: Vec<Violation>,
    },
}

/// Fails at the first observation that [`History::resolve`] gives a schema
/// with other fields than the one the fold gave it (`schemas`, one per
/// observation): the lake reads a block with the schema it resolves to. A
/// schema that differs only in which fields are required (a key column
/// that took a null later in the month) reads the block the same.
fn unresolved(h: &History, obs: &[Obs], schemas: &[u32]) -> Result<(), HistoryError> {
    let ids = |s: Option<u32>| {
        s.and_then(|s| h.schema(s))
            .map(|s| s.fields.iter().map(|f| f.id).collect::<Vec<_>>())
    };
    for (o, &schema) in obs.iter().zip(schemas) {
        let columns: Vec<&str> = o.columns.iter().map(String::as_str).collect();
        let report = [o.report[0].as_str(), o.report[1].as_str()];
        let resolved = h.resolve(report, o.version, &columns, &o.month.to_string());
        if ids(resolved) != ids(Some(schema)) {
            return Err(HistoryError::Unresolved {
                table: h.table.clone(),
                file: o.from.file.clone(),
                schema,
                resolved,
            });
        }
    }
    Ok(())
}

/// One observation: a block, or a run of report files of one shape.
#[derive(Debug, Clone)]
struct Obs {
    /// Release month, data end in seconds, file, block.
    key: (YearMonth, i64, String, u32),
    month: YearMonth,
    report: [String; 2],
    version: u16,
    kind: String,
    from: Mark,
    to: Mark,
    files: u64,
    rows: u64,
    columns: Vec<String>,
    /// The headers the columns were written under, where the record keeps
    /// them; empty otherwise.
    headers: Vec<String>,
    ctl: Vec<TypeClass>,
    evidence: Vec<Evidence>,
    nulls: Vec<u64>,
    /// Whether every value of the span column was at midnight; none when
    /// the block has no span or was recorded before that was.
    midnight: Option<bool>,
}

/// Every observation of every table, gathering, in one market's clock.
#[derive(Debug, Default)]
pub struct Observations {
    market: Market,
    tables: BTreeMap<String, Vec<Obs>>,
}

impl Observations {
    /// Observations of `market`, whose times are read in its clock.
    pub fn for_market(market: Market) -> Self {
        Self {
            market,
            tables: BTreeMap::new(),
        }
    }

    /// Takes in block `index` of `file` of the census of `month`.
    pub fn block(
        &mut self,
        table: &str,
        month: YearMonth,
        (file, index): (&str, u32),
        f: &FileRecord,
        b: &BlockRecord,
    ) {
        let file = (file, f.sha256.as_str());
        self.take(table, month, (file, file, 1, index), f, b);
    }

    /// Takes in a run of `files` report files of one shape, from the file
    /// named `first` to the one named `last`, each with its SHA-256.
    pub fn run(
        &mut self,
        table: &str,
        month: YearMonth,
        (first, last, files): ((&str, &str), (&str, &str), u64),
        f: &FileRecord,
        b: &BlockRecord,
    ) {
        self.take(table, month, (first, last, files, 0), f, b);
    }

    fn take(
        &mut self,
        table: &str,
        month: YearMonth,
        (first, last, files, index): ((&str, &str), (&str, &str), u64, u32),
        f: &FileRecord,
        b: &BlockRecord,
    ) {
        let offset = self.market.offset_text();
        let start = b
            .first
            .as_deref()
            .and_then(|t| crate::fold::market_time(t, offset));
        let end = b
            .last
            .as_deref()
            .and_then(|t| crate::fold::market_time(t, offset));
        let mark = |(file, sha256): (&str, &str), data_time: Option<String>| Mark {
            file: file.to_owned(),
            sha256: sha256.to_owned(),
            release: month.to_string(),
            data_time,
        };
        // Blocks without a time sort at the start of their month.
        let ends_at = instant(end.as_deref())
            .or_else(|| instant(Some(&format!("{month}-01T00:00:00{offset}"))))
            .unwrap_or_default();
        let n = b.columns.len();
        let profiles: Vec<_> = (0..n).map(|j| b.profile_of(j)).collect();
        self.tables.entry(table.to_owned()).or_default().push(Obs {
            key: (month, ends_at, first.0.to_owned(), index),
            month,
            report: [b.report.0.clone(), b.report.1.clone()],
            version: b.version,
            kind: f.kind.clone(),
            from: mark(first, start),
            to: mark(last, end),
            files,
            rows: b.rows,
            columns: b.columns.clone(),
            headers: b.headers.clone(),
            ctl: (0..n).map(|j| b.ctl_of(j)).collect(),
            evidence: profiles
                .iter()
                .map(crate::profile::ColumnProfile::evidence)
                .collect(),
            nulls: profiles.iter().map(|p| p.nulls).collect(),
            midnight: b.midnight,
        });
    }

    /// Whether every recorded value of `table`'s span column was at
    /// midnight: `Some(true)` when every block that says is a day label,
    /// `Some(false)` when any block saw a time of day, `None` when no block
    /// of the table says (none has a span, or all were recorded before the
    /// census kept this).
    pub fn midnight_only(&self, table: &str) -> Option<bool> {
        let seen: Vec<bool> = self
            .tables
            .get(table)
            .into_iter()
            .flatten()
            .filter_map(|o| o.midnight)
            .collect();
        (!seen.is_empty()).then(|| seen.iter().all(|m| *m))
    }

    /// Every table's history: of every table observed, and of every table
    /// `published` holds, each kept to what was published.
    pub fn finish(
        self,
        d: &Declarations,
        published: &BTreeMap<String, History>,
    ) -> Result<BTreeMap<String, History>, HistoryError> {
        let mut tables = self.tables;
        for t in published.keys() {
            tables.entry(t.clone()).or_default();
        }
        let none = BTreeMap::new();
        let mut out = BTreeMap::new();
        for (table, mut obs) in tables {
            obs.sort_by(|a, b| a.key.cmp(&b.key));
            let no_renames = BTreeMap::new();
            let rules = Rules {
                key: d.keys.get(&table).map_or(&[][..], Vec::as_slice),
                overrides: d.overrides.get(&table).unwrap_or(&none),
                renames: d.renames.get(&table).unwrap_or(&no_renames),
                published: published.get(&table),
            };
            let (mut h, schemas) = fold_table(&table, &rules, d, &obs)?;
            unresolved(&h, &obs, &schemas)?;
            // The header a column was written under, kept by the first
            // observation that names one for its field.
            let mut headers = BTreeMap::new();
            for (o, &id) in obs.iter().zip(&schemas) {
                // Every observation was folded under a schema the history
                // has, with a field per column.
                let s = h.schema(id).expect("the observation's schema");
                for (name, header) in o.columns.iter().zip(&o.headers) {
                    if header.is_empty() || header == name {
                        continue;
                    }
                    let f = s.by_name(name).expect("the column's field");
                    headers.entry(f.id).or_insert_with(|| header.clone());
                }
            }
            h.headers = headers;
            if let Some(old) = rules.published {
                let violations = check(old, &h);
                if !violations.is_empty() {
                    return Err(HistoryError::Broken { table, violations });
                }
            }
            out.insert(table, h);
        }
        Ok(out)
    }
}

/// An RFC 3339 time (`YYYY-MM-DDTHH:MM:SS[.f]` and `Z` or `±HH:MM`) as
/// whole seconds since 1970.
fn instant(text: Option<&str>) -> Option<i64> {
    let t = text?;
    let n = |at: usize, len: usize| -> Option<i64> {
        let s = t.get(at..at + len)?;
        s.bytes()
            .all(|c| c.is_ascii_digit())
            .then(|| s.parse().ok())?
    };
    let at = |i: usize, c: u8| t.as_bytes().get(i) == Some(&c);
    if !(at(4, b'-') && at(7, b'-') && at(10, b'T') && at(13, b':') && at(16, b':')) {
        return None;
    }
    let (y, mo, d) = (n(0, 4)?, n(5, 2)?, n(8, 2)?);
    let (h, mi, s) = (n(11, 2)?, n(14, 2)?, n(17, 2)?);
    let mut rest = &t[19..];
    if let Some(f) = rest.strip_prefix('.') {
        let digits = f.bytes().take_while(u8::is_ascii_digit).count();
        rest = &f[digits..];
        if digits == 0 {
            return None;
        }
    }
    let offset = match rest.as_bytes().first() {
        Some(b'Z') if rest.len() == 1 => 0,
        Some(&sign @ (b'+' | b'-')) if rest.len() == 6 && rest.as_bytes()[3] == b':' => {
            let (oh, om) = (n(t.len() - 5, 2)?, n(t.len() - 2, 2)?);
            let o = if oh < 24 && om < 60 {
                oh * 3600 + om * 60
            } else {
                return None;
            };
            if sign == b'-' { -o } else { o }
        }
        _ => return None,
    };
    let leap = y % 4 == 0 && (y % 100 != 0 || y % 400 == 0);
    let days_in = [
        31,
        if leap { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    let month_days = usize::try_from(mo - 1).ok().and_then(|i| days_in.get(i));
    if !month_days.is_some_and(|&m| (1..=m).contains(&d)) || h > 23 || mi > 59 || s > 59 {
        return None;
    }
    // Days from 1970-01-01 to the date, counted in 400-year eras from
    // March 1st.
    let ya = if mo <= 2 { y - 1 } else { y };
    let era = ya.div_euclid(400);
    let yoe = ya - era * 400;
    let doy = (153 * ((mo + 9) % 12) + 2) / 5 + d - 1;
    let days = era * 146_097 + yoe * 365 + yoe / 4 - yoe / 100 + doy - 719_468;
    Some(days * 86_400 + h * 3600 + mi * 60 + s - offset)
}

fn declared(ctl: TypeClass, numeric: Option<(u8, u8)>, key: bool) -> Declared {
    let (precision, scale) = numeric.map_or((None, None), |(p, s)| (Some(p), Some(s)));
    Declared {
        whole: scale == Some(0) || ctl == TypeClass::Integer,
        fractional: scale.is_some_and(|s| s > 0) || ctl == TypeClass::Number,
        precision,
        scale,
        fallback: fallback(precision, scale),
        key,
    }
}

fn fallback(precision: Option<u8>, scale: Option<u8>) -> Option<IcebergType> {
    match (precision, scale) {
        (Some(_), Some(0)) => Some(IcebergType::Long),
        (Some(p), Some(s)) if p <= 18 => IcebergType::decimal(p, s),
        _ => None,
    }
}

/// The widest of two declarations: whole only if never declared with a
/// fraction, the larger precision and scale.
fn widest(a: Declared, b: Declared) -> Declared {
    let precision = a.precision.max(b.precision);
    let scale = a.scale.max(b.scale);
    let fractional = a.fractional || b.fractional;
    Declared {
        whole: (a.whole || b.whole) && !fractional,
        fractional,
        precision,
        scale,
        fallback: fallback(precision, scale).or(b.fallback).or(a.fallback),
        key: a.key || b.key,
    }
}

/// Whether values of kind `new` end a lineage of kind `old`: a number or a
/// date (or the other) that turns to text.
fn ends(old: Kind, new: Kind) -> bool {
    old != Kind::Empty && new != Kind::Empty && old != Kind::Text && old.join(new) == Kind::Text
}

/// What sets a table's types beyond the rule.
struct Rules<'a> {
    key: &'a [String],
    overrides: &'a BTreeMap<String, IcebergType>,
    /// Each new name of a renamed column to the name it had.
    renames: &'a BTreeMap<String, String>,
    published: Option<&'a History>,
}

impl Rules<'_> {
    /// The name a column's lineage goes by: its first, down its renames.
    fn lineage<'n>(&'n self, name: &'n str) -> &'n str {
        let mut n = name;
        for _ in 0..=self.renames.len() {
            match self.renames.get(n) {
                Some(from) => n = from,
                None => break,
            }
        }
        n
    }
}

/// What a published run's schema gives a column: its field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Want {
    id: u32,
    ty: IcebergType,
    required: bool,
}

/// One column in one observation.
#[derive(Debug, Clone, Copy)]
struct Occ {
    /// The observation.
    obs: usize,
    evidence: Evidence,
    declared: Declared,
    nulls: u64,
    /// The published field the observation is covered by, if any.
    want: Option<Want>,
}

/// A column over every observation, folding.
#[derive(Debug, Default)]
struct Column {
    /// Every observation of it, in order.
    occ: Vec<Occ>,
    /// Where in `occ` its values turned to text: each starts a lineage.
    turns: Vec<usize>,
    /// The kind of every value since the last turn.
    kind: Kind,
    /// The next of `occ` to fold.
    next: usize,
    /// Its current lineage.
    field: Option<Lineage>,
}

/// A column's lineage, folding: its field.
#[derive(Debug, Clone, Copy)]
struct Lineage {
    id: u32,
    ty: IcebergType,
    /// Whether the field is a published one.
    pinned: bool,
    /// Whether the published field is required.
    required: bool,
    /// Where in `occ` the lineage ends.
    end: usize,
}

impl Column {
    /// The joined evidence and widest declaration of `occ[from..to]`.
    fn over(&self, from: usize, to: usize) -> (Evidence, Declared) {
        let first = self.occ[from];
        self.occ[from + 1..to]
            .iter()
            .fold((first.evidence, first.declared), |(e, d), o| {
                (e.join(o.evidence), widest(d, o.declared))
            })
    }

    /// The published field a lineage starting at `occ[p]` and ending at
    /// `occ[end]` continues: the one `occ[p]` is covered by, when its type
    /// holds the values; else the first one a later observation of the
    /// lineage is covered by, when its type holds everything up to it and,
    /// if it is required, nothing before it holds a null. None: a new
    /// field.
    fn pin(&self, p: usize, end: usize) -> Option<Want> {
        if let Some(w) = self.occ[p].want {
            return holds(w.ty, self.occ[p].evidence).then_some(w);
        }
        let (q, w) = (p + 1..end).find_map(|q| Some((q, self.occ[q].want?)))?;
        let (e, _) = self.over(p, q);
        let nulled = w.required && self.occ[p..q].iter().any(|o| o.nulls > 0);
        (holds(w.ty, e) && !nulled).then_some(w)
    }
}

/// For each observation, the published schema whose run covers it, and
/// whether it is the first of a published run. A run covers the
/// observations of its report pair and version from the one its first
/// file begins to the one its last file ends (the first alone when the
/// last is not found, or comes first). A run whose first file no
/// observation begins covers nothing: [`check`] names it.
fn coverage<'a>(published: Option<&'a History>, obs: &[Obs]) -> Vec<(Option<&'a Schema>, bool)> {
    let mut out = vec![(None, false); obs.len()];
    let Some(h) = published else {
        return out;
    };
    for r in &h.runs {
        let is = |o: &Obs, mark: &Mark, m: &Mark| {
            o.report == r.report
                && o.version == r.version
                && mark.file == m.file
                && mark.sha256 == m.sha256
        };
        // One file may hold blocks of more than one schema (a report
        // folder's zip of a month whose shape changed part way), each an
        // observation of its own that ends one run and begins the next:
        // the run's mark is the observation at its data time, else the
        // first (or last) at the file.
        let at = |o: &Obs, mark: &Mark, m: &Mark| is(o, mark, m) && mark.data_time == m.data_time;
        let Some(from) = obs
            .iter()
            .position(|o| at(o, &o.from, &r.from))
            .or_else(|| obs.iter().position(|o| is(o, &o.from, &r.from)))
        else {
            continue;
        };
        let to = obs
            .iter()
            .rposition(|o| at(o, &o.to, &r.to))
            .or_else(|| obs.iter().rposition(|o| is(o, &o.to, &r.to)))
            .filter(|&t| t >= from)
            .unwrap_or(from);
        let schema = h.schema(r.schema_id);
        for (i, o) in obs.iter().enumerate().take(to + 1).skip(from) {
            if o.report == r.report && o.version == r.version {
                out[i].0 = schema;
            }
        }
        out[from].1 = true;
    }
    out
}

fn fold_table(
    table: &str,
    rules: &Rules<'_>,
    d: &Declarations,
    obs: &[Obs],
) -> Result<(History, Vec<u32>), HistoryError> {
    let key = rules.key;
    let covered = coverage(rules.published, obs);
    // Each column's observations, and where its values turn to text: a
    // renamed column's under the name it had, so both names are one field.
    let mut columns: BTreeMap<&str, Column> = BTreeMap::new();
    for o in obs {
        for (to, from) in rules.renames {
            if o.columns.contains(to) && o.columns.contains(from) {
                return Err(HistoryError::RenameOverlap {
                    table: table.to_owned(),
                    from: from.clone(),
                    to: to.clone(),
                    file: o.from.file.clone(),
                });
            }
        }
    }
    for (i, o) in obs.iter().enumerate() {
        let numeric = d.numeric(table, o.month);
        for (j, name) in o.columns.iter().enumerate() {
            let declared = declared(
                o.ctl[j],
                numeric.and_then(|n| n.get(name)).copied(),
                key.contains(name),
            );
            let c = columns.entry(rules.lineage(name)).or_default();
            let seen = o.evidence[j];
            if ends(c.kind, seen.kind) && !rules.overrides.contains_key(name) {
                c.turns.push(c.occ.len());
                c.kind = seen.kind;
            } else {
                c.kind = c.kind.join(seen.kind);
            }
            c.occ.push(Occ {
                obs: i,
                evidence: seen,
                declared,
                nulls: o.nulls[j],
                want: covered[i].0.and_then(|s| s.by_name(name)).map(|f| Want {
                    id: f.id,
                    ty: f.ty,
                    required: f.required,
                }),
            });
        }
    }
    // The published fields and schemas keep their ids; new ones take the
    // ids after them.
    let published = rules.published;
    let mut next_id = published
        .map(History::fields)
        .unwrap_or_default()
        .iter()
        .map(|f| f.id)
        .max()
        .unwrap_or(0);
    let mut required: BTreeMap<u32, bool> = BTreeMap::new();
    let mut schemas = Vec::with_capacity(obs.len());
    let mut out = History {
        table: table.to_owned(),
        format_version: 3,
        current_schema_id: 0,
        schemas: published.map(|h| h.schemas.clone()).unwrap_or_default(),
        runs: Vec::new(),
        breaking: Vec::new(),
        headers: BTreeMap::new(),
    };
    let mut next_schema_id = out
        .schemas
        .iter()
        .map(|s| s.schema_id)
        .max()
        .map_or(0, |m| m + 1);
    for (i, o) in obs.iter().enumerate() {
        let mut fields = Vec::with_capacity(o.columns.len());
        for (j, name) in o.columns.iter().enumerate() {
            let c = columns
                .get_mut(rules.lineage(name))
                .expect("every column was gathered");
            let p = c.next;
            c.next += 1;
            let seen = c.occ[p].evidence;
            let want = c.occ[p].want;
            // A lineage starts at the column's first observation, where
            // its values turn to text, where a published type no longer
            // holds them, where a published required field meets a null
            // outside its runs, and where another published field covers
            // them.
            let starts = match c.field {
                None => true,
                Some(l) => {
                    p == l.end
                        || (l.pinned && !holds(l.ty, seen))
                        || (l.pinned && l.required && want.is_none() && c.occ[p].nulls > 0)
                        || want.is_some_and(|w| w.id != l.id && holds(w.ty, seen))
                }
            };
            if starts {
                let end = c
                    .turns
                    .iter()
                    .copied()
                    .find(|&t| t > p)
                    .unwrap_or(c.occ.len());
                let next = match c.pin(p, end) {
                    Some(w) => Lineage {
                        id: w.id,
                        ty: w.ty,
                        pinned: true,
                        required: w.required,
                        end,
                    },
                    None => {
                        next_id += 1;
                        Lineage {
                            id: next_id,
                            ty: lineage_type(table, name, rules, c, obs, (p, end))?,
                            pinned: false,
                            required: false,
                            end,
                        }
                    }
                };
                if let Some(old) = c.field {
                    out.breaking.push(Break {
                        column: name.clone(),
                        old_field_id: old.id,
                        old_type: old.ty.name(),
                        new_field_id: next.id,
                        new_type: next.ty.name(),
                        at: o.from.clone(),
                    });
                }
                c.field = Some(next);
            }
            let Lineage { id, ty, .. } = c.field.expect("a lineage was started");
            let req = required.entry(id).or_insert_with(|| key.contains(name));
            *req &= o.nulls[j] == 0;
            fields.push(Field {
                id,
                name: name.clone(),
                ty,
                required: *req,
                doc: None,
            });
        }
        // Iceberg's identifier fields are required; a key column that took
        // a null leaves the key until the schema says otherwise.
        let identifier_field_ids = key
            .iter()
            .filter_map(|c| fields.iter().find(|f| &f.name == c && f.required))
            .map(|f| f.id)
            .collect();
        let schema = Schema {
            schema_id: 0,
            last_column_id: fields.iter().map(|f| f.id).max().unwrap_or(0),
            fields,
            identifier_field_ids,
        };
        let id = match out.schemas.iter().find(|s| s.same_as(&schema)) {
            Some(s) => s.schema_id,
            None => {
                let id = next_schema_id;
                next_schema_id += 1;
                out.schemas.push(Schema {
                    schema_id: id,
                    ..schema
                });
                id
            }
        };
        extend(&mut out, id, o, covered[i].1);
        schemas.push(id);
    }
    out.current_schema_id = out.runs.last().map_or(0, |r| r.schema_id);
    Ok((out, schemas))
}

/// The type of a new lineage of column `name`, `occ[from..to]` of `c`: its
/// override, which must hold every block of it, else the rule's.
fn lineage_type(
    table: &str,
    name: &str,
    rules: &Rules<'_>,
    c: &Column,
    obs: &[Obs],
    (from, to): (usize, usize),
) -> Result<IcebergType, HistoryError> {
    let Some(&ty) = rules.overrides.get(name) else {
        let (e, d) = c.over(from, to);
        return Ok(choose(e, d).ty);
    };
    match c.occ[from..to].iter().find(|o| !holds(ty, o.evidence)) {
        None => Ok(ty),
        Some(o) => Err(HistoryError::Override {
            table: table.to_owned(),
            column: name.to_owned(),
            ty,
            file: obs[o.obs].from.file.clone(),
        }),
    }
}

/// Adds `o`, under schema `id`, to the last run or as a new one; `published`
/// when `o` began a published run, which is then kept whole.
fn extend(out: &mut History, id: u32, o: &Obs, published: bool) {
    let same = |r: &Run| r.schema_id == id && r.version == o.version && r.report == o.report;
    if !published
        && let Some(r) = out.runs.last_mut()
        && same(r)
    {
        if let Err(at) = r.kinds.binary_search(&o.kind) {
            r.kinds.insert(at, o.kind.clone());
        }
        // The run ends where its latest data ends (no time sorts first).
        if o.to.data_time >= r.to.data_time {
            r.to = o.to.clone();
        }
        r.files += o.files;
        r.rows += o.rows;
        return;
    }
    out.runs.push(Run {
        schema_id: id,
        report: o.report.clone(),
        version: o.version,
        kinds: vec![o.kind.clone()],
        from: o.from.clone(),
        to: o.to.clone(),
        files: o.files,
        rows: o.rows,
        changes: Vec::new(),
        updates: Vec::new(),
        overlaps: false,
    });
    let n = out.runs.len() - 1;
    let (before, last) = out.runs.split_at_mut(n);
    derive_run(&out.schemas, before, &mut last[0]).expect("every run's schema is kept");
}

#[cfg(test)]
mod tests;
