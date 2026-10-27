# The data

The files under `evidence/`, `sources/`, `nem/` and `testdata/`, and
`manifest.json`, are derived from information published by the Australian
Energy Market Operator (AEMO) on nemweb.com.au:

- `evidence/ctl/` holds AEMO's control files byte for byte, named by their
  SHA-256.
- `evidence/models/` records the data-model create scripts AEMO ships in
  its monthly archives.
- `evidence/census/` records, for every block of every file AEMO published,
  its header, row count, data span and a profile of each column's values.
  No row of AEMO's data is copied.
- `sources/` lists every file the evidence was read from: its URL, SHA-256,
  size and the time the server says it was published.
- `nem/` and `manifest.json` are generated from the above by `phasor-schemas
  export`.
- `testdata/` holds short excerpts of AEMO's files for the tests.

© AEMO. Used under AEMO's copyright permissions
(https://aemo.com.au/privacy-and-legal-notices/copyright-permissions),
which allow use of published information provided AEMO is attributed. This
repository is not produced or endorsed by AEMO.
