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
> The plugin API is at version **0.3** and may still change: plugins name the version they were built for and
> OpenRAGE only loads plugins for its own version (older plugins need `api = "0.3"` and a rebuild with the new SDK).
> Plugins work in the explorer (file list, preview pane, context menu, **Settings → Plugins**) and from the
> [command line](@/cli.md#plugins). Plugins are shared as packages (`.orp`). Next: openFormats export and importers
> into game formats.

## What plugins can do

| | API 0.3 |
|---|---|
| **Preview** files of the types they register: text, a tree, a table, an image or a **3D model** | ✅ |
| **Models of the game** for exporters: meshes, materials and textures of `.ydr`/`.yft`/`.ydd` (all games) | ✅ (`load-model`, `load-texture`) |
| **Actions** on the files the user picks (export, convert, reports), producing files the user saves | ✅ |
| **Batch** actions on whole folders and archives, with progress and cancel | ✅ (`batch = true`) |
| **Ask for files** from disk for an action, e.g. a patch to apply | ✅ (`pick`) |
| **Read** any file of the game (archives included, with the changes in `mods/`) | ✅ with the `read-game` permission |
| **Metadata as XML** (`.ymt`, `.ymap`, `.ytyp`, `.meta`, PSO, RBF): no binary parsing in the plugin | ✅ (`load-xml`, read only) |
| **Change** game files: into the `mods` folder, as one step the user can undo | ✅ with `write-mods`, in `modify` actions |
| openFormats export, importers into game formats | planned |

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

Install it by putting the manifest and the built `.wasm` into a folder of its own in a plugin folder, then turn it
on in **Settings → Plugins** (after **Reload plugins**) or from the command line. New plugins are off until the user
enables them:

```sh
mkdir -p ~/.config/openrage/plugins/strings
cp plugin.toml ../../target/wasm32-wasip2/release/strings.wasm ~/.config/openrage/plugins/strings/
openrage plugin list
openrage plugin enable strings
openrage plugin preview --file some.bin
openrage plugin run strings export x64a.rpf/data/x.dat --out exported/
```

## Sharing your plugin

Pack it into a plugin package (`.orp`), a ZIP file with `plugin.toml`, the component and the files you ship with it
(`README*`, `LICENSE*`, `CHANGELOG*`, `NOTICE*`, `COPYING*` from the plugin folder):

```sh
openrage plugin pack plugins/examples/strings --wasm plugins/target/wasm32-wasip2/release/strings.wasm
# Packed Strings 0.1.0 into strings-0.1.0.orp (…)
# SHA-256: …
```

Packing is reproducible (the same files give the same package) and checks the package the way installing does.
Publish the `.orp` together with its SHA-256: OpenRAGE shows the checksum before installing, so users can compare
it with yours. There are no signatures yet.

Users install it with **Settings → Plugins → Install plugin…**, by opening the `.orp` with `openrage-gui`, or with
`openrage plugin install`. Before anything is written they see the name, version, authors, licence, games,
permissions, what it adds, which installed version it replaces and the checksum. The package goes into the user's
plugin folder. A new plugin stays off until the user turns it on; an update keeps the user's choice, unless it asks
for more permissions. **Uninstall** (or `openrage plugin uninstall`) removes it again.

Packages are checked before installing: at most 1000 files and 256 MB unpacked, no paths outside the plugin folder,
a valid `plugin.toml` for this OpenRAGE, and a component that exports the plugin interface and imports nothing
OpenRAGE does not offer. An installation is written in full first and swapped in at the end; if it fails, the
installed version stays as it was.

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
api = "0.3"                    # the plugin API it was built for
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
kind = "export"                # export, tool or modify (changes game files)
extensions = ["*"]
batch = true                   # also on folders and archives: once per matching file inside
pick = ["oxt", "txt"]          # ask the user for files from disk (handed over with picked = true)
pick-title = "Choose a patch"  # title of that file dialog
```

Unknown keys, permissions or categories are errors, so typos show up in `openrage plugin list` right away.

## Permissions

| Permission | Allows |
|---|---|
| *(none)* | looking at the files the user picks for a preview or an action |
| `read-game` | `read-file` and `list-dir` on every file of the game |
| `write-mods` | `write-mod-file` in actions of kind `modify`: changes go into the `mods` folder |

The user grants the permissions when enabling a plugin. If a new version asks for more, it stays off until the user
enables it again (`needs approval` in `openrage plugin list`).

## The API

The interface is described in [WIT](https://component-model.bytecodealliance.org/design/wit.html):
[`crates/openrage-plugins/wit/plugin.wit`](https://github.com/Two-Play/openrage/blob/main/crates/openrage-plugins/wit/plugin.wit).

A plugin exports:

- `preview(vpath, data) -> result<preview, string>`: info lines and an optional view: `text`, `tree` (nodes listed
  parents first, each with the index of its parent), `table` (columns and rows), `image` (8-bit RGBA, rows from the
  top) or `model` (shown in the 3D view, exportable as glTF there).
- `run-action(action, inputs) -> result<list<output-file>, string>`: each input has its `vpath` (its path in the game,
  or its file name for files from disk), its data and `picked` (true for files the user chose because of `pick`).
  Output names may contain `/` for sub-folders but never `..`.

OpenRAGE offers:

- `current-game()`: `gta5`, `rdr2`, `gta4` or `other`.
- `read-file(vpath)` and `list-dir(vpath)`: paths relative to the game folder, through archives
  (`x64a.rpf/levels/gta5/x.ytd`), with the changes in `mods/`. Absolute paths and `..` are refused. The files the
  user handed to a call can always be read; everything else needs `read-game`.
- `load-model(vpath)`: a model as the explorer shows it (`.ydr`, `.yft`, `.ydd` of every game, also GTA IV's `.wdr`,
  `.wdd`, `.wft`): the parts shown by default (a vehicle's wheels included), placed in model space. Each mesh has
  positions, normals, texture coordinates, triangles and a material; materials have a name, the name of their colour
  texture and a colour. Same permission rule as `read-file`.
- `load-texture(vpath, name)`: a texture as RGBA pixels: from a texture dictionary (`.ytd`, `.wtd`), or a texture of a
  model (embedded, or in the dictionaries next to it that the explorer uses, such as `vehshare.ytd`).
- `load-xml(vpath)`: a metadata file (`.ymt`, `.ymap`, `.ytyp`, `.meta`, PSO, RBF and the other files the explorer
  shows as XML) as that XML text, so plugins need not parse the binary formats. Writing XML back into a game file is
  not supported yet. Same permission rule as `read-file`. Added to API 0.3 after its first release: OpenRAGE versions
  without it refuse plugins that use it.
- `build-model(template, model)`: a game model file built from `model` on top of the model at `template` (so far a
  GTA V Enhanced `.ydr`): the template keeps its settings, OpenRAGE writes the meshes (normals if missing, tangents,
  bounds). Every material gets its own copy of a template shader: in order, or shader `n` for a material named `#n`.
  A material with `image` gets it embedded as its colour texture (named after `texture`, else the material). Meshes
  whose part ends in `_lod1`, `_lod2` or `_lod3` form further detail levels. Returns the file's bytes; pass them to
  `write-mod-file` in a `modify` action. Same permission rule as `read-file`. Added to API 0.3 after its first
  release: OpenRAGE versions without it refuse plugins that use it.
- `log(level, message)`: shown by `openrage plugin`, and in the explorer's info panel next to a preview (the last
  lines) or in the status bar when an action fails. Whatever the plugin prints to stdout
  or stderr ends up there too, panics included.
