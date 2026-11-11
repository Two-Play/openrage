# OpenRAGE

An open-source explorer and modding toolkit for games built on Rockstar's RAGE engine, for **Linux and Windows**.

> OpenRAGE is an independent community project. It is not affiliated with or endorsed by Rockstar Games or Take-Two Interactive. It contains **no game files and no encryption keys**. Keys are taken from your own copy of the game at runtime.

## Status

| Game | Archives | Formats |
|---|---|---|
| GTA V (Legacy & Enhanced) | ✅ RPF7 read, incl. nested archives; edit in place in the mods folder, with undo | ✅ textures (YTD): view, DDS/PNG export, replace/add/remove · models (YDR/YFT/YDD): 3D preview, glTF export · resources as RSC7 files |
| GTA IV | planned (RPF3) | – |
| Red Dead Redemption 2 | planned (RPF8) | – |

Roadmap: **1** archive reading + CLI (done) → **2** Slint GUI explorer (done: explorer, search, mods overlay) → **3** textures (YTD) (done) → **4** editing archives and `.oiv` packages (done) → **5** models (YDR/YFT → glTF, complete vehicles and peds, GPU viewer) (done) → audio (`.awc` playback, WAV export) (done) → animations → **6** GTA IV → **7** RDR2.

## Build

Requires a recent stable Rust toolchain. On Linux the GUI also needs the development packages of fontconfig, xkbcommon, Wayland and ALSA (audio output), e.g. `sudo dnf install fontconfig-devel libxkbcommon-devel wayland-devel alsa-lib-devel` or `sudo apt install libfontconfig1-dev libxkbcommon-dev libwayland-dev libasound2-dev`.

```sh
cargo build --release
# binary: target/release/openrage
```

## Getting started (GTA V)

OpenRAGE needs the archive keys from your own game installation. They are extracted once and cached at `~/.config/openrage/gtav.keys` (Linux) or `%APPDATA%\openrage\gtav.keys` (Windows). Never share that file.

```sh
openrage --game "/path/to/Grand Theft Auto V" keys
```

* **GTA V Legacy** (`GTA5.exe`): all keys are found in the executable.
* **GTA V Enhanced** (`GTA5_Enhanced.exe`): the executable only contains the AES key; the NG keys are no longer stored in plain form. Pass a CodeWalker key blob you obtained yourself: `openrage keys --magic /path/to/magic.dat`. The blob is decrypted with the AES key from *your* executable and verified against `common.rpf` before it is cached.

The game directory is remembered after this step.

## GUI

```sh
cargo run --release -p openrage-gui            # or: openrage-gui <virtual path>
```

On first start a setup dialog detects Steam installations and derives the keys. The explorer shows a folder tree in which archives (including nested ones) behave like folders, a sortable file list with format icons (textures, models, maps, data, audio, scripts …), and a preview pane (text with a fixed-width font, hex dump for binaries and resource pages, resource version and page sizes). Extraction runs in the background and can be cancelled; its output matches OpenIV's.

* **Menus and context menus:** every action is in the menu bar (File, Edit, View, Go, Help) with its keyboard shortcut; right-click a file for the actions that apply to it (open, view, extract, export, replace, rename, delete, copy name/path).
* **Double-click** opens folders, archives and texture dictionaries; models and textures open in their own **viewer window** (also Ctrl+Enter) with a details panel. Several viewers can be open at once, e.g. to compare models.
* **Address bar** with clickable breadcrumbs like a file manager; click the free space (or Ctrl+L) to type a path.
* The info panel shows metadata as a table (type, size, version, format, mip levels, meshes, triangles …).
* **Keyboard:** arrow keys, Page Up/Down, Home/End and Enter in the list; Backspace or Alt+Up goes up; Ctrl+L address, Ctrl+F search, F2 rename, Delete, Ctrl+Z undo. Help → Keyboard shortcuts (F1) lists them all.
* **Edit mode, like OpenIV:** OpenRAGE starts read-only. **Read-only → Editing** in the toolbar (or Edit → Unlock editing…) asks once per session; only then can archives, textures and packages be changed. The bar above the list says whether you are looking at an original game archive (changes are copied to `mods`) or the copy in `mods`.
* **Light and dark mode:** follows the system, or pick one under View → Theme (the sun/moon button switches quickly). The choice is remembered.

### Search

OpenRAGE keeps a search index of the whole game (built once in about 30 seconds, then updated incrementally — only archives that changed after a game update are re-read). The search field accepts:

