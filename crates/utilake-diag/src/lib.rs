//! The one shape every error and warning of a utilake repository is
//! reported as.
//!
//! - [`Diagnostic`]: a stable machine-readable code, its [`Category`], a
//!   [`Severity`], a message, and every piece of context the layer that
//!   raised it knew; outer layers add theirs with [`Diagnostic::fill_from`].
//! - [`Diagnose`]: an error that describes itself as a diagnostic. Every
//!   library error enum implements it beside its own type.
//! - [`Category`]: the first segment of a code. Categories are open: the
//!   ones every repository shares are consts here, and each repository
//!   declares its own beside its errors.
//! - [`exit`]: the exit codes every binary of the family returns.
//!
//! Pure: no I/O.

pub mod category;
pub mod diag;
pub mod exit;

pub use category::Category;
pub use diag::{Code, Context, Diagnose, Diagnostic, Sample, Severity};
