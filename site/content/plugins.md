+++
title = "Writing plugins"
description = "Extend OpenRAGE with sandboxed WebAssembly plugins: file previews and export actions, written in Rust or any language that builds WebAssembly components."

[extra]
docs = true
lead = "Plugins add file previews and actions to OpenRAGE. They run in a sandbox, so users can try them without risk."
toc = true
+++

A plugin is a folder with a manifest (`plugin.toml`) and a WebAssembly component (`.wasm`). OpenRAGE runs it with
[wasmtime](https://wasmtime.dev/). The plugin sees only the files the user hands it, plus the game's files if the user
grants that (read-only). It has no access to other files, the network or the environment, and every call has a
memory and a time limit. The same `.wasm` runs on Linux and Windows.

> [!NOTE]
> The plugin API is at version **0.1** and may still change: plugins name the version they were built for and
> OpenRAGE only loads plugins for its own version. For now plugins work from the [command line](@/cli.md#plugins).
> Next: in the explorer (preview pane, context menu, settings), then writing into the `mods` folder, batch tools, 3D
> models and installable plugin packages.

## What plugins can do

| | API 0.1 |
|---|---|
| **Preview** files of the types they register: text, a tree, a table or an image | ✅ |
| **Actions** on the files the user picks (export, convert, reports), producing files the user saves | ✅ |
| **Read** any file of the game (archives included, with the changes in `mods/`) | ✅ with the `read-game` permission |
| Write into the `mods` folder, batch tools over folders, 3D previews, imports | planned |

## Quick start (Rust)

The example plugin [`plugins/examples/strings`](https://github.com/Two-Play/openrage/tree/main/plugins/examples/strings)
shows the readable text in files nothing else can display, and exports it. Copy its folder to start your own:

```sh
rustup target add wasm32-wasip2
cd plugins/examples/strings
cargo build --release --target wasm32-wasip2
```

Outside the OpenRAGE repository, depend on the SDK by path or git:

```toml
[lib]
crate-type = ["cdylib"]

[dependencies]
openrage-plugin-sdk = { git = "https://github.com/Two-Play/openrage" }
```

A plugin implements the `Plugin` trait and exports it:

```rust
use openrage_plugin_sdk::prelude::*;

struct Hello;

impl Plugin for Hello {
    fn preview(vpath: String, data: Vec<u8>) -> Result<Preview, String> {
        Ok(Preview::text(format!("{} bytes", data.len())).with("File", vpath))
    }

    fn run_action(action: String, inputs: Vec<Input>) -> Result<Vec<OutputFile>, String> {
        match action.as_str() {
            "size" => Ok(inputs
                .iter()
                .map(|i| OutputFile::new(format!("{}.txt", openrage_plugin_sdk::file_name(&i.vpath)), i.data.len().to_string()))
                .collect()),
            _ => Err(format!("unknown action {action}")),
        }
    }
}

openrage_plugin_sdk::export_plugin!(Hello);
```

Install it by putting the manifest and the built `.wasm` into a folder of its own in a plugin folder, then enable it.
New plugins are off until the user enables them:

```sh
mkdir -p ~/.config/openrage/plugins/strings
cp plugin.toml ../../target/wasm32-wasip2/release/strings.wasm ~/.config/openrage/plugins/strings/
openrage plugin list
openrage plugin enable strings
openrage plugin preview --file some.bin
openrage plugin run strings export x64a.rpf/data/x.dat --out exported/
```

## Where plugins are found

In this order (the first plugin with an id wins):

1. the folders in the `OPENRAGE_PLUGIN_PATH` environment variable (handy while developing),
2. the user's plugin folder: `~/.config/openrage/plugins` on Linux, `%APPDATA%\openrage\plugins` on Windows,
3. `plugins` next to the `openrage` executable (portable installs).

Each plugin has its own sub-folder there. `openrage plugin list` shows the folders, the plugins and the ones that
could not be read, with the reason.

## `plugin.toml`

```toml
id = "strings"                 # unique: lowercase letters, digits and '-'
name = "Strings"
version = "0.1.0"
api = "0.1"                    # the plugin API it was built for
description = "Shows the readable text in files."
authors = ["You"]
license = "MIT OR Apache-2.0"  # SPDX expression of your plugin
homepage = "https://…"
wasm = "strings.wasm"          # default: plugin.wasm
games = ["gta5", "rdr2"]       # default: every game
permissions = []               # see below

# File types it adds: their name and icon in the file list.
[[file-types]]
extensions = ["xyz"]
name = "XYZ table"
category = "data"              # file, texture, model, map, text, lang, audio, anim, script, package, data

# File types it previews. "*" makes it a fallback for files nothing else shows.
[[preview]]
extensions = ["xyz"]

# Actions on files (context menu, `openrage plugin run`).
[[actions]]
id = "export"                  # passed to run-action
title = "Export strings…"
kind = "export"                # export or tool
extensions = ["*"]
```

Unknown keys, permissions or categories are errors, so typos show up in `openrage plugin list` right away.

## Permissions

| Permission | Allows |
|---|---|
| *(none)* | looking at the files the user picks for a preview or an action |
| `read-game` | `read-file` and `list-dir` on every file of the game |

The user grants the permissions when enabling a plugin. If a new version asks for more, it stays off until the user
enables it again (`needs approval` in `openrage plugin list`).

## The API

The interface is described in [WIT](https://component-model.bytecodealliance.org/design/wit.html):
[`crates/openrage-plugins/wit/plugin.wit`](https://github.com/Two-Play/openrage/blob/main/crates/openrage-plugins/wit/plugin.wit).

A plugin exports:

- `preview(vpath, data) -> result<preview, string>`: info lines and an optional view: `text`, `tree` (nodes listed
  parents first, each with the index of its parent), `table` (columns and rows) or `image` (8-bit RGBA, rows from the
  top).
- `run-action(action, inputs) -> result<list<output-file>, string>`: each input has its `vpath` (its path in the game,
  or its file name for files from disk) and its data. Output names may contain `/` for sub-folders but never `..`.

OpenRAGE offers:

- `current-game()`: `gta5`, `rdr2`, `gta4` or `other`.
- `read-file(vpath)` and `list-dir(vpath)`: paths relative to the game folder, through archives
  (`x64a.rpf/levels/gta5/x.ytd`), with the changes in `mods/`. Absolute paths and `..` are refused.
- `log(level, message)`: shown by `openrage plugin` (and later in the explorer). Whatever the plugin prints to stdout
  or stderr ends up there too, panics included.

Each call starts a fresh instance: plugins keep no state between calls. Limits per call: 1 GiB of memory and 60
seconds. A plugin that crashes, loops or runs out of memory ends that call with an error; OpenRAGE goes on.

## Other languages

Any toolchain that builds a WebAssembly component for WASI 0.2 from `plugin.wit` works, for example
[componentize-py](https://github.com/bytecodealliance/componentize-py) (Python), [jco](https://github.com/bytecodealliance/jco)
(JavaScript), TinyGo or wit-bindgen for C. The component must export the `plugin` world. It may import the WASI
interfaces, but they give it no files, sockets, arguments or environment.

## Licences

OpenRAGE is GPL-3.0-only. The plugin interface (`plugin.wit`), the SDK (`plugins/sdk`) and the examples are
**MIT OR Apache-2.0**, so you can publish plugins under any licence. GPL-compatible licences are welcome, but not
required. Plugins are not part of OpenRAGE; their authors are responsible for them.

Never put Rockstar's files or keys into a plugin. Read what you need from the user's game at runtime, with the
`read-game` permission.

## Tips

- `OPENRAGE_PLUGIN_PATH=path/to/folder openrage plugin list` tries a plugin without installing it.
- `--file` previews or converts files on disk, without a game.
- Start with an action that writes text: it is the fastest way to see what your plugin finds.
