//! Reading any interval of a table across its schemas.
//!
//! A table's files were written under different schemas. A read of a time
//! interval takes the files whose data overlaps it and projects each onto
//! one target schema by field id, as an Iceberg reader does: a field the
//! file's schema has is read under the name it has there and cast when the
//! target's type is a promotion of it; a field it lacks is null of the
//! target's type; a field the target lacks is left out. The target is the
//! table's current schema, or the schema of a point in its history, so a
//! loader written against that schema reads every file the same way.
//!
//! One rule goes beyond an Iceberg reader. A column whose values left its
//! field's type got a new field ([`History::breaking`]); Iceberg reads the
//! old files as null for the new field. Here, a field the file lacks is
//! looked for down the column's breaks, and the old field is read as the
//! new one when every value of the old type is a value of the new
//! ([`casts_to`]): a wider decimal, a double. A column that turned to text
//! is not cast: the text is not the number. This is a read rule of this
//! crate, not a schema promotion; the table metadata never claims it.
//!
//! [`duckdb_sql`] writes the plan as one DuckDB query over the Parquet
//! files: one `SELECT` per group of files with the same projection, joined
//! by `UNION ALL`, each bounded to the interval on the time column.

use crate::history::{Break, History};
use crate::schema::{Field, Schema};
use crate::types::IcebergType;

/// A data file of the table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataFile {
    /// Where it is.
    pub path: String,
    /// The schema it was written under.
    pub schema_id: u32,
    /// Its least time value, microseconds since 1970 (UTC).
    pub min_us: Option<i64>,
    /// Its greatest.
    pub max_us: Option<i64>,
}

/// The schema a read projects onto.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    /// The table's current schema.
    Latest,
    /// The schema with this id.
    AsOf(u32),
}

/// One column of a projection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Column {
    /// Read from the file.
    Read {
        /// Its name in the file.
        source: String,
        /// Its name in the target.
        name: String,
        /// The target's type, when it is a promotion of the file's.
        cast: Option<IcebergType>,
    },
    /// Not in the file: null of the target's type.
    Null {
        /// Its name in the target.
        name: String,
        /// The target's type.
        ty: IcebergType,
    },
}

/// Files read with one projection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Read {
    /// The files.
    pub paths: Vec<String>,
    /// The target's columns, in its order.
    pub columns: Vec<Column>,
}

/// Why a read cannot be planned.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PlanError {
    /// The target or a file names a schema the table lacks.
    #[error("The table has no schema {0}.")]
    UnknownSchema(u32),
    /// A field's type in the target is not one Iceberg reaches from the file's.
    #[error(
        "Column {name} is {from} in schema {schema} and {to} in the target; Iceberg does not promote one to the other."
    )]
    NotAPromotion {
        /// The column.
        name: String,
        /// The file's schema.
        schema: u32,
        /// Its type there.
        from: IcebergType,
        /// The target's.
        to: IcebergType,
    },
}

fn schema(h: &History, id: u32) -> Result<&Schema, PlanError> {
    h.schema(id).ok_or(PlanError::UnknownSchema(id))
}

/// Whether every value of a column of type `from` is read as one of `to`
/// without loss, the read rule across a breaking column: the same type, a
/// decimal of no fewer digits on either side of the point, a long as a
/// decimal of at least 18 whole digits (a long holds at most 18 here, as
/// [`crate::choose`] types it), and a long or decimal as a double. Text is
/// never cast either way, nor a date to a time.
pub fn casts_to(from: IcebergType, to: IcebergType) -> bool {
    use IcebergType::{Decimal, Double, Long};
    match (from, to) {
        (a, b) if a == b => true,
        (
            Decimal {
                precision: p,
                scale: s,
            },
            Decimal {
                precision: q,
                scale: t,
            },
        ) => t >= s && q - t >= p - s,
        (Long, Decimal { precision, scale }) => precision - scale >= 18,
        (Long | Decimal { .. }, Double) => true,
        _ => false,
    }
}