- `write-mod-file(vpath, data)`: changes a game file, e.g. `update/update.rpf/x64/data/lang/american_rel.rpf/global.gxt2`.
  Only in `modify` actions and with `write-mods`. The changes are collected and applied when the action has
  succeeded (a failing action changes nothing): files in archives through OpenRAGE's archive editor (the archive is
  copied into `mods` first), loose files into `mods/`. The game's own files are never touched. Everything one run
  changed is one entry in History and can be undone at once. Loose files that could run code (`.asi`, `.dll`, `.exe`,
  scripts) are refused, and one run may change at most 1 GB.

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

## How the explorer uses a plugin

- **File types** (`[[file-types]]`) name and give an icon to extensions OpenRAGE does not know itself.
- **Previews** (`[[preview]]`) are used for files OpenRAGE has no viewer for. A plugin naming the extension wins over
  one registered for `*`; `*` plugins are only asked about binary files. Files larger than 256 MB are not handed to
  plugins for a preview. Trees and tables are shown as indented text and aligned columns for now.
- **Actions** (`[[actions]]`) appear in the context menu of matching files; `batch` actions also on folders and
  archives, where they run once per matching file inside (progress in the status bar, **Cancel** stops and keeps
  nothing). One output file is saved with a save dialog, several into a folder the user picks; batch outputs keep
  the folder structure. `pick` actions first ask for the files.
