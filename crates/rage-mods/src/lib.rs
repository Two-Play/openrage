//! Support for OpenIV's `mods` folder.
//!
//! With OpenIV.asi installed, the game loads `<game>/mods/<path>` instead of
//! `<game>/<path>` whenever the former exists. Mods therefore consist of whole
//! replaced (or new) top-level files — usually complete copies of large
//! archives in which only a few entries differ. This crate answers the
//! questions OpenIV leaves open:
//!
//! * [`overlay`]: what does the game actually see (the effective file system)?
//! * [`compare`]: which entries inside a modded archive really differ from the
//!   original, per folder (for browsing) or as a complete change list?
//!
//! Everything here is read-only; the `mods` folder format is not changed.

pub mod compare;
pub mod overlay;

pub use compare::{
    Change, ChangeKind, CompareMode, FolderDiff, Mark, RemovedEntry, compare_folders, compare_gta4_folders,
    list_changes,
};
pub use overlay::{ModFile, ModFileKind, Origin, mods_dir, resolve, scan};
