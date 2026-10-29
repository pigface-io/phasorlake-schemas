# Contributing

## Commits

Commits follow [Conventional Commits](https://www.conventionalcommits.org):
`feat`, `fix`, `perf`, `refactor`, `docs`, `test`, `build`, `ci`, `chore`,
with a scope where one fits (`feat(census): …`). A breaking change says so
with `!` and a `BREAKING CHANGE:` paragraph. A release is a tag
`vX.Y.Z` on main, its notes drafted from the history by git-cliff
(`cliff.toml`).

## Gates

`just check` runs every hook of both stages, as CI does:

- rustfmt, clippy with warnings as errors, rustdoc with broken links as
  errors;
- every dependency between the workspace's crates is one `layers.json`
  declares (`normal` for normal and build edges, `dev` for dev edges; see
  `phasor_gates::layers`), and none it declares is gone;
- every diagnostic code raised in the source is in `docs/errors.md`
  (`just errors-doc` regenerates it), under a declared category;
- every error message (`#[error("…")]`, and the message of a
  `Diagnostic`) in the Rust style: a lower-case first letter, unless the
  first word is an acronym or identifier such as `HTTP` or `DuckDB`, and
  no trailing period (`phasor_gates::msg_style`);
- every test;
- line coverage held to `coverage.json`: every source file falls under a
  floor, new code at 100%. Floors only rise; lowering one needs a
  `coverage-floor:` line in a commit message saying why;
- gitleaks over the staged changes and the commits being pushed.

CI pins every action to a commit and checks every downloaded tool against
its SHA-256.
