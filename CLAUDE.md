# phasorlake-schemas

Rust workspace: AEMO's market data as Apache Iceberg v3 table metadata,
derived from a census of AEMO's files, and the crates phasor builds on. Run
tools only through `just` (`just --list`); never `cargo test` directly, use
`just test` or `just cover`. `just check` runs every gate CI runs.

- The market directories (`nem/`, `gas/`) and `manifest.json` are written
  by `just export` from `evidence/histories/`, which `just fold` writes from
  the evidence. Never edit them by hand; commit evidence, histories and
  market directories together. `just export-check` and `just compat
  origin/main` must pass: a published field id never changes meaning.
- Everything is derived from what AEMO's files hold, checked against AEMO's
  control files and data models; every step is deterministic (no clock, no
  network after `fetch`).
- No key or temporal roles are published: those are a reader's decisions.
- Error messages lower-case first with no trailing period; every
  diagnostic code is in `docs/errors.md` (`just errors-doc`). New code is
  covered to 100% (`coverage.json`).
- Conventional Commits. Never name the project's old name (`just
  old-name`) or other AEMO tools in docs, comments or commits.
- `README.md` is the reader's contract; `docs/pipeline.md` says what each
  step reads and records.