/// The field of `from` that target field `f` is read from down the breaks:
/// `f` itself, else the field the column broke from, as far back as it
/// takes to reach one `from` has.
fn source<'a>(from: &'a Schema, f: &Field, breaking: &[Break]) -> Option<&'a Field> {
    let mut id = f.id;
    for _ in 0..=breaking.len() {
        if let Some(g) = from.field(id) {
            return Some(g);
        }
        id = breaking.iter().find(|b| b.new_field_id == id)?.old_field_id;
    }
    None
}

/// The projection of a file of schema `from` onto `to`.
fn project(from: &Schema, to: &Schema, breaking: &[Break]) -> Result<Vec<Column>, PlanError> {
    to.fields
        .iter()
        .map(|f| match source(from, f, breaking) {
            None => Ok(Column::Null {
                name: f.name.clone(),
                ty: f.ty,
            }),
            Some(g) if g.id != f.id => Ok(if casts_to(g.ty, f.ty) {
                Column::Read {
                    source: g.name.clone(),
                    name: f.name.clone(),
                    cast: (g.ty != f.ty).then_some(f.ty),
                }
            } else {
                Column::Null {
                    name: f.name.clone(),
                    ty: f.ty,
                }
            }),
            Some(g) if g.ty == f.ty => Ok(Column::Read {
                source: g.name.clone(),
                name: f.name.clone(),
                cast: None,
            }),
            Some(g) if g.ty.promotes_to(f.ty) => Ok(Column::Read {
                source: g.name.clone(),
                name: f.name.clone(),
                cast: Some(f.ty),
            }),
            Some(g) => Err(PlanError::NotAPromotion {
                name: f.name.clone(),
                schema: from.schema_id,
                from: g.ty,
                to: f.ty,
            }),
        })
        .collect()
}

/// The reads of the files whose data overlaps `[t0, t1)` (microseconds),
/// projected onto `target`. A file with no time bounds is always read.
/// Files keep their order; consecutive files with the same projection share
/// a read.
pub fn plan(
    h: &History,
    files: &[DataFile],
    (t0, t1): (i64, i64),
    target: Target,
) -> Result<Vec<Read>, PlanError> {
    let to = match target {
        Target::AsOf(id) => schema(h, id)?,
        Target::Latest => schema(h, h.current_schema_id)?,
    };
    let mut out: Vec<Read> = Vec::new();
    for f in files {
        let overlaps = f.max_us.is_none_or(|m| m >= t0) && f.min_us.is_none_or(|m| m < t1);
        if !overlaps {
            continue;
        }
        let columns = project(schema(h, f.schema_id)?, to, &h.breaking)?;
        match out.last_mut() {
            Some(r) if r.columns == columns => r.paths.push(f.path.clone()),
            _ => out.push(Read {
                paths: vec![f.path.clone()],
                columns,
            }),
        }
    }
    Ok(out)
}

/// DuckDB's name for a type.
fn duck_type(t: IcebergType) -> String {
    match t {
        IcebergType::Boolean => "BOOLEAN".into(),
        IcebergType::Long => "BIGINT".into(),
        IcebergType::Double => "DOUBLE".into(),
        IcebergType::Decimal { precision, scale } => format!("DECIMAL({precision}, {scale})"),
        IcebergType::Date => "DATE".into(),
        IcebergType::Timestamptz => "TIMESTAMPTZ".into(),
        IcebergType::String => "VARCHAR".into(),
    }
}

