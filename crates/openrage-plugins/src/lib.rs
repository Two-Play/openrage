//! Plugins for OpenRAGE: sandboxed WebAssembly components that add file
//! previews and actions.
//!
//! A plugin is a folder with a [`Manifest`] (`plugin.toml`) and a component
//! built against `wit/plugin.wit`. [`Registry::discover`] finds them,
//! [`Host`] runs them. Plugins see only what OpenRAGE hands them: the file
//! the user picked and, with the `read-game` permission, the game's files
//! (read-only). They have no access to other files, the network or the
//! environment, and each call has a memory and a time limit.

pub mod manifest;
mod registry;
mod runtime;

pub use manifest::{API_VERSION, Action, ActionKind, GameId, Manifest, Permission};
pub use registry::{Broken, Plugin, Registry, Status, parse_grants, search_dirs, user_dir};
pub use runtime::{
    DirEntry, Game, GameFiles, Host, ImageView, Info, Input, Level, Limits, LogLine, Material, Mesh, ModWrite, Model,
    NoGame, Outcome, OutputFile, Preview, Table, TreeNode, View, level_name,
};
/// A compiled plugin (see [`Host::compile`]).
pub use wasmtime::component::Component;
