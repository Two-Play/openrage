# OpenRAGE

An open-source explorer and modding toolkit for games built on Rockstar's RAGE engine, for **Linux and Windows**.

> OpenRAGE is an independent community project. It is not affiliated with or endorsed by Rockstar Games or Take-Two Interactive. It contains **no game files and no encryption keys**. Keys are taken from your own copy of the game at runtime.

## Status

| Game | Archives | Formats |
|---|---|---|
| GTA V (Legacy & Enhanced) | ✅ RPF7 read, incl. nested archives | resources are extracted as RSC7 files |
| GTA IV | planned (RPF3) | – |
| Red Dead Redemption 2 | planned (RPF8) | – |

Roadmap: **1** archive reading + CLI (done) → **2** Slint GUI explorer (done: explorer, search, mods overlay) → **3** textures (YTD) → **4** writing/replacing archive contents → **5** models (YDR/YFT → glTF) → **6** GTA IV → **7** RDR2.

## Build

Requires a recent stable Rust toolchain.

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

On first start a setup dialog detects Steam installations and derives the keys. The explorer shows a folder tree in which archives (including nested ones) behave like folders, a sortable file list with type badges, and a preview pane (text with a fixed-width font, hex dump for binaries and resource pages, resource version and page sizes). Extraction runs in the background and can be cancelled; its output matches OpenIV's.

### Search

OpenRAGE keeps a search index of the whole game (built once in about 30 seconds, then updated incrementally — only archives that changed after a game update are re-read). The search field accepts:

* **names** — any part of a file or folder name, exact matches first;
* **hashes** — `0xB779A091`, `B779A091`, `3078201489` or `-1216765807` resolve to `adder` via a dictionary built from all names and identifiers in the game's data, and find files with that name hash and game text with that label hash;
* **text** — words in XML/META/DAT/UGC files, audio name tables and the game's GXT2 text tables (prefix matching, all words must occur). Game text is filtered to one language, by default your system language.

Typing `?query` into the address bar searches as well; Back returns to the results.

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
| `rage-mods` | OpenIV `mods` folder: overlay resolution, folder comparison, change lists |
| `openrage-app` | settings, key setup, game detection and extraction shared by CLI and GUI |
| `openrage-cli` | the `openrage` command-line tool |
| `openrage-gui` | the Slint explorer (`openrage-gui`) |

The UI language is English; the GUI (stage 2) is built with Slint's translation support so further languages can be added.

## License

GPL-3.0-only. See [LICENSE](LICENSE) and [NOTICE.md](NOTICE.md) for third-party attributions.
