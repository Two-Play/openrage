//! Reader for RAGE RPF archives.
//!
//! Supports RPF7 (GTA V Legacy and Enhanced, including nested archives),
//! RPF8 (RDR2) and the GTA IV archives (IMG3, RPF2, RPF3).

pub mod editor;
mod error;
pub mod gta4;
pub mod gta4_editor;
pub mod gta4_rpf_editor;
mod rpf7;
pub mod rpf8;
pub mod rpf8_exe;
pub mod rpf8_keys;
pub mod rpf8_names;
mod toc;
pub mod vfs;
pub mod writer;

pub use editor::{ArchiveEditor, Backup, CommitStats};
pub use error::RpfError;
pub use gta4::{Gta4Archive, Gta4Entry, Gta4EntryId, Gta4EntryKind, Gta4Format};
pub use rpf7::{Encryption, Entry, EntryId, EntryKind, Rpf7};
pub use rpf8::Rpf8;
pub use writer::{ArchiveWriter, RawEntry, entry_for, file_entry, resource_entry};
