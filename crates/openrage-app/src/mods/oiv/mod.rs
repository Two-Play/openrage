//! OpenIV packages (`.oiv`): reading, previewing, installing into the mods
//! folder and uninstalling.

pub mod install;
pub mod package;
pub mod text;
pub mod xml;

pub use package::{Command, Metadata, Package};
