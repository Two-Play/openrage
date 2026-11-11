//! Reader for RAGE RPF archives.
//!
//! Currently supports RPF7 (GTA V Legacy and Enhanced), including archives
//! nested inside other archives. Other RPF versions (GTA IV: RPF3, RDR2: RPF8)
//! will be added as separate modules behind the same API.

pub mod editor;
mod error;
mod rpf7;
pub mod rpf8;
pub mod rpf8_exe;
pub mod rpf8_keys;
mod toc;
pub mod vfs;
pub mod writer;

pub use editor::{ArchiveEditor, Backup, CommitStats};
pub use error::RpfError;
pub use rpf7::{Encryption, Entry, EntryId, EntryKind, Rpf7};
pub use rpf8::Rpf8;
pub use writer::{ArchiveWriter, RawEntry, entry_for, file_entry, resource_entry};
