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
| `evidence/census/`    | For every block of every file AEMO published: its header, row count, data span and a profile of each column's values. No row is copied.      |
| `evidence/ctl/`       | AEMO's control files, byte for byte, named by their SHA-256.                                                                                 |
| `evidence/models/`    | The data-model create scripts the monthly archives ship: each release's tables, columns, types and comments.                                 |
| `evidence/histories/` | Each table's history as an Iceberg table, folded from all of the above.                                                                      |
| `nem/`                | The published documents: `nem/{TABLE}/table.json` and one `schema-{hash}.json` per schema; `manifest.json` names each with its SHA-256.      |
| `crates/`             | `utilake-iceberg`, the Iceberg contract (no I/O); `utilake-schemas`, the tool; `xtask`, the repository's gates.                                      |

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
published: a field keeps its id, name and type, a schema its id, and
data a field's type cannot hold gets a new field, recorded as breaking.
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
```

## Reading a table

`nem/{TABLE}/table.json` holds the table's declaration, each column's
comment and model type from the latest data-model release that has it,
and its history (`iceberg`): schemas, runs, the updates between them and
the breaking columns. Each `schema-{hash}.json` is a JSON Schema of a row
under one Iceberg schema; its name is a hash of its fields and key, so a
published document never changes.

To read the histories with an Iceberg library, write them as tables
without data:

```sh
utilake-schemas iceberg --out target/iceberg   # target/iceberg/{TABLE}/metadata/
just replay                                # every table read back by pyiceberg
```

```python
from pyiceberg.table import StaticTable
t = StaticTable.from_metadata("target/iceberg/DISPATCHPRICE/metadata/v1.metadata.json")
print(t.schemas())
```

```sql
-- DuckDB, with the iceberg extension: the columns, no rows
DESCRIBE SELECT * FROM iceberg_scan('target/iceberg/DISPATCHPRICE');
```

To read an interval of a table's Parquet files written under different
schemas, list them and plan the read; the output is one DuckDB query that
projects every file onto one schema by field id:

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
published changed meaning, and `just diff origin/main` summarises, per
table, the schemas, runs, fields and breaks added. See `CONTRIBUTING.md`
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