fn ident(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

fn literal(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

/// An instant as DuckDB reads it: microseconds since 1970 in UTC.
fn instant(us: i64) -> String {
    format!("make_timestamptz({us})")
}

/// The reads as one DuckDB query; `time` is the column the interval bounds
/// (named as in the target), none for a table without one.
pub fn duckdb_sql(reads: &[Read], time: Option<&str>, (t0, t1): (i64, i64)) -> String {
    let selects: Vec<String> = reads
        .iter()
        .map(|r| {
            let cols: Vec<String> = r
                .columns
                .iter()
                .map(|c| match c {
                    Column::Read { source, name, cast } => {
                        let v = match cast {
                            Some(t) => format!("CAST({} AS {})", ident(source), duck_type(*t)),
                            None => ident(source),
                        };
                        format!("{v} AS {}", ident(name))
                    }
                    Column::Null { name, ty } => {
                        format!("CAST(NULL AS {}) AS {}", duck_type(*ty), ident(name))
                    }
                })
                .collect();
            let files: Vec<String> = r.paths.iter().map(|p| literal(p)).collect();
            let source_time = time.and_then(|t| {
                r.columns.iter().find_map(|c| match c {
                    Column::Read { source, name, .. } if name == t => Some(source.clone()),
                    _ => None,
                })
            });
            let filter = source_time.map_or_else(String::new, |s| {
                format!(
                    "\nWHERE {} >= {} AND {} < {}",
                    ident(&s),
                    instant(t0),
                    ident(&s),
                    instant(t1)
                )
            });
            format!(
                "SELECT {}\nFROM read_parquet([{}]){filter}",
                cols.join(", "),
                files.join(", ")
            )
        })
        .collect();
    selects.join("\nUNION ALL\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::Mark;

    fn field(id: u32, name: &str, ty: IcebergType) -> Field {
        Field {
            id,
            name: name.into(),
            ty,
            required: false,
            doc: None,
        }
    }

    fn dec(p: u8) -> IcebergType {
        IcebergType::decimal(p, 5).unwrap()
    }

    /// A: time, rrp decimal(15, 5); B: rrp widened, raise added before it;
    /// C: raise left out again; D: rrp broke into field 4, decimal(20, 6);
    /// E: field 4 broke into field 5, double; F: field 5 broke into field
    /// 6, text. The current schema is C.
    fn schemas() -> History {
        let t = field(1, "SETTLEMENTDATE", IcebergType::Timestamptz);
        let schema = |id, fields| Schema {
            schema_id: id,
            fields,
            identifier_field_ids: vec![1],
            last_column_id: 3,
        };
        let broke = |column: &str, old: (u32, IcebergType), new: (u32, IcebergType)| Break {
            column: column.into(),
            old_field_id: old.0,
            old_type: old.1.name(),
            new_field_id: new.0,
            new_type: new.1.name(),
            at: Mark {
                file: "X.zip".into(),
                sha256: "ab".into(),
                release: "2024-09".into(),
                data_time: None,
            },
        };
        let wide = IcebergType::decimal(20, 6).unwrap();
        History {
            table: "PRICE".into(),
            format_version: 3,
            current_schema_id: 2,
            headers: std::collections::BTreeMap::new(),
            schemas: vec![
                schema(0, vec![t.clone(), field(2, "RRP", dec(15))]),
                schema(
                    1,
                    vec![
                        t.clone(),
                        field(3, "RAISE", IcebergType::Double),
                        field(2, "RRP", dec(18)),
                    ],
                ),
                schema(2, vec![t.clone(), field(2, "RRP", dec(18))]),
                schema(3, vec![t.clone(), field(4, "RRP", wide)]),
                schema(4, vec![t.clone(), field(5, "RRP", IcebergType::Double)]),
                schema(5, vec![t, field(6, "RRP", IcebergType::String)]),
            ],
            runs: Vec::new(),
            breaking: vec![
                broke("RRP", (2, dec(18)), (4, wide)),
                broke("RRP", (4, wide), (5, IcebergType::Double)),
                broke("RRP", (5, IcebergType::Double), (6, IcebergType::String)),
            ],
        }
    }

    fn file(path: &str, schema_id: u32, min: i64, max: i64) -> DataFile {
        DataFile {
            path: path.into(),
            schema_id,
            min_us: Some(min),
            max_us: Some(max),
        }
    }

    #[test]
    fn every_file_reads_through_the_target() {
        let files = [
            file("a.parquet", 0, 0, 9),
            file("a2.parquet", 0, 10, 19),
            file("b.parquet", 1, 20, 29),
            file("c.parquet", 2, 30, 39),
            DataFile {
                path: "timeless.parquet".into(),
                schema_id: 2,
                min_us: None,
                max_us: None,
            },
        ];
        let reads = plan(&schemas(), &files, (5, 35), Target::AsOf(1)).unwrap();
        let paths: Vec<Vec<&str>> = reads
            .iter()
            .map(|r| r.paths.iter().map(String::as_str).collect())
            .collect();
        assert_eq!(
            paths,
            [
                vec!["a.parquet", "a2.parquet"],
                vec!["b.parquet"],
                vec!["c.parquet", "timeless.parquet"]
            ]
        );
        // A file of schema A: RAISE is null, RRP widened.
        assert_eq!(
            reads[0].columns,
            [
                Column::Read {
                    source: "SETTLEMENTDATE".into(),
                    name: "SETTLEMENTDATE".into(),
                    cast: None
                },
                Column::Null {
                    name: "RAISE".into(),
                    ty: IcebergType::Double
                },
                Column::Read {
                    source: "RRP".into(),
                    name: "RRP".into(),
                    cast: Some(dec(18))
                },
            ]
        );
        // Outside the interval, nothing; the current schema leaves RAISE out.
        assert!(
            plan(&schemas(), &files[..2], (100, 200), Target::Latest)
                .unwrap()
                .is_empty()
        );
        let latest = plan(&schemas(), &files[2..3], (0, 100), Target::Latest).unwrap();
        assert_eq!(latest[0].columns.len(), 2);
    }

    /// A file from before a break reads its old field as the new one when
    /// the old type casts to the new, down every break between; a column
    /// that turned to text reads null.
    #[test]
    fn a_broken_column_is_read_from_its_old_field_when_it_casts() {
        let h = schemas();
        let read = |target, file_schema| {
            let f = [file("a.parquet", file_schema, 0, 9)];
            plan(&h, &f, (0, 10), Target::AsOf(target)).unwrap()[0].columns[1].clone()
        };
        let rrp = |cast| Column::Read {
            source: "RRP".into(),
            name: "RRP".into(),
            cast,
        };
        // One break: decimal(15, 5) as decimal(20, 6).
        assert_eq!(read(3, 0), rrp(IcebergType::decimal(20, 6)));
        // Two breaks: through decimal(20, 6) to double.
        assert_eq!(read(4, 0), rrp(Some(IcebergType::Double)));
        assert_eq!(read(4, 3), rrp(Some(IcebergType::Double)));
        // Three: text is never cast.
        assert_eq!(
            read(5, 0),
            Column::Null {
                name: "RRP".into(),
                ty: IcebergType::String
            }
        );
        // Backwards, a break is no path: the new field is not the old.
        assert_eq!(
            read(0, 3),
            Column::Null {
                name: "RRP".into(),
                ty: dec(15)
            }
        );
        // The same type down a break reads without a cast.
        let mut same = h.clone();
        same.schemas[3].fields[1].ty = dec(18);
        let f = [file("a.parquet", 2, 0, 9)];
        assert_eq!(
            plan(&same, &f, (0, 10), Target::AsOf(3)).unwrap()[0].columns[1],
            rrp(None)
        );
        // A break whose old field no schema has reaches nothing; nor does
        // one that names itself, however often it is followed.
        let f = [file("a.parquet", 0, 0, 9)];
        for old in [99, 4] {
            let mut lost = h.clone();
            lost.breaking[0].old_field_id = old;
            assert!(matches!(
                plan(&lost, &f, (0, 10), Target::AsOf(3)).unwrap()[0].columns[1],
                Column::Null { .. }
            ));
        }
    }

    #[test]
    fn the_read_rule_casts_only_without_loss() {
        use IcebergType as T;
        let d = |p, s| T::decimal(p, s).unwrap();
        assert!(casts_to(T::Long, T::Long));
        assert!(!casts_to(d(15, 5), d(15, 6)));
        assert!(casts_to(d(15, 5), d(16, 6)));
        assert!(casts_to(d(15, 5), d(20, 5)));
        assert!(!casts_to(d(15, 5), d(15, 4)));
        assert!(casts_to(T::Long, d(18, 0)));
        assert!(casts_to(T::Long, d(20, 2)));
        assert!(!casts_to(T::Long, d(17, 0)));
        assert!(casts_to(T::Long, T::Double));
        assert!(casts_to(d(15, 5), T::Double));
        assert!(!casts_to(T::Double, d(15, 5)));
        assert!(!casts_to(T::Double, T::Long));
        assert!(!casts_to(T::Long, T::String));
        assert!(!casts_to(T::String, T::Long));
        assert!(!casts_to(T::Date, T::Timestamptz));
        assert!(!casts_to(T::Boolean, T::Long));
    }

    #[test]
    fn refuses_what_it_cannot_read() {
        let s = schemas();
        assert_eq!(
            plan(&s, &[], (0, 1), Target::AsOf(9)),
            Err(PlanError::UnknownSchema(9))
        );
        let mut none = s.clone();
        none.schemas.clear();
        assert_eq!(
            plan(&none, &[], (0, 1), Target::Latest),
            Err(PlanError::UnknownSchema(2))
        );
        let stray = [file("x.parquet", 7, 0, 1)];
        assert_eq!(
            plan(&s, &stray, (0, 2), Target::Latest),
            Err(PlanError::UnknownSchema(7))
        );
        // A narrower target than a file's type: no promotion reaches it.
        let wide = [file("b.parquet", 1, 0, 1)];
        let e = plan(&s, &wide, (0, 2), Target::AsOf(0)).unwrap_err();
        assert!(matches!(e, PlanError::NotAPromotion { ref name, schema: 1, .. } if name == "RRP"));
        assert!(e.to_string().contains("does not promote"), "{e}");
    }

    #[test]
    fn writes_one_duckdb_query() {
        let files = [file("a.parquet", 0, 0, 9), file("it's.parquet", 1, 20, 29)];
        let reads = plan(&schemas(), &files, (0, 100), Target::AsOf(1)).unwrap();
        let sql = duckdb_sql(&reads, Some("SETTLEMENTDATE"), (0, 100));
        assert_eq!(
            sql,
            "SELECT \"SETTLEMENTDATE\" AS \"SETTLEMENTDATE\", CAST(NULL AS DOUBLE) AS \"RAISE\", CAST(\"RRP\" AS DECIMAL(18, 5)) AS \"RRP\"\n\
             FROM read_parquet(['a.parquet'])\n\
             WHERE \"SETTLEMENTDATE\" >= make_timestamptz(0) AND \"SETTLEMENTDATE\" < make_timestamptz(100)\n\
             UNION ALL\n\
             SELECT \"SETTLEMENTDATE\" AS \"SETTLEMENTDATE\", \"RAISE\" AS \"RAISE\", \"RRP\" AS \"RRP\"\n\
             FROM read_parquet(['it''s.parquet'])\n\
             WHERE \"SETTLEMENTDATE\" >= make_timestamptz(0) AND \"SETTLEMENTDATE\" < make_timestamptz(100)"
        );
        // No time column: no bound.
        assert!(!duckdb_sql(&reads, None, (0, 100)).contains("WHERE"));
        assert!(!duckdb_sql(&reads, Some("NONE"), (0, 100)).contains("WHERE"));
        let all = [
            IcebergType::Boolean,
            IcebergType::Long,
            IcebergType::Date,
            IcebergType::String,
            IcebergType::Timestamptz,
        ]
        .map(duck_type);
        assert_eq!(all, ["BOOLEAN", "BIGINT", "DATE", "VARCHAR", "TIMESTAMPTZ"]);
        assert_eq!(ident("a\"b"), "\"a\"\"b\"");
    }
}
