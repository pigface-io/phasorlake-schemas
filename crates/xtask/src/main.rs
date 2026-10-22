//! `cargo run -p xtask -- <command>`: the gates the hooks and CI run, from
//! [`utilake_gates`].
//!
//! Everything here is Rust so the repository needs no second toolchain.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use utilake_gates::{covergate, ratchet, repo_root};

#[derive(Parser)]
#[command(name = "xtask", about = "utilake-schemas repository gates")]
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
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let root = repo_root(env!("CARGO_MANIFEST_DIR"));
    let result = match cli.cmd {
        Cmd::Covergate { lcov } => covergate::run(&root, &lcov),
        Cmd::Ratchet { base } => ratchet::run(&root, &base),
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