* **names** — any part of a file or folder name, exact matches first;
* **hashes** — `0xB779A091`, `B779A091`, `3078201489` or `-1216765807` resolve to `adder` via a dictionary built from all names and identifiers in the game's data, and find files with that name hash and game text with that label hash;
* **text** — words in XML/META/DAT/UGC files, audio name tables and the game's GXT2 text tables (prefix matching, all words must occur). Game text is filtered to one language, by default your system language.

Typing `?query` into the address bar searches as well; Back returns to the results.

### Textures

Texture dictionaries (`.ytd`) open like folders: double-click one (or type its path) to list its textures with size, format and mip levels. Selecting a texture shows it in the preview; selecting a `.ytd` previews its first texture. **Extract…** writes DDS files exactly like OpenIV/CodeWalker (classic DXT/ATI FourCC headers, DX10 headers for BC7 and other modern formats); **Save PNG…** writes images. With nothing selected, the whole dictionary is exported into a folder named after it.

**Replace…**, **Add…** and **Remove** change the dictionary the OpenIV way: the archive is copied to the `mods` folder (or the copy already there is used) and rewritten there — your game files are never touched. Replacing a texture with a DDS of the same format, size and mip count only overwrites its pixels; anything else rebuilds the dictionary from its original structures with a fresh page layout. Archives are streamed while they are rewritten, so even multi-gigabyte archives need little memory.

Both layouts are supported: GTA V Legacy (resource version 13, Direct3D 9 formats) and GTA V Enhanced (version 5, DXGI formats such as BC7). All 341,000+ textures of GTA V Enhanced decode.

### Models

Selecting a model (`.ydr` drawable, `.yft` fragment such as a vehicle, `.ydd` drawable dictionary) shows it in 3D: drag to rotate, scroll to zoom. Models are shown **complete**: vehicles with their wheels on every wheel bone, peds assembled from their components (head, hair, upper body, legs, accessories — the first variant of each), parts rigidly attached to bones in place. Textures are taken from the model itself, the `.ytd` next to it (`adder.ytd`, `adder+hi.ytd`) and, for vehicles, `vehshare.ytd`.

Double-click opens the model in a **viewer window** (rendered on the GPU via wgpu — Vulkan, DirectX 12, Metal or OpenGL — with a software fallback):

