# nem-schemas

Every table of the National Electricity Market data that AEMO publishes on
nemweb.com.au, as an [Apache Iceberg](https://iceberg.apache.org/) table:
its fields with ids that never change meaning, its schemas, and the runs
of AEMO files written under each, with every change between them. The
schemas are derived from what AEMO's files hold, file by file, and the
code that derives them is here, so anyone can check the result or improve
it.

The code is Rust (`crates/`), licensed under Apache-2.0. The data is
derived from AEMO's and is attributed to AEMO; see `LICENCE-DATA.md`.

## Working on it

Install [just](https://just.systems), [prek](https://prek.j178.dev) and
`cargo-llvm-cov`, then:

```sh
just hooks   # the git hooks
just check   # every gate, as CI runs them
```

See `CONTRIBUTING.md`.
