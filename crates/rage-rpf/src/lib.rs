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

/// How deep archives may be nested in each other. The game nests at most a
/// few levels; a crafted archive that contains itself would recurse forever.
pub const MAX_NESTING: u8 = 16;

/// Most bytes one entry may unpack to. The largest files of the games are a
/// few hundred MB; the limit keeps a crafted entry from exhausting memory.
pub const MAX_UNPACKED: u64 = 2 << 30;

/// Most bytes reserved up front for an entry whose size comes from the archive.
pub(crate) const PREALLOCATE: u64 = 64 << 20;

pub use editor::{ArchiveEditor, Backup, CommitStats};
pub use error::RpfError;
pub use gta4::{Gta4Archive, Gta4Entry, Gta4EntryId, Gta4EntryKind, Gta4Format};
pub use rpf7::{Encryption, Entry, EntryId, EntryKind, Rpf7};
pub use rpf8::Rpf8;
pub use writer::{ArchiveWriter, RawEntry, entry_for, file_entry, resource_entry};
