# utilake-schemas

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
    cargo clippy --workspace --all-targets --locked --features utilake-diag/log -- -D warnings
    RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --locked --quiet --features utilake-diag/log

# every dependency between the workspace's crates is one layers.json declares
layers:
    cargo run -q -p xtask -- layers

# docs/errors.md lists every diagnostic code; regenerate it
errors-doc:
    cargo run -q -p xtask -- errors-doc --write

# fail if docs/errors.md is out of date
errors-check:
    cargo run -q -p xtask -- errors-doc --check

# every error message lower-case first with no trailing period (msg_style.allow: not yet fixed)
msg-style:
    cargo run -q -p xtask -- msg-style

# ── tests ────────────────────────────────────────────────────────────────────

# every unit and integration test
test:
    cargo test --workspace --locked --features utilake-diag/log

# tests with line coverage, held to the floors in coverage.json
cover:
    @mkdir -p coverage
    cargo llvm-cov --workspace --locked --features utilake-diag/log --lcov --output-path coverage/rust.lcov
    cargo run -q -p xtask -- covergate coverage/rust.lcov

# fail if a coverage floor dropped against the base branch
ratchet base="origin/main":
    cargo run -q -p xtask -- ratchet --base {{ base }}

# ── census ───────────────────────────────────────────────────────────────────

# download one month of the archive into a directory, one request a second
fetch month dir:
    cargo run -q -p utilake-schemas -- fetch --month {{ month }} --dir {{ dir }}

# record every block of every file of a month's directory under evidence/census/{market}/
census month dir market="nem":
    cargo run -q -p utilake-schemas -- census --month {{ month }} --dir {{ dir }} --market {{ market }}

# record every folder of a market's feeds under evidence/census/{market}/, from the host directory of a mirror archive
reports host market="nem" feed="" mmsdm="" mirror="":
    cargo run -q -p utilake-schemas -- reports --dir {{ host }} --market {{ market }} {{ if feed == "" { "" } else { "--feed " + feed } }} {{ if mmsdm == "" { "" } else { "--mmsdm " + mmsdm } }} {{ if mirror == "" { "" } else { "--mirror " + mirror } }}

# list every file each census read in sources/{market}/, from a mirror archive's sidecars
sources archive market="nem":
    cargo run -q -p utilake-schemas -- sources --archive {{ archive }} --market {{ market }}

# fail if listing the sources again would change any file, listing each
sources-check archive market="nem":
    cargo run -q -p utilake-schemas -- sources --archive {{ archive }} --market {{ market }} --check

# record the data-model releases a month's directory carries under evidence/models/{market}/
models month dir market="nem":
    cargo run -q -p utilake-schemas -- models --month {{ month }} --dir {{ dir }} --market {{ market }}

# fold every census of a market into each table's history under evidence/histories/{market}/
fold market="nem":
    cargo run -q -p utilake-schemas -- fold --market {{ market }}

# fail if folding a market again would change any history
fold-check market="nem":
    cargo run -q -p utilake-schemas -- fold --check --market {{ market }}

# ── export ───────────────────────────────────────────────────────────────────

# write every market's directory and manifest.json from every history
export:
    cargo run -q -p utilake-schemas -- export

# fail if exporting again would change any file, listing each
export-check:
    cargo run -q -p utilake-schemas -- export --check

# fail if a market's directory breaks a promise a revision published
compat base="origin/main":
    #!/usr/bin/env zsh
    set -eu
    if git rev-parse -q --verify "{{ base }}^{commit}" >/dev/null; then
      cargo run -q -p utilake-schemas -- compat --base {{ base }}
    else
      echo "compat: {{ base }} does not exist yet; nothing was published to hold to"
    fi

# what the market directories add to a revision, per table, as Markdown
diff base="origin/main":
    cargo run -q -p utilake-schemas -- diff --base {{ base }}

# write every history as an Iceberg table without data, under {out}/{market}/
iceberg out="target/iceberg":
    cargo run -q -p utilake-schemas -- iceberg --out {{ out }}

# write every table under {dir} (`just iceberg`), then read each with pyiceberg and DuckDB, held to its history, and scan it empty
replay dir="target/iceberg": (iceberg dir)
    uv run --with pyiceberg --with pyarrow --with duckdb python scripts/replay.py {{ dir }}

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
