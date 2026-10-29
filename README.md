# phasorlake-schemas

Every table of the market data AEMO publishes, as an
[Apache Iceberg](https://iceberg.apache.org/) v3 table: its fields with ids
that never change meaning, its schemas, and the runs of AEMO files written
under each. Two markets are published: the National Electricity Market
(`nem/`, from nemweb's MMSDM archive and report folders) and the east-coast
gas markets (`gas/`: the Gas Bulletin Board, the Gas Supply Hub, the Short
Term Trading Market and the Declared Wholesale Gas Market).

The schemas are derived from what AEMO's files hold, file by file, checked
against AEMO's control files and data models, and the code that derives
them is here, so anyone can check the result or improve it. No key or
temporal roles are published: which rows are one fact, and which
restatement is the latest, are a reader's decisions.

## Layout

| Path | What it holds |
|---|---|
| `{market}/{TABLE}/` | The published table: `metadata/v1.metadata.json` (and `version-hint.text`, one empty manifest list per run), and one `schema-{id}.json` per schema |
| `{market}/headers.json` | Every header AEMO's files carry, to its table and schemas |
| `{market}/folders.json` | Which report folder carries each block, over which releases |
| `manifest.json` | Every published file with its SHA-256, and each market's data version |
| `tables/{market}/` | One hand-kept declaration per table: report pairs, time column, load mode, columns typed by hand |
| `evidence/` | The census of every block of every file (no row copied), the data models, loader configurations and each table's folded history |
| `sources/` | Every file the evidence was taken from: URL, SHA-256, size, publication time |
| `crates/` | The code: `phasor-iceberg` (the contract), the decoders, `phasor-provenance`, `phasor-zip`, `phasor-diag`, `phasor-gates` and the `phasor-schemas` tool |

## Reading a table

A table is its Iceberg table metadata, with no data files. Any Iceberg
reader loads it from the raw GitHub URL; `just iceberg` writes the same
tables under `target/iceberg/` for readers that cannot fetch over HTTP.

```python
from pyiceberg.table import StaticTable
t = StaticTable.from_metadata("target/iceberg/nem/DISPATCHPRICE/metadata/v1.metadata.json")
print(t.schemas(), t.properties["comment"])
for s in t.snapshots():            # one per run of AEMO files, in release order
    print(s.schema_id, s.summary["phasor.from.file"], s.summary["phasor.rows"])
```

```sql
-- DuckDB, with the iceberg extension
DESCRIBE SELECT * FROM iceberg_scan('target/iceberg/nem/DISPATCHPRICE');
SELECT * FROM iceberg_snapshots('target/iceberg/nem/DISPATCHPRICE');
```

Beyond what Iceberg holds, the metadata carries, as strings:

| Where | Key | What it holds |
|---|---|---|
| `properties` | `phasor.format`, `phasor.table` | The layout's generation (5) and the table's name |
| `properties` | `phasor.market`, `phasor.offset` | The market and its clock, for a table that is not the NEM's (a NEM table reads at `+10:00`) |
| `properties` | `phasor.declared` | Whether `tables/` declares it; when it does, `phasor.package`, `phasor.reports`, `phasor.with-columns`, `phasor.time`, `phasor.load`, `phasor.visibility` |
| `properties` | `comment`, `phasor.model-types`, `phasor.model`, `phasor.headers` | The data model's words and types for the table and its columns, and the header a column was named from |
| `properties` | `phasor.breaks` | Columns whose values left their field's type: the old and new field, and the file it happened in |
| snapshot `summary` | `phasor.report-type`, `phasor.report-subtype`, `phasor.version`, `phasor.kinds` | The run's report pair, AEMO version and the kinds of file it came in |
| snapshot `summary` | `phasor.files`, `phasor.rows`, `phasor.total-rows` | Its files and rows, and the rows of every run so far |
| snapshot `summary` | `phasor.from.*`, `phasor.to.*` | Its first and last file: name, SHA-256, release month, where its data starts or ends |

What it means:

- **A snapshot is a run**: AEMO's files in release order under one schema,
  report version and report pair, timed at the month its first file was
  released. Snapshot ids hash the run's first file, pair, version and
  schema, so a run keeps its id across exports.
- **One type for life.** Values a field's type cannot hold get a new field
  under the same name, listed under `phasor.breaks`; a type is chosen over
  everything the column ever held, so Iceberg's promotions are never
  needed. `phasor-schemas sql` reads an old field as the new one where
  every old value is a new one.
- **Restored fields.** A table whose files alternate between two column
  lists restores a field with its id rather than minting a new one.
- **Data versions.** Within one data version (`manifest.json`) every
  published id keeps its meaning.

To read Parquet files written under different schemas, plan the read from
the published metadata alone: one DuckDB query projecting every file onto
one schema by field id.

```sh
# files.json: [{"path": "…parquet", "schema_id": 0, "min_us": …, "max_us": …}]
phasor-schemas sql DISPATCHPRICE --files files.json --from 2024-08-01 --to 2024-09-01
```

## Provenance columns

A lake file holds four columns after its table's. Each time is its own
column, transcribed as found and named for where it was read; nothing is
combined at write. No table may declare a column starting `_phasor_`.

| Column | Field id | Type | Holds |
|---|---|---|---|
| `_phasor_source` | 2147483000 | `string` | The URL the row was read from, and `!{entry}` for a file inside a zip |
| `_phasor_server_last_modified` | 2147483001 | `timestamptz` | The HTTP `Last-Modified` the server gave: never early, late where the server touched a file again (AEMO touched most of its archive in April 2026) |
| `_phasor_zip_entry_modified` | 2147483002 | `timestamptz` | The entry's modified time in its zip (two-second resolution, the packer's clock) |
| `_phasor_filename_timestamp` | 2147483003 | `timestamptz` | The first run of exactly fourteen digits (`YYYYMMDDHHMMSS`) in the file name; the twelve-digit interval stamp is not read |

## The pipeline

```sh
phasor-schemas fetch --month 2026-09 --dir mirror/MMSDM_2026_09   # sources/
phasor-schemas census --month 2026-09 --dir mirror/MMSDM_2026_09  # evidence/census/
phasor-schemas reports --dir archive/www.nemweb.com.au [--market gas] [--feed gbb]
phasor-schemas sources --archive archive                          # sources/ from a mirror archive
phasor-schemas models --month 2026-09 --dir mirror/MMSDM_2026_09  # evidence/models/
phasor-schemas fold [--market nem]                                # evidence/histories/
phasor-schemas export                                             # {market}/, manifest.json
```

Each step writes the same bytes from the same input, and has a `just`
recipe of the same name; `docs/pipeline.md` says what each one reads and
records. To check a month against AEMO's files:

```sh
just fetch 2026-09 mirror/MMSDM_2026_09
just census 2026-09 mirror/MMSDM_2026_09
just models 2026-09 mirror/MMSDM_2026_09
git diff --exit-code evidence/   # the same bytes
just fold-check                  # the histories follow from the evidence
just export-check                # every market directory follows from the histories
just replay                      # pyiceberg and DuckDB read every table as its history says
```

## Contributing

A table's types come from its evidence by a rule, so a change is one of:

- **An override**: a `[[override]]` in the table's declaration, setting a
  column's type, with a `reason` citing the evidence.
- **Two kinds of row under one pair**: a second table with
  `with_columns = ["AREAID"]`, as the `*_AREA` tables take AEMO's area
  reports from the regional ones.
- **A rename**: `[[rename]]` (`from`, `to`, `evidence`) makes a renamed
  column one field; columns that only look renamed are `[[distinct]]`.
- **A rule change**: a change under `crates/`, with a test showing the case.

Run `just fold` and `just export`, and commit the evidence, histories and
market directories together. CI holds `just export-check`, `just compat
origin/main` (nothing published changed meaning) and `just replay`;
`just diff origin/main` summarises what a change adds. See
`CONTRIBUTING.md` for commits and gates.

## Licences

The code under `crates/` is Apache-2.0 (`LICENSE`). The files under
`evidence/`, `sources/`, `nem/`, `gas/` and `testdata/`, and
`manifest.json`, are derived from information AEMO publishes and are
attributed to AEMO; they are not covered by that licence. See
`LICENCE-DATA.md` and `NOTICE`. This repository is not produced or
endorsed by AEMO.
