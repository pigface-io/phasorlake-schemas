//! Where a row sits in its file, and what a reader does after a bad one.

use std::fmt;

/// Where in the file something happened.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Pos {
    /// 1-based line.
    pub line: u64,
    /// 0-based byte offset of the line start.
    pub byte_offset: u64,
}

/// What to do after a bad row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flow {
    /// Keep scanning.
    Continue,
    /// Stop now.
    Stop,
}

/// The refusal or failure of a sink that has none.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Never {}

impl fmt::Display for Never {
    fn fmt(&self, _: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {}
    }
}

impl std::error::Error for Never {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_position_starts_at_nothing() {
        assert_eq!(
            Pos::default(),
            Pos {
                line: 0,
                byte_offset: 0
            }
        );
        assert_ne!(Flow::Continue, Flow::Stop);
    }
}
