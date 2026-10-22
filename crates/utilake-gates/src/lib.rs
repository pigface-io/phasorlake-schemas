//! The gates every utilake repository runs from its `xtask`.
//!
//! Each gate is a function of the repository root and its inputs, so a
//! repository's `crates/xtask/src/main.rs` is a clap enum that calls them;
//! what differs per repository (its benchmark targets, its retired codes,
//! its product name) is an argument, not a copy.
//!
//! - [`covergate`]: every source file held to the floor of its directory.
//! - [`ratchet`]: budgets only tighten and floors only rise.
//! - [`llm_check`]: the agent index stays small and every pointer in it is true.
//! - [`mod@bench`]: criterion medians held to `bench/budgets.json`.
//! - [`errors_doc`]: `docs/errors.md` lists every diagnostic code.

pub mod bench;
pub mod covergate;
pub mod errors_doc;
pub mod llm_check;
pub mod ratchet;

use std::path::PathBuf;

/// The repository root of an `xtask` crate at `crates/xtask`: the parent
/// of `crates/`.
///
/// # Panics
///
/// When `manifest_dir/../..` does not exist.
#[must_use]
pub fn repo_root(manifest_dir: &str) -> PathBuf {
    PathBuf::from(manifest_dir)
        .join("../..")
        .canonicalize()
        .expect("the workspace root exists")
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_root_is_two_up() {
        let root = super::repo_root(env!("CARGO_MANIFEST_DIR"));
        assert!(root.join("Cargo.toml").is_file());
        assert!(root.join("crates").is_dir());
    }
}