- **Modify** actions only run while editing is unlocked (the **Read-only** button), like every change in OpenRAGE.

## Models and coordinates

Models are in metres, **Z up**, with DirectX texture coordinates (**v grows downwards**), as in the game. Formats with
Y up (OBJ, glTF) need `(x, y, z) → (x, z, −y)` and `v → 1 − v` on the way out, and the reverse on the way in.

## Example: models in and out

[`plugins/examples/obj-tools`](https://github.com/Two-Play/openrage/tree/main/plugins/examples/obj-tools):

- **Export as OBJ…** on `.ydr`, `.yft`, `.ydd`, `.wdr`, `.wdd`, `.wft` (also on whole folders): writes `name.obj`,
  `name.mtl` and one PNG per texture. It only calls `load-model` and `load-texture`, so it works for every game
  OpenRAGE reads.
- **Preview** of `.obj` files in the 3D view: the plugin returns a `model` view.
- **Replace with OBJ…** on a GTA V Enhanced `.ydr` (a `modify` action with `pick = ["obj"]`): reads the chosen OBJ,
  calls `build-model` with the drawable as the template and writes the result with `write-mod-file`. This is why it
  asks for `write-mods`.

```rust
let model = load_model(vpath)?;                       // the file the user right-clicked
for m in &model.materials {
    let pixels = load_texture(vpath, &m.texture)?;    // RGBA
    // … write PNG …
}
```

## Example: changing game files

[`plugins/examples/gxt2-patch`](https://github.com/Two-Play/openrage/tree/main/plugins/examples/gxt2-patch) applies an
OpenIV text file (`.oxt`, lines like `LABEL = text`) to a GTA V text table: right-click a `.gxt2`, **Apply text
patch…**, choose the `.oxt`. Its lines replace or add entries; the patched table goes into the `mods` folder. The
important parts:

```toml
permissions = ["write-mods"]

[[actions]]
id = "apply"
title = "Apply text patch…"
kind = "modify"
extensions = ["gxt2"]
pick = ["oxt", "txt"]
```

```rust
let patch = inputs.iter().filter(|i| i.picked);      // the .oxt the user chose
for table in inputs.iter().filter(|i| !i.picked) {   // the .gxt2 right-clicked
    // … merge …
    write_mod_file(&table.vpath, &patched)?;
}
```

From the command line: `openrage plugin run gxt2-patch apply <path>.gxt2 --pick patch.oxt`.

## Tips

- `OPENRAGE_PLUGIN_PATH=path/to/folder openrage plugin list` tries a plugin without installing it.
- `--file` previews or converts files on disk, without a game.
- Start with an action that writes text: it is the fastest way to see what your plugin finds.
