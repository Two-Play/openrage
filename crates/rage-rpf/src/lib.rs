//! Reader for RAGE RPF archives.
//!
//! Currently supports RPF7 (GTA V Legacy and Enhanced), including archives
//! nested inside other archives. Other RPF versions (GTA IV: RPF3, RDR2: RPF8)
//! will be added as separate modules behind the same API.

mod error;
mod rpf7;
pub mod vfs;

pub use error::RpfError;
pub use rpf7::{Encryption, Entry, EntryId, EntryKind, Rpf7};
