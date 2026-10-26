//! What an archived file is and every place AEMO served it.
//!
//! - [`stamp`]: an instant with the offset it was written in, and every
//!   text form the family reads one from. The one time type of the family.
//! - [`content`]: a file's bytes as size and digests.
//! - [`provenance`]: the record beside each held file: its content, and
//!   every address that served it, the one it was obtained from first.
//! - [`archive`]: where held files and their records are, read and written
//!   whole.
//! - [`layout`]: the rule that names a file's local path from its URL.
//! - [`civil`]: the calendar the instants are counted in.
//! - [`diagnose`]: the diagnostic of each error here, with its stable code.
//!
//! Pure but for the file system: knows no network and no decoder.

pub mod archive;
pub mod civil;
pub mod content;
pub mod diagnose;
pub mod layout;
pub mod provenance;
pub mod stamp;

pub use archive::{Archive, Held};
pub use content::{Content, Sha256};
pub use provenance::{Address, Listed, Provenance, Served, Span, Via};
pub use stamp::Stamp;
