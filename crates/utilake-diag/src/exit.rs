//! The exit codes every binary of the family returns.
//!
//! A gate (`xtask`) keeps to the first three: it passed, it failed, or it
//! could not run.

/// Everything asked for was done.
pub const OK: u8 = 0;
/// A run failed or an error occurred: nothing succeeded, or the run could
/// not start.
pub const FAILURE: u8 = 1;
/// The command line was not understood; what clap returns.
pub const USAGE: u8 = 2;
/// Some of the work failed and the rest was done; or, for a check, running
/// it for real would change something.
pub const PARTIAL: u8 = 3;
/// Offline, and every failure was a file that was not already at hand
/// (`net.offline`).
pub const OFFLINE: u8 = 4;
/// A schema could not be resolved in strict mode, or drifted.
pub const SCHEMA: u8 = 5;

#[cfg(test)]
mod tests {
    #[test]
    fn every_code_is_distinct() {
        let mut all = [
            super::OK,
            super::FAILURE,
            super::USAGE,
            super::PARTIAL,
            super::OFFLINE,
            super::SCHEMA,
        ];
        all.sort_unstable();
        assert_eq!(all, [0, 1, 2, 3, 4, 5]);
    }
}