* **Orbit** (drag, wheel) or **Fly** (F): W A S D to move, Q/E down/up, Shift faster, drag to look around.
* **Parts:** grouped (body, wheels, ped components with their variants); show or hide groups or single parts.
* **Materials:** every shader with a readable name (primary paint, glass, tyres, interior, …); hide it or give it a colour — paint the car. Preview and glTF export follow.
* **Export glTF…** writes a binary glTF 2.0 file (`.glb`) with one node per shown part and the textures embedded, which Blender and other tools open directly (Z-up is converted to glTF's Y-up).

All 168,000+ models of GTA V Enhanced load, as do the Legacy formats. Not yet: animations and skinning poses, vehicle mods and tuning parts, map placements (ymap/ytyp).

### Audio

Audio containers (`.awc`) open like folders: effects and dialogue list their sounds, music and radio tracks show one multi-channel stream. Selecting a sound shows its waveform, length, sample rate and codec; **Play** (or double-click, or Space) plays it, clicking the waveform jumps. **Save WAV…** and **Extract…** write `.wav` files (16-bit PCM, like OpenIV).

Supported: PCM and IMA ADPCM (GTA V Legacy), MPEG audio (GTA V Enhanced), single sounds and block-based multi-channel streams, and Rockstar's audio encryption (whole files, sound data or stream blocks). The audio key comes from the key blob used during setup; key caches from older versions are upgraded automatically.


Inside an archive, **Import…** (or dropping files onto the list, where the platform supports it), **Replace…**, **Rename…**, **New folder…** and **Delete** change the game the OpenIV way: the archive is copied to the `mods` folder once and then edited **in place** — new data is appended and only the table of contents is rewritten, so a change to a 2 GB archive takes milliseconds. Changes inside nested archives rebuild just that nested archive. Your game files are never touched.

Every change is recorded in `mods/.openrage/journal` and can be reverted with **Undo**, step by step, back to the original files (OpenIV has no undo).

### OpenIV packages (`.oiv`)

**Install .oiv…** (or `openrage-gui package.oiv`) shows the package — name, author, description — and exactly what installing it would change, before anything is written. Packages of format 2.x are supported: files added to or deleted from the game and from archives (also nested ones), new archives and DLC packs (`createIfNotExist`), text edits (`add`, `insert`, `replace`, `delete` with Equal/StartWith/Mask) and XML edits by XPath (`add` First/Last/Before/After, `replace`, `remove`) that keep the file's formatting. Changes go to the `mods` folder; loose files that are new (such as `.asi` plugins) go to the game folder, as loaders expect them there.

Every installed package can be **uninstalled** from **History** (or `openrage oiv uninstall`) — OpenIV cannot do that. Not supported yet: `<pso>` edits (they need a PSO converter; such packages are refused with an explanation). `<defragmentation>` is skipped, as it only saves disk space.

### Mods folder (OpenIV compatible)

OpenRAGE understands the `mods` folder used by OpenIV.asi, where a file at `mods/<path>` replaces `<path>` in the game. It never changes that layout — it only shows what it means:

* **Mods** (toolbar toggle) shows the game the way it sees itself: replaced files are marked *mod*, files only in `mods` *new*, and folders containing mods *has mods*.
* Inside a modded archive every folder is compared with the original: entries are marked *added* (green), *changed* (orange) or *removed* (red, shown greyed out with a preview of the original).
* **Changes** lists every difference of all mods at once, across nested archives — including complete copies of game archives that contain no change at all and can simply be deleted.

## CLI usage

Paths are virtual: archives — including archives inside archives — behave like directories.

```sh
openrage ls                                    # game directory
openrage ls x64a.rpf/data
openrage ls -r update/update.rpf               # recursive, descends into nested archives
openrage info x64e.rpf/levels/gta5/vehicles.rpf
openrage find adder.yft                        # search all archives of the game
openrage extract common.rpf/data/cargens.meta -o out/
openrage extract x64a.rpf -o out/              # whole archive
openrage extract some.rpf -o out/ --expand-archives
openrage verify                                # decode every entry of every archive
openrage index                                 # build / update the search index
openrage search weapon_pistol                  # names + full text, shows matching lines
openrage search 0xB779A091                     # hash lookup
openrage search Pegassi --lang american        # game text in another language (or `all`)
openrage textures x64e.rpf/levels/gta5/vehicles.rpf/adder.ytd                 # list textures
openrage textures …/adder.ytd --export out/ [--png]                            # DDS (default) or PNG
openrage textures …/blips.ytd --set radar_darts=new.dds --remove radar_taco_van   # written into mods/
openrage put new.meta x64e.rpf/data/            # import into an archive (mods folder)
openrage mv x64e.rpf/a.xml x64e.rpf/b.xml       # rename / move inside an archive
openrage rm x64e.rpf/levels/gta5/paths.rpf       # remove a file or folder
openrage mkdir x64e.rpf/my_folder
openrage oiv info mod.oiv                      # what a package would change
openrage oiv install mod.oiv                   # install into the mods folder
openrage oiv list / oiv uninstall "Name 1.0"   # installed packages, uninstall
openrage history                               # changes made to the mods folder
openrage undo                                  # revert the newest change
openrage verify --textures [--rebuild]         # decode (and rebuild) every texture dictionary
openrage model …/vehicles.rpf/adder.yft                       # meshes, shaders, textures
openrage model …/adder.yft --export adder.glb                  # glTF with embedded textures
openrage model …/adder.yft --render adder.png [--yaw 1.2]      # software-rendered image
openrage verify --models                       # parse every model of the game
openrage audio x64/audio/sfx/RADIO_02_POP.rpf/alright.awc --export out/   # sounds → .wav
openrage verify --audio                        # decode every sound of the game
openrage mods                                  # files in the OpenIV mods folder
openrage mods --changes [--deep]               # every added/removed/modified entry
```

## Project layout

| Crate | Purpose |
|---|---|
| `rage-core` | shared primitives: hashing (joaat), positional I/O |
| `rage-crypto` | AES / NG ciphers, key discovery in the user's executable, key cache |
| `rage-rpf` | RPF7 reader and OpenIV-style writer, virtual file system over nested archives |
| `rage-index` | SQLite search index: names, hash dictionary, FTS5 full text, incremental updates |
| `rage-resource` | RSC7 resources: page layout, virtual pointers into system/graphics memory |
| `rage-formats` | game formats: texture dictionaries (YTD), BC1–BC7 decoding, DDS and PNG export |
| `rage-mods` | OpenIV `mods` folder: overlay resolution, folder comparison, change lists |
| `openrage-app` | settings, key setup, game detection and extraction shared by CLI and GUI |
| `openrage-cli` | the `openrage` command-line tool |
| `openrage-gui` | the Slint explorer (`openrage-gui`) |

The UI language is English; the GUI (stage 2) is built with Slint's translation support so further languages can be added.

## License

GPL-3.0-only. See [LICENSE](LICENSE) and [NOTICE.md](NOTICE.md) for third-party attributions.
