# utilake-schemas

Every table of the National Electricity Market data that AEMO publishes on
nemweb.com.au, as an [Apache Iceberg](https://iceberg.apache.org/) table:
its fields with ids that never change meaning, its schemas, and the runs
of AEMO files written under each, with every change between them. The
schemas are derived from what AEMO's files hold, file by file, and the
code that derives them is here, so anyone can check the result or improve
it.

## Layout

| Path                  | What it holds                                                                                                                                |
| --------------------- | -------------------------------------------------------------------------------------------------------------------------------------------- |
| `tables/`             | One hand-kept declaration per table: its report pairs, key, time column, temporal roles, load mode, visibility and any column typed by hand. |
| `sources/`             | Every file the evidence was taken from, per month: its URL, SHA-256, size and the time the server says it was published.                       |
| `evidence/census/`    | For every block of every file AEMO published: its header, row count, data span and a profile of each column's values, with each month's control files. No row is copied. |
| `evidence/models/`    | The data-model create scripts the monthly archives ship: each release's tables, columns, types and comments.                                 |
| `evidence/histories/` | Each table's history as an Iceberg table, folded from all of the above; `BASELINE.toml` names the data version when one was re-based.         |
| `nem/`                | The published tables: `nem/{TABLE}/metadata/` is the table as Iceberg table metadata (`v1.metadata.json`, `version-hint.text`, one empty manifest list per run); beside it one `schema-{id}.json` per schema, derived from the metadata; `nem/headers.json`; `manifest.json` names every file with its SHA-256. |
| `crates/`             | `utilake-iceberg`, the Iceberg contract (no I/O): types, schemas, histories, the planner, metadata as bytes; `utilake-mms-decode`, AEMO's MMS files decoded; `utilake-zip`, zips walked; `utilake-gates`, the gates every utilake repository runs; `utilake-schemas`, the tool; `xtask`, this repository's gates. utilake and pigface-io depend on the first four by git revision. |

## The pipeline

Each step reads the one before and writes the same bytes from the same
input: no clock, no network after `fetch`, and no order of work shows in
the output.

```sh
utilake-schemas fetch --month 2026-09 --dir mirror/MMSDM_2026_09   # sources/2026-09.tsv
utilake-schemas census --month 2026-09 --dir mirror/MMSDM_2026_09  # evidence/census/2026-09.toml, evidence/ctl/
utilake-schemas reports --dir mirror/Reports --mmsdm mirror        # evidence/census/reports/
utilake-schemas sources --archive archive                          # sources/, for a census of a mirror archive
utilake-schemas models --month 2026-09 --dir mirror/MMSDM_2026_09  # evidence/models/
utilake-schemas fold                                               # evidence/histories/
utilake-schemas export                                             # nem/, manifest.json
utilake-schemas iceberg --out target/iceberg                       # the same tables at file locations
```

`fetch` downloads one month of the MMSDM archive, one request at a time,
and records each file in `sources/{YYYY-MM}.tsv` as it lands. A census
taken from a local mirror archive instead (nemweb as laid out under
`www.nemweb.com.au/`, a sidecar `{file}.json` beside each file) has
`sources/` written by `sources --archive DIR` (`just sources DIR`): one row
per file every census under `evidence/census/` records, report folders
included, with the URL and `Last-Modified` its sidecar gives and the
SHA-256 and size the census gives. A file without a sidecar, or whose
sidecar disagrees with the census, is named, every one, and nothing is
written; `--check` lists each sources file that would change. The
publication time is held in `sources/` alone: no census, history or
document under `nem/` depends on it.
`reports` reads a local copy of nemweb's `Reports/ARCHIVE` and
`Reports/CURRENT`. A file nemweb rewrote in place is kept by nemweb-mirror
as `{name}@{listed time}` beside the first version; each such file is listed
under `[[rewritten]]`, every version classed `first`, `data` (new data),
`report_time` (only AEMO's report time changed) or `reverted` (an earlier
version's data again). With `--mirror` (the nemweb-mirror home, else
`NEMWEB_MIRROR_HOME`), it also counts from the mirror's journal how often
the file was listed again with the same bytes (`relisted`) or as a zip of
the same entries (`repacked`). A sidecar written before nemweb-mirror kept
its `_at` instants has them read from the text beside them.
`fold` keeps every history to the one `nem/` last
published (read back out of each table's metadata): a field keeps its id,
name and type, a schema its id, and every file its schema, however much
evidence from before or between the published files is added later; data
a field's type cannot hold gets a new field, recorded as breaking. Once,
`fold --rebaseline REASON` keeps nothing and begins the next data version,
recorded in `evidence/histories/BASELINE.toml` and `manifest.json`.
`export` writes each table's metadata and everything derived from it.
Each step has a `just` recipe of the same name.

## Checking it from AEMO's files

Every file the evidence was taken from is named in `sources/` with its
SHA-256, and every census entry carries the SHA-256 of the file it read.
To check a month, fetch it, take its census again, and compare
(`sources/` may differ only in the publication times the server sends):

```sh
just fetch 2026-09 mirror/MMSDM_2026_09
just census 2026-09 mirror/MMSDM_2026_09
just models 2026-09 mirror/MMSDM_2026_09
git diff --exit-code evidence/            # the same bytes
just fold-check                           # the histories follow from the evidence
just export-check                         # nem/ follows from the histories
just iceberg && just replay               # two Iceberg readers read every table as its history says
```

## Reading a table

A table is its Iceberg table metadata, `nem/{TABLE}/metadata/v1.metadata.json`
(format version 3, no data files). Any Iceberg reader loads it as it is;
the published location is the raw GitHub URL, and `just iceberg` writes
the same tables under `target/iceberg/` at file locations for readers
that cannot fetch over HTTP:

```python
from pyiceberg.table import StaticTable
t = StaticTable.from_metadata("target/iceberg/DISPATCHPRICE/metadata/v1.metadata.json")
print(t.schemas(), t.properties["comment"])
for s in t.snapshots():            # one per run of AEMO files, in release order
    print(s.schema_id, s.summary["utilake.from.file"], s.summary["utilake.rows"])
print(t.scan().to_arrow())         # the current columns, no rows
```

```sql
-- DuckDB, with the iceberg extension
DESCRIBE SELECT * FROM iceberg_scan('target/iceberg/DISPATCHPRICE');   -- the columns, no rows
SELECT * FROM iceberg_snapshots('target/iceberg/DISPATCHPRICE');       -- one row per run
```

`just replay` reads every table back with pyiceberg and DuckDB and holds
it to its history. Beyond what Iceberg holds of any table (the schemas by
id, `current-schema-id`), the metadata carries, as strings:

| Where                       | Key                                                                                   | What it holds                                                                                                                                                      |
| --------------------------- | ------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `properties`                | `utilake.format`, `utilake.table`                                                     | The layout's generation (3) and the table's name.                                                                                                                  |
| `properties`                | `utilake.declared`                                                                    | Whether `tables/` declares the table. When `true`: `utilake.package`, `utilake.reports` (`[[type, subtype], …]`), `utilake.key`, `utilake.time`, `utilake.load`, `utilake.visibility`, `utilake.temporal.valid.kind`, `utilake.temporal.valid.column`, `utilake.temporal.entity`, `utilake.temporal.order`, `utilake.temporal.cutoff`, `utilake.temporal.origin`. Every list is a JSON array. |
| `properties`                | `comment`, `utilake.model-types`                                                      | What the table holds and each column's type (`{field id: "NUMERIC(16,6)"}`) in the latest data-model release that has them; each column's words are its field's `doc`. |
| `properties`                | `utilake.breaks`                                                                      | The columns whose values left their field's type: `[{column, old-field-id, old-type, new-field-id, new-type, at: {file, sha256, release, data-time}}]`.             |
| each snapshot's `summary`   | `utilake.report-type`, `utilake.report-subtype`, `utilake.version`, `utilake.kinds`  | The run's report pair, AEMO version and the kinds of file it came in.                                                                                              |
| each snapshot's `summary`   | `utilake.files`, `utilake.rows`, `utilake.total-rows`                                 | Its files and rows, and the rows of every run so far.                                                                                                              |
| each snapshot's `summary`   | `utilake.from.file`, `.sha256`, `.release`, `.time`; `utilake.to.*`                   | Its first and last file: name, SHA-256, release month, and where its data starts or ends (absent when the rows carry no time).                                     |

Each schema's `schema-{id}.json` is a JSON Schema of a row under that
schema, with each field's id, Iceberg type and, where the data model has
them, its words and model type; `nem/headers.json` maps every header
AEMO's files carry (report pair, version, columns in order) to its table
and the schemas it was read with, each with the releases of its runs (a
file resolves to the last schema whose first release is at or before the
file's, else the first). Both are derived from the metadata at export;
nothing in them is not in it.

What the metadata means, beyond Iceberg:

- **Runs.** A snapshot is a run: AEMO's files in release order under one
  schema, report version and report pair. Its `schema-id` is the run's;
  its time is the start of the month the run's first file was released
  in, so snapshots rise in release order; its manifest list is empty.
  Kinds are `dvd` and `archive` (AEMO's monthly archive; `#all` for a
  file of every forecast run), `other`, and `reports/archive` and
  `reports/current` (the report folders). What changed at a run's start
  and the schema updates from the run before are not stored: they follow
  from the snapshots in order, and `utilake_iceberg::history::derive`
  computes them as the fold did.
- **One type for life.** A field keeps one type. Values a field's type
  cannot hold (a wider scale, a float printed in full, text) get a new
  field under the same name, listed under `utilake.breaks` with the old
  and new field and the file it happened in. Iceberg's own promotions
  (a wider decimal precision) are never needed: a type is chosen over
  everything the column ever held.
- **Reading across a break.** An Iceberg reader reads a file from
  before a break as null for the new field. `utilake-schemas sql` reads
  the old field as the new one when every value of the old type is a
  value of the new (a wider decimal, a double), and null when the column
  turned to text. This is a read rule of the tool, not a promotion: the
  table metadata never claims it.
- **Restored fields.** A table whose files alternate between two column
  lists has two schemas, and a field left out of one is deleted and
  restored with its id rather than minted again. Iceberg's metadata
  allows it; its schema-update API would mint a new id.
- **The current schema** is the last run's, as `current-schema-id`.
- **Snapshot ids** are a hash of the run's first file, report pair,
  version and schema, so a run kept across exports keeps its id; the
  metadata file is rewritten in place at each export, as a static table.
- **Data versions.** `manifest.json` names the data version
  (`baseline`). Within one version every published id keeps its meaning;
  a later version was re-based for the reason given, and ids may differ.

To read an interval of a table's Parquet files written under different
schemas, list them and plan the read from the published metadata alone;
the output is one DuckDB query that projects every file onto one schema
by field id:

```sh
# files.json: [{"path": "…parquet", "schema_id": 0, "min_us": …, "max_us": …}]
utilake-schemas sql DISPATCHPRICE --files files.json --from 2024-08-01 --to 2024-09-01 [--as-of 0]
```

A time is RFC 3339, or a day, which is its midnight in market time
(+10:00).

## Contributing

A table's types come from its evidence by a rule, so a change is one of
two kinds:

- **An override**: a `[[override]]` in the table's file under `tables/`,
  setting a column's type, with a `reason` that cites the evidence (the
  census month, file or data-model release that shows it).
- **A rule change**: a change to the code under `crates/`, with a test
  that shows the case it fixes.

Run `just fold` and `just export`, and commit the evidence, histories and
`nem/` together. CI proves the effect: `just export-check` that `nem/` is
what the evidence exports, `just compat origin/main` that nothing
published changed meaning (every table's metadata keeps what it promised,
every schema document says the same of its fields), `just iceberg` and
`just replay` that two Iceberg readers read every table as its history
says, and `just diff origin/main` summarises, per table, the schemas,
runs, fields and breaks added. See `CONTRIBUTING.md`
for commits and gates; install [just](https://just.systems),
[prek](https://prek.j178.dev) and `cargo-llvm-cov`, then `just hooks` and
`just check`.

## Licences

The code under `crates/` is licensed under Apache-2.0 (`LICENSE`). The
files under `evidence/`, `sources/`, `nem/` and `testdata/`, and
`manifest.json`, are derived from information AEMO publishes and are
attributed to AEMO; they are not covered by that licence. See
`LICENCE-DATA.md` and `NOTICE`. This repository is not produced or
endorsed by AEMO.
