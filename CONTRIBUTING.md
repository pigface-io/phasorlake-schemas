# Contributing

## Commits

Commits follow [Conventional Commits](https://www.conventionalcommits.org):
`feat`, `fix`, `perf`, `refactor`, `docs`, `test`, `build`, `ci`, `chore`,
with a scope where one fits (`feat(census): …`). A breaking change says so
with `!` and a `BREAKING CHANGE:` paragraph. Release notes are generated
from the history by git-cliff (`cliff.toml`).

## Gates

`just check` runs every hook of both stages, as CI does:

- rustfmt, clippy with warnings as errors, rustdoc with broken links as
  errors;
- every test;
- line coverage held to `coverage.json`: every source file falls under a
  floor, new code at 100%. Floors only rise; lowering one needs a
  `coverage-floor:` line in a commit message saying why;
- gitleaks over the staged changes and the commits being pushed.

CI pins every action to a commit and checks every downloaded tool against
its SHA-256.
