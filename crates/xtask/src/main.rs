//! `cargo run -p xtask -- <command>`: the gates the hooks and CI run, from
//! [`phasor_gates`].
//!
//! Everything here is Rust so the repository needs no second toolchain.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use phasor_gates::errors_doc::{self, Spec};
use phasor_gates::layers::{self, Family};
use phasor_gates::{covergate, msg_style, ratchet, repo_root};

/// This repository's error catalogue: the categories every repository
/// shares, then its own (`phasor_schemas::diagnose`).
const ERRORS: Spec<'static> = Spec {
    product: "phasor-schemas",
    categories: &[
        "config",
        "net",
        "listing",
        "cache",
        "zip",
        "csv",
        "json",
        "parse",
        "schema",
        "store",
        "manifest",
        "verify",
        "query",
        "internal",
        "census",
        "catalog",
        "models",
        "history",
        "sql",
        "published",
        "export",
        "ctl",
    ],
    retired: &[],
    inherited: &[],
};

#[derive(Parser)]
#[command(name = "xtask", about = "phasor-schemas repository gates")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Hold every source file to the floor of its directory in coverage.json.
    Covergate {
        /// The lcov profile.
        lcov: PathBuf,
    },
    /// Fail if a coverage floor dropped against a git ref.
    Ratchet {
        /// The base ref.
        #[arg(long, default_value = "origin/main")]
        base: String,
    },
    /// Fail on a dependency between the workspace's crates that layers.json
    /// does not declare, or one it declares that is gone.
    Layers,
    /// Regenerate or check docs/errors.md from the diagnostic codes in the
    /// source.
    ErrorsDoc {
        /// Write the file.
        #[arg(long)]
        write: bool,
        /// Fail if the file would change.
        #[arg(long)]
        check: bool,
    },
    /// Fail on an error message not in the Rust style: lower-case first,
    /// no trailing period (msg_style.allow holds the ones not yet fixed).
    MsgStyle,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let root = repo_root(env!("CARGO_MANIFEST_DIR"));
    let result = match cli.cmd {
        Cmd::Covergate { lcov } => covergate::run(&root, &lcov),
        Cmd::Ratchet { base } => ratchet::run(&root, &base),
        Cmd::Layers => layers::run(&root, &Family::PHASOR),
        Cmd::ErrorsDoc { write, check } => errors_doc::run(&root, &ERRORS, write, check),
        Cmd::MsgStyle => msg_style::run(&root),
    };
    match result {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::from(1),
        Err(e) => {
            eprintln!("xtask: {e:#}");
            ExitCode::from(2)
        }
    }
}
