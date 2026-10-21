# nem-schemas

set shell := ["zsh", "-cu"]
set positional-arguments := true

# list recipes
default:
    @just --list

# install the git hooks (pre-commit and pre-push)
hooks:
    prek install

# every hook of both stages on every file: what CI runs
check:
    prek run --all-files
    prek run --all-files --stage pre-push

# ── style and lint ───────────────────────────────────────────────────────────

# rewrite files in place
fmt:
    cargo fmt --all

# fail on a formatting diff, without rewriting
fmt-check:
    cargo fmt --all -- --check

# clippy with warnings as errors, and the docs with broken links as errors
lint:
    cargo clippy --workspace --all-targets --locked -- -D warnings
    RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --locked --quiet

# ── tests ────────────────────────────────────────────────────────────────────

# every unit and integration test
test:
    cargo test --workspace --locked

# tests with line coverage, held to the floors in coverage.json
cover:
    @mkdir -p coverage
    cargo llvm-cov --workspace --locked --lcov --output-path coverage/rust.lcov
    cargo run -q -p xtask -- covergate coverage/rust.lcov

# fail if a coverage floor dropped against the base branch
ratchet base="origin/main":
    cargo run -q -p xtask -- ratchet --base {{ base }}

# ── census ───────────────────────────────────────────────────────────────────

# download one month of the archive into a directory, one request a second
fetch month dir:
    cargo run -q -p nem-schemas -- fetch --month {{ month }} --dir {{ dir }}

# record every block of every file of a month's directory under evidence/census/
census month dir:
    cargo run -q -p nem-schemas -- census --month {{ month }} --dir {{ dir }}

# record every folder of the report roots under evidence/census/reports/
reports dir mmsdm="":
    cargo run -q -p nem-schemas -- reports --dir {{ dir }} {{ if mmsdm == "" { "" } else { "--mmsdm " + mmsdm } }}

# record the data-model releases a month's directory carries under evidence/models/
models month dir:
    cargo run -q -p nem-schemas -- models --month {{ month }} --dir {{ dir }}

# fold every census into each table's history under evidence/histories/
fold:
    cargo run -q -p nem-schemas -- fold

# fail if folding again would change any history
fold-check:
    cargo run -q -p nem-schemas -- fold --check

# ── secrets ──────────────────────────────────────────────────────────────────

# gitleaks over the staged changes
secrets-staged:
    gitleaks git --pre-commit --staged --no-banner --redact

# gitleaks over the commits being pushed
secrets-push:
    scripts/gitleaks-push.sh

# gitleaks over the whole history
secrets:
    gitleaks git --no-banner --redact

# known vulnerabilities in the Rust crates (needs the network)
audit:
    cargo audit
