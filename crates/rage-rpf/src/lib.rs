//! Reader for RAGE RPF archives.
//!
//! Supports RPF7 (GTA V Legacy and Enhanced, including nested archives),
//! the GTA IV archives (IMG3, RPF2, RPF3) and, with the `rdr2` feature,
//! RPF8 (RDR2).
//!
//! # Licence of the `rdr2` feature
//!
//! RPF8 decryption uses `rage-tfit2`, which is licensed under the
//! AGPL-3.0-only. The feature is off by default; enabling it puts the whole
//! program under the AGPL.
//!
//! # Process-wide defaults
//!
//! [`vfs::resolve`] opens archives by itself, so it takes their keys from
//! registries set once at start-up: [`gta4::set_default_key`] (and
//! `rpf8::set_defaults` with `rdr2`). Programs that open archives directly
//! can pass keys explicitly instead ([`Gta4Archive::open`],
//! `Rpf8::open_with_keys`) and need not touch the registries.
#![forbid(unsafe_code)]

pub mod editor;
mod error;
pub mod gta4;
pub mod gta4_editor;
pub mod gta4_rpf_editor;
mod rpf7;
#[cfg(feature = "rdr2")]
pub mod rpf8;
#[cfg(feature = "rdr2")]
pub mod rpf8_exe;
#[cfg(feature = "rdr2")]
pub mod rpf8_keys;
#[cfg(feature = "rdr2")]
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

/// The little-endian u32 at `at`; 0 past the end. Callers check the table
/// sizes first, so a short read only happens on a bug, never a panic.
pub(crate) fn word(b: &[u8], at: usize) -> u32 {
    use rage_core::bytes::LeRead;
    b.u32_at(at).unwrap_or(0)
}

pub use editor::{ArchiveEditor, Backup, CommitStats};
pub use error::RpfError;
pub use gta4::{Gta4Archive, Gta4Entry, Gta4EntryId, Gta4EntryKind, Gta4Format};
pub use rpf7::{Encryption, Entry, EntryId, EntryKind, Rpf7};
#[cfg(feature = "rdr2")]
pub use rpf8::Rpf8;
pub use writer::{ArchiveWriter, RawEntry, entry_for, file_entry, resource_entry};
