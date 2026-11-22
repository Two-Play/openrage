//! One module per group of commands; `main.rs` dispatches to them.

mod archive;
mod asi;
mod edit;
mod oiv;
mod openformats;
mod plugin;
mod rdr2;
mod search;
mod verify;
mod verify_gta4;
mod verify_rdr2;
mod view;

pub(crate) use archive::*;
pub(crate) use asi::*;
pub(crate) use edit::*;
pub(crate) use oiv::*;
pub(crate) use openformats::*;
pub(crate) use plugin::*;
pub(crate) use rdr2::*;
pub(crate) use search::*;
pub(crate) use verify::*;
pub(crate) use verify_gta4::*;
pub(crate) use verify_rdr2::*;
pub(crate) use view::*;
