//! Write [OpenRAGE](https://github.com/Two-Play/openrage) plugins in Rust.
//!
//! ```ignore
//! use openrage_plugin_sdk::prelude::*;
//!
//! struct Hello;
//!
//! impl Plugin for Hello {
//!     fn preview(vpath: String, data: Vec<u8>) -> Result<Preview, String> {
//!         Ok(Preview::text(format!("{vpath}: {} bytes", data.len())))
//!     }
//!
//!     fn run_action(action: String, inputs: Vec<Input>) -> Result<Vec<OutputFile>, String> {
//!         Err(format!("unknown action {action}"))
//!     }
//! }
//!
//! openrage_plugin_sdk::export_plugin!(Hello);
//! ```
//!
//! Build with `cargo build --target wasm32-wasip2 --release` and put the
//! `.wasm` next to a `plugin.toml` (see the plugin author guide).

#![allow(clippy::too_many_arguments)]

/// The generated bindings of `plugin.wit`.
#[doc(hidden)]
pub mod bindings {
    wit_bindgen::generate!({
        path: "../../crates/openrage-plugins/wit",
        world: "plugin",
        pub_export_macro: true,
        export_macro_name: "export_plugin",
        default_bindings_module: "openrage_plugin_sdk::bindings",
    });
}

/// Exports a type implementing [`Plugin`] as the plugin's entry points.
pub use bindings::export_plugin;
pub use bindings::openrage::plugin::host;
pub use bindings::openrage::plugin::types::{
    DirEntry, Game, Image, Info, Input, Level, OutputFile, Preview, Table, TreeNode, View,
};

/// The trait a plugin implements (the world's exports).
pub use bindings::Guest as Plugin;

/// Everything a plugin usually needs.
pub mod prelude {
    pub use crate::host::{current_game, list_dir, load_xml, read_file, write_mod_file};
    pub use crate::{
        DirEntry, Game, Image, Info, Input, Level, OutputFile, Plugin, Preview, Table, TreeNode, View, log,
    };
}

/// Writes a line to OpenRAGE's log.
pub fn log(level: Level, message: impl AsRef<str>) {
    host::log(level, message.as_ref());
}

impl Info {
    pub fn new(key: impl Into<String>, value: impl ToString) -> Self {
        Info { key: key.into(), value: value.to_string() }
    }
}

impl Preview {
    /// A preview with only info lines.
    pub fn info(info: Vec<Info>) -> Self {
        Preview { info, view: None }
    }

    pub fn text(text: impl Into<String>) -> Self {
        Preview { info: Vec::new(), view: Some(View::Text(text.into())) }
    }

    pub fn tree(nodes: Vec<TreeNode>) -> Self {
        Preview { info: Vec::new(), view: Some(View::Tree(nodes)) }
    }

    pub fn table(columns: Vec<String>, rows: Vec<Vec<String>>) -> Self {
        Preview { info: Vec::new(), view: Some(View::Table(Table { columns, rows })) }
    }

    /// An RGBA image (`width * height * 4` bytes, rows from the top).
    pub fn image(width: u32, height: u32, rgba: Vec<u8>) -> Self {
        Preview { info: Vec::new(), view: Some(View::Image(Image { width, height, rgba })) }
    }

    /// Adds an info line.
    pub fn with(mut self, key: impl Into<String>, value: impl ToString) -> Self {
        self.info.push(Info::new(key, value));
        self
    }
}

impl TreeNode {
    pub fn new(parent: Option<u32>, label: impl Into<String>, value: impl Into<String>) -> Self {
        TreeNode { parent, label: label.into(), value: value.into() }
    }
}

impl OutputFile {
    pub fn new(name: impl Into<String>, data: impl Into<Vec<u8>>) -> Self {
        OutputFile { name: name.into(), data: data.into() }
    }
}

/// The file name of a virtual path (`a/b/c.ydr` → `c.ydr`).
pub fn file_name(vpath: &str) -> &str {
    vpath.rsplit(['/', '\\']).next().unwrap_or(vpath)
}
