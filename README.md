# utilake-schemas

Every table of the market data AEMO publishes, as an
[Apache Iceberg](https://iceberg.apache.org/) table: its fields with ids
that never change meaning, its schemas, and the runs of AEMO files written
under each, with every change between them. Three markets, each under its
own directory: the National Electricity Market (`nem/`, from nemweb's
MMSDM archive and report folders), the east-coast gas markets (`gas/`:
the Gas Bulletin Board, the Gas Supply Hub, the Short Term Trading Market
and the Declared Wholesale Gas Market, from nemweb's gas report folders)
and the Wholesale Electricity Market of Western Australia (`wem/`, from
data.wa.aemo.com.au's datafiles and WEMDE documents). The schemas are
derived from what AEMO's files hold, file by file, and the code that
derives them is here, so anyone can check the result or improve it.

## Layout

| Path                  | What it holds                                                                                                                                |
| --------------------- | -------------------------------------------------------------------------------------------------------------------------------------------- |
| `tables/{market}/`    | One hand-kept declaration per table of a market (`nem`, `wem`, `gas`): its report pairs, key, time column, load mode, visibility, any column typed by hand, and its temporal roles only when a person set them (`[temporal]`); otherwise the export derives them from the key and the columns. |
| `sources/`             | Every file the evidence was taken from, per month: its URL, SHA-256, size and the time the server says it was published.                       |
| `evidence/census/`    | For every block of every file AEMO published: its header, row count, data span and a profile of each column's values, with each month's control files. No row is copied. |
| `evidence/models/`    | The data-model create scripts the monthly archives ship: each release's tables, columns, types and comments.                                 |
| `evidence/histories/{market}/` | Each table's history as an Iceberg table, folded from all of the above; `BASELINE.toml` names the market's data version when one was re-based. The census, the models and the sources sit under a market directory the same way (`evidence/census/{market}/`, `evidence/models/{market}/`, `sources/{market}/`). |
| `{market}/`           | The published tables of a market (`nem/`, `wem/`, `gas/`): `{market}/{TABLE}/metadata/` is the table as Iceberg table metadata (`v1.metadata.json`, `version-hint.text`, one empty manifest list per run); beside it one `schema-{id}.json` per schema, derived from the metadata; `{market}/headers.json`; `{market}/folders.json` (which report folder of a feed carries each block, from the folder census; absent until one is recorded); `manifest.json` names every file with its SHA-256, and each market's data version and tables under `markets`. |
| `crates/`             | `utilake-iceberg`, the Iceberg contract (no I/O): types, schemas, histories, the planner, metadata as bytes; `utilake-mms-decode`, AEMO's MMS files decoded; `utilake-plain-decode`, AEMO's header-row CSV and WEMDE JSON files decoded, with the naming, release and row-diff rules of every feed that is not MMSDM; `utilake-provenance`, what an archived file is and every place AEMO served it (the record beside each file, the one time type, the archive, and the rule that names a file's local path); `utilake-zip`, zips walked; `utilake-gates`, the gates every utilake repository runs; `utilake-schemas`, the tool; `xtask`, this repository's gates. utilake depends on these crates by git revision. |

## The pipeline

Each step reads the one before and writes the same bytes from the same
input: no clock, no network after `fetch`, and no order of work shows in
the output.

```sh
utilake-schemas fetch --month 2026-09 --dir mirror/MMSDM_2026_09   # sources/2026-09.tsv
utilake-schemas census --month 2026-09 --dir mirror/MMSDM_2026_09  # evidence/census/2026-09.toml, evidence/ctl/
utilake-schemas reports --dir archive/www.nemweb.com.au [--market gas] [--feed gbb] [--mmsdm mirror]  # evidence/census/{market}/{feed}/
utilake-schemas sources --archive archive                          # sources/, for a census of a mirror archive
utilake-schemas models --month 2026-09 --dir mirror/MMSDM_2026_09  # evidence/models/
utilake-schemas fold [--market nem]                                # evidence/histories/{market}/
utilake-schemas export                                             # {market}/, manifest.json
utilake-schemas iceberg --out target/iceberg                       # the same tables at file locations
```

`fetch` downloads one month of the MMSDM archive, one request at a time,
and records each file in `sources/{YYYY-MM}.tsv` as it lands. A census
taken from a local mirror archive instead (nemweb as laid out under
`www.nemweb.com.au/`, a provenance record `{file}.provenance.json` beside
each file, or `{file}.json` where the mirror wrote it before format 2) has
`sources/` written by `sources --archive DIR` (`just sources DIR`): one row
per file every census under `evidence/census/` records, report folders
included, with the URL and `Last-Modified` its sidecar gives and the
SHA-256 and size the census gives, and, in a month where any file has
one, a fifth column `archived_in`: the day's zip and entry the mirror
found the same bytes in (`{zip URL}!{entry}`), where they are still
served once AEMO stops listing the URL. A file without a sidecar, or whose
sidecar disagrees with the census, is named, every one, and nothing is
written; `--check` lists each sources file that would change. The
publication time is held in `sources/` alone: no census, history or
document under `nem/` depends on it.
`reports` reads a market's feeds from the host directory of a mirror
archive (`…/archive/www.nemweb.com.au`, `…/archive/data.wa.aemo.com.au`),
kept one local folder per report folder: AEMO lists a folder's files
under `Reports/CURRENT/` for a few days and the day's zip of them under
`Reports/ARCHIVE/`, and the archive keeps both at `Reports/{folder}/`
(`utilake_provenance::layout`), the listing in each record's URL. So a
5-minute file and the day's zip holding it are one folder, and a CSV read
in both counts once (`repeated`). The feeds: the NEM's `Reports` (less the
gas folders), the gas feeds `gsh` (MMS rows under `Reports/GSH`), `gbb`,
`sttm` and `dwgm` (header-row CSVs under `Reports/{GBB,STTM,VicGas}`), and
the WEM's `datafiles` (header-row CSVs) and `wemde` (JSON documents, one
block per array). A folder's census is
`evidence/census/{market}/{feed}/{folder}.toml` (the NEM's feed is
`reports`) and lists the listings its files came from. A plain file's block is named by
its path (`crates/utilake-plain-decode`: the family is the feed, the name
the file's stem, the version its `_vN`), its columns by their headers,
every date rewritten into the MMS form before it is read, and `true` and
`false` as booleans. A block whose header names a column per forecast
day (the GBB's `ForecastUtilisation`) is read with those columns
unpivoted into a date and a value, so its shape does not change daily.
A file's first bytes say what it is before its feed's decoder reads it:
a `C` row is read as MMS rows whatever the folder (the GBB's GSH trades
file), and markup named `.csv` counts among the files that are not
reports. An entry found again under another file with the
same name and bytes (a day's slot zip beside the current day's) is read
once and counted as `repeated`; a feed whose files restate the ones
before them has each file's rows compared against the last file of the
same block, so a run's `rows` are the rows its files added and
`rows_repeated` the rest. A file nemweb rewrote in place is kept by utilake-mirror
as `{name}@{listed time}` beside the first version; each such file is listed
under `[[rewritten]]`, every version classed `first`, `data` (new data),
`report_time` (only AEMO's report time changed) or `reverted` (an earlier
version's data again). With `--mirror` (the utilake-mirror home, else
`UTILAKE_MIRROR_HOME`), it also counts from the mirror's journal how often
the file was listed again with the same bytes (`relisted`) or as a zip of
the same entries (`repacked`). A record of format 1, the flat sidecar the
mirror wrote before, is read as format 2 (`utilake_provenance::provenance`).
`models --market gas --dir …/MMSDataModelReport/Gas` reads AEMO's Gas
Data Model Report (a web report, not a create script) into the same model
files; the WEM publishes no model, so its tables are declared by hand.
`fold` reads a market's evidence in the market's clock (+10:00 for the
NEM and gas, +08:00 for the WEM) and keeps every history to the one the
market's directory last published (read back out of each table's
metadata): a field keeps its id, name and type, a schema its id, and every
file its schema, however much evidence from before or between the
published files is added later; data a field's type cannot hold gets a
new field, recorded as breaking. Once, `fold --market M --rebaseline
REASON` keeps nothing and begins the market's next data version, recorded
in `evidence/histories/{market}/BASELINE.toml` and `manifest.json`.
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
just export-check                         # every market directory follows from the histories
just replay                               # two Iceberg readers read every table as its history says
```

## Reading a table

A table is its Iceberg table metadata,
`{market}/{TABLE}/metadata/v1.metadata.json` (format version 3, no data
files). Any Iceberg reader loads it as it is;
the published location is the raw GitHub URL, and `just iceberg` writes
the same tables under `target/iceberg/` at file locations for readers
that cannot fetch over HTTP:

```python
from pyiceberg.table import StaticTable
t = StaticTable.from_metadata("target/iceberg/nem/DISPATCHPRICE/metadata/v1.metadata.json")
print(t.schemas(), t.properties["comment"])
for s in t.snapshots():            # one per run of AEMO files, in release order
    print(s.schema_id, s.summary["utilake.from.file"], s.summary["utilake.rows"])
print(t.scan().to_arrow())         # the current columns, no rows
```

```sql
-- DuckDB, with the iceberg extension
DESCRIBE SELECT * FROM iceberg_scan('target/iceberg/nem/DISPATCHPRICE');   -- the columns, no rows
SELECT * FROM iceberg_snapshots('target/iceberg/nem/DISPATCHPRICE');       -- one row per run
```

`just replay` reads every table back with pyiceberg and DuckDB and holds
it to its history. Beyond what Iceberg holds of any table (the schemas by
id, `current-schema-id`), the metadata carries, as strings:

| Where                       | Key                                                                                   | What it holds                                                                                                                                                      |
| --------------------------- | ------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `properties`                | `utilake.format`, `utilake.table`                                                     | The layout's generation (3) and the table's name.                                                                                                                  |
| `properties`                | `utilake.market`, `utilake.offset`                                                    | The market (`wem`, `gas`) and its clock (`+08:00`, `+10:00`), for a table that is not the NEM's; a NEM table carries neither, and reads at `+10:00`.                 |
| `properties`                | `utilake.headers`, `utilake.model`                                                    | The header a file wrote for a column named from it (`{field id: "Load Forecast (MW)"}`), and the data-model tables whose words describe a table not named in the model (a JSON array); only when there are any. |
| `properties`                | `utilake.declared`                                                                    | Whether `tables/{market}/` declares the table. When `true`: `utilake.package`, `utilake.reports` (`[[type, subtype], …]`), `utilake.key`, `utilake.time`, `utilake.load`, `utilake.visibility`, `utilake.temporal.valid.kind`, `utilake.temporal.valid.column`, `utilake.temporal.valid.until` (a `from` table's end column, when it has one), `utilake.temporal.entity`, `utilake.temporal.order`, `utilake.temporal.cutoff`, `utilake.temporal.origin`. Every list is a JSON array. |
| `properties`                | `comment`, `utilake.model-types`                                                      | What the table holds and each column's type (`{field id: "NUMERIC(16,6)"}`) in the latest data-model release that has them; each column's words are its field's `doc`. |
| `properties`                | `utilake.breaks`                                                                      | The columns whose values left their field's type: `[{column, old-field-id, old-type, new-field-id, new-type, at: {file, sha256, release, data-time}}]`.             |
| each snapshot's `summary`   | `utilake.report-type`, `utilake.report-subtype`, `utilake.version`, `utilake.kinds`  | The run's report pair, AEMO version and the kinds of file it came in. For a file without MMS rows the report type is its feed (`GBB`, `STTM`, `DWGM`, `WEM`, `WEMDE`), the subtype the file's stem (`ACTUAL_FLOW_STORAGE`, `INT651_EX_ANTE_MARKET_PRICE_RPT_1`, `REFERENCE_TRADING_PRICE_REFERENCE_TRADING_PRICES`) and the version its `_vN`, else 1. |
| each snapshot's `summary`   | `utilake.files`, `utilake.rows`, `utilake.total-rows`                                 | Its files and rows, and the rows of every run so far.                                                                                                              |
| each snapshot's `summary`   | `utilake.from.file`, `.sha256`, `.release`, `.time`; `utilake.to.*`                   | Its first and last file: name, SHA-256, release month, and where its data starts or ends (absent when the rows carry no time).                                     |

Each schema's `schema-{id}.json` is a JSON Schema of a row under that
schema, with each field's id, Iceberg type, the header it was written
under where that is not its name (`x-header`) and, where the data model
has them, its words and model type; `{market}/headers.json` maps every header
AEMO's files carry (report pair, version, columns in order) to its table
and the schemas it was read with, each with the releases of its runs (a
file resolves to the last schema whose first release is at or before the
file's, else the first); `{market}/folders.json` says which report folder of
which feed carried each report pair and version, and over which releases,
so a reader fetching an interval of a table from the report folders rather
than the monthly archive knows where to look: each folder is `{feed, root,
folder, listings, blocks}`, `root` and `folder` as the archive keeps them
(`Reports`, `Dispatch_SCADA`) and `listings` the ones AEMO showed its files
under (`ARCHIVE`, `CURRENT`), so a reader lists
`{root}/{listing}/{folder}/` on the host
(`utilake_provenance::layout::listed_path`). All three are derived at export;
nothing in them is not in it.

What the metadata means, beyond Iceberg:

- **Runs.** A snapshot is a run: AEMO's files in release order under one
  schema, report version and report pair. Its `schema-id` is the run's;
  its time is the start of the month the run's first file was released
  in, so snapshots rise in release order; its manifest list is empty.
  Kinds are `dvd` and `archive` (AEMO's monthly archive; `#all` for a
  file of every forecast run), `other`, `reports` (the NEM's report
  folders) and the feed's id for every other feed (`gbb`, `gsh`,
  `datafiles`); revisions folded before the report folders were held once
  name `reports/archive`, `reports/current` and `{feed}/{root}`. What changed at a run's start
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

A time is RFC 3339, or a day, which is its midnight in the table's
market (`--market`, `nem` by default: +10:00; the WEM's +08:00).

Every lake file holds two columns after its table's, so each row says
where it came from whatever is done to the file later:

| Column | Field id | Type | Holds |
|---|---|---|---|
| `_nl_source` | 2147483000 | `string` | The file the row was read from: its URL, and `!{entry}` for a file inside a zip |
| `_nl_published` | 2147483001 | `timestamptz` | When that file became public; null when nothing says |

A row's copies rank by the table's order columns, then `_nl_published`,
then `_nl_source`; a point-in-time read keeps rows with `_nl_published`
at or before its time. No table may declare a column starting `_nl_`.

## Contributing

A table's types come from its evidence by a rule, so a change is one of
two kinds:

- **An override**: a `[[override]]` in the table's file under `tables/`,
  setting a column's type, with a `reason` that cites the evidence (the
  census month, file or data-model release that shows it).
- **A rule change**: a change to the code under `crates/`, with a test
  that shows the case it fixes.

Run `just fold` (per market) and `just export`, and commit the evidence,
histories and market directories together. CI proves the effect: `just
export-check` that every market directory is what the evidence exports, `just compat origin/main` that nothing
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
files under `evidence/`, `sources/`, `nem/`, `wem/`, `gas/` and
`testdata/`, and `manifest.json`, are derived from information AEMO publishes and are
attributed to AEMO; they are not covered by that licence. See
`LICENCE-DATA.md` and `NOTICE`. This repository is not produced or
endorsed by AEMO.
