# The pipeline, step by step

Each step reads the one before and writes the same bytes from the same
input: no clock, no network after `fetch`, and no order of work shows in
the output. Each has a `just` recipe of the same name.

## fetch, sources

`fetch` downloads one month of the MMSDM archive, one request at a time,
and records each file in `sources/{market}/{YYYY-MM}.tsv` as it lands.

A census taken from a local mirror archive instead (nemweb as laid out
under `www.nemweb.com.au/`, a provenance record `{file}.provenance.json`
beside each file, or `{file}.json` where the mirror wrote it before format
2) has `sources/` written by `sources --archive DIR`: one row per file
every census records, with the URL and `Last-Modified` its record gives
and the SHA-256 and size the census gives. In a month where any file has
one, a fifth column `archived_in` names the day's zip and entry the mirror
found the same bytes in (`{zip URL}!{entry}`), where they are still served
once AEMO stops listing the URL. A file without a record, or whose record
disagrees with the census, is named, every one, and nothing is written;
`--check` lists each sources file that would change. The publication time
is held in `sources/` alone: nothing under a market directory depends on
it.

## census, reports

`census` records every block of every file of a month's directory, with
each month's control files. `reports` reads a market's feeds from the host
directory of a mirror archive (`…/archive/www.nemweb.com.au`), kept one
local folder per report folder: AEMO lists a folder's files under
`Reports/CURRENT/` for a few days and the day's zip of them under
`Reports/ARCHIVE/`, and the archive keeps both at `Reports/{folder}/`
(`phasor_provenance::layout`), the listing in each record's URL. So a
5-minute file and the day's zip holding it are one folder, and a CSV read
in both counts once (`repeated`).

The feeds: the NEM's `Reports` (less the gas folders), and the gas feeds
`gsh` (MMS rows under `Reports/GSH`), `gbb`, `sttm` and `dwgm` (header-row
CSVs under `Reports/{GBB,STTM,VicGas}`). A folder's census is
`evidence/census/{market}/{feed}/{folder}.toml` (the NEM's feed is
`reports`) and lists the listings its files came from.

- A plain file's block is named by its path (the family is the feed, the
  name the file's stem, the version its `_vN`), its columns by their
  headers, every date rewritten into the MMS form before it is read, and
  `true` and `false` as booleans.
- A block whose header names a column per forecast day (the GBB's
  `ForecastUtilisation`) is read with those columns unpivoted into a date
  and a value, so its shape does not change daily.
- A file's first bytes say what it is: a `C` row is read as MMS rows
  whatever the folder, and markup named `.csv` counts among the files that
  are not reports.
- An entry found again under another file with the same name and bytes is
  read once and counted as `repeated`. A feed whose files restate the ones
  before them has each file's rows compared against the last file of the
  same block, so a run's `rows` are the rows its files added and
  `rows_repeated` the rest.
- A file nemweb rewrote in place is kept by the mirror as
  `{name}@{listed time}` beside the first version, and listed under
  `[[rewritten]]`, every version classed `first`, `data` (new data),
  `report_time` (only AEMO's report time changed) or `reverted` (an
  earlier version's data again). With `--mirror` (else
  `PHASOR_MIRROR_HOME`) it also counts how often the file was listed again
  with the same bytes (`relisted`) or as a zip of the same entries
  (`repacked`).

## models

`models` records the data-model releases a month's directory carries
under `evidence/models/{market}/`, and each release's loader configuration
(`evidence/loads/{market}/{version}.toml`: which version of which pair
loads which table, and how). `models --market gas --dir
…/MMSDataModelReport/Gas` reads AEMO's Gas Data Model Report (a web
report, not a create script) into the same model files.

## fold

`fold` reads a market's evidence in the market's clock (+10:00 for the NEM
and gas) and keeps every history to the one the market's directory last
published, read back out of each table's metadata: a field keeps its id,
name and type, a schema its id, and every file its schema, however much
evidence is added later. Data a field's type cannot hold gets a new field,
recorded as breaking. The fold notes every run AEMO's loader puts into
another table or does not load; nothing routes by it.

Once, `fold --market M --rebaseline REASON` keeps nothing and begins the
market's next data version, recorded in
`evidence/histories/{market}/BASELINE.toml` and `manifest.json`.

## export, iceberg

`export` writes each table's metadata and everything derived from it under
`{market}/`, and `manifest.json`. `iceberg` writes the same tables at file
locations under `target/iceberg/`, for readers that cannot fetch over
HTTP.
