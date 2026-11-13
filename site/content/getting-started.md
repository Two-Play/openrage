+++
title = "Getting started"
description = "Switch from OpenIV in minutes: install OpenRAGE from a release or build it from source, then set up the archive keys from your own copy of GTA V or Red Dead Redemption 2."

[extra]
lead = "Install OpenRAGE, point it at your game and let it take the archive keys from your own copy."
toc = true
+++

## Download a release

Ready-made builds for **Linux** (x86-64, glibc 2.35 or newer) and **Windows** (x86-64) are on the
[releases page](https://github.com/Two-Play/openrage/releases/latest).

{{ <downloads /> }}

<div class="os-tabs">
<div class="os-panel" data-os="linux">

### Linux

Unpack the archive and start the GUI, or `openrage` for the command line. The archive also holds `openrage.desktop`
and `openrage.png` for a menu entry. Newer releases also come as an **AppImage**: make it executable and start it.

```
tar -xzf openrage-*-linux-x86_64.tar.gz
cd openrage-*-linux-x86_64
./openrage-gui
```

To check the download, put `SHA256SUMS.txt` next to it:

```
sha256sum --check --ignore-missing SHA256SUMS.txt
```

</div>
<div class="os-panel" data-os="windows">

### Windows

Unzip the archive (right-click → *Extract All…*) and start `openrage-gui.exe`; `openrage.exe` is the command line.
The program is not signed yet, so Windows SmartScreen may ask first: *More info* → *Run anyway*.

To check the download, compare its hash in PowerShell with the line in `SHA256SUMS.txt`:

```
Get-FileHash .\openrage-*-windows-x86_64.zip -Algorithm SHA256
```

</div>
</div>

## Set up GTA V

<div class="callout">

**On first start, OpenRAGE opens a setup dialog.** It finds your Steam installation and takes the archive keys from
your own copy of the game: check the game folder and click **Derive keys**. For GTA V Legacy that is all.

</div>

- **GTA V Enhanced** (`GTA5_Enhanced.exe`) no longer stores all keys in the executable. In the setup dialog, choose a
  CodeWalker key blob (`magic.dat`) you obtained yourself under *Key blob*. It is decrypted with the key from *your*
  executable and checked against `common.rpf` before it is used.
- The keys are cached once, at `~/.config/openrage/gtav.keys` (Linux) or `%APPDATA%\openrage\gtav.keys` (Windows),
  and the game folder is remembered. To add or switch games later: **File → Switch game…**

> [!WARNING]
> **Never share the key cache.** It contains keys taken from your copy of the game.

## Coming from OpenIV

Nothing to migrate: OpenRAGE works with what you already have.

- **Your `mods` folder stays as it is.** OpenRAGE reads it the way OpenIV.asi does, marks modded files, and writes
  its own edits there too. Toggle **Mods** in the toolbar to see what your mods change.
- **`.oiv` packages install as in OpenIV** (format 2.x), but you see every change before it is written, and you can
  uninstall a package later from **History**.
- **Same output:** DDS textures, WAV audio and `.oxt` game text match OpenIV's exports.
- **Same safety:** OpenRAGE starts read-only; **Read-only → Editing** unlocks changes, like OpenIV's edit mode.
- **New:** undo for every change, search across the whole game, Linux, GTA V Enhanced and RDR2.
- **Not yet:** importing models. Keep OpenIV for that; both tools share the same `mods` folder.

## Set up Red Dead Redemption 2

RDR2's keys are set up on the **command line** for now; after that you browse the game in the GUI.
Unlike GTA V's, most RDR2 archive keys are not stored in the executable on disk: they exist only while the game runs.
OpenRAGE takes them from your own game and never ships them. There are two ways:

### From a key file

```
openrage rdr2-keys --file secrets.bin "/path/to/Red Dead Redemption 2"
```

The file holds length-prefixed key blocks, such as the `secrets.bin` Swage writes.

### From a memory dump of your running game

1. Start RDR2 in story mode (not Red Dead Online) and wait for the main menu, or load a save.
2. Dump the process. On Windows: Task Manager → Details → right-click `RDR2.exe` → *Create dump file*.
   Under Proton/Linux: `gcore <pid of RDR2.exe>` (from gdb).
3. Run `openrage rdr2-keys --dump <dump file> "/path/to/Red Dead Redemption 2"`.

> [!WARNING]
> **Delete the dump afterwards.** It is several gigabytes and holds everything the game had in memory, including login data.

The keys are stored in OpenRAGE's config folder (`rdr2.keys`).

### File names

RDR2 archives store only a hash of each file name. Let OpenRAGE search the game's own files for matching names once
(about 7 minutes; the result is cached):

```
openrage rdr2-names "/path/to/Red Dead Redemption 2"
```

This finds about 44 % of all names, including nearly all models, clip sets and particle files; the rest show as
`hash/XXXXXXXX.ext`. `--import <list>` adds names from a text file (one per line).

### Browse

In the GUI, add the RDR2 folder as a game under **File → Switch game… → Add a game…**: archives open like folders, with file
details, a text preview and *Extract…* for files and whole folders. **Textures** (`.ytd`) work as in GTA V: preview, viewer and export as DDS
or PNG. On the command line:

```
openrage rpf8 "/path/to/Red Dead Redemption 2"
openrage --game "/path/to/Red Dead Redemption 2" ls levels_1.rpf/levels/rdr3
openrage textures <path to .ytd> --export out/ --png
```

Only the Steam version of 2 April 2024 is known so far; other versions report which pieces are missing.
The search index and the other format viewers (models, …) are GTA V only for now.

## Set up GTA IV

Choose the GTA IV folder in the setup (or under **File → Switch game… → Add a game…**). The Steam folder
`Grand Theft Auto IV` and the `GTAIV` folder inside it both work. OpenRAGE takes the archive key from your own `GTAIV.exe`
in under a second; nothing else is needed. On the command line:

```
openrage --game "/path/to/Grand Theft Auto IV" keys
openrage ls pc/models/cdimages
```

GTA IV is read-only for now: browse, preview and extract; view textures and models and export them (DDS/PNG, glTF); read texts and placement files. Maps follow.

## Using the GUI

- Archives, even nested ones, open like folders. Double-click a model, texture or sound to open its viewer.
- **Ctrl+F** searches names, hashes and the text of the whole game.
- OpenRAGE starts read-only. Switch **Read-only → Editing** in the toolbar to change files: changes go to OpenIV's
  `mods` folder, never into your game files, and can be undone with **Ctrl+Z** or from **History**.
- **Install .oiv…** shows what a package changes before anything is written; installed packages can be uninstalled.
- **F1** lists all keyboard shortcuts.

The [user guide](@/guide.md) describes every part of the GUI.

## Command line

For scripts, automation and everyone who prefers a terminal. `openrage` comes in the same archive as the GUI.
Paths are virtual: archives, including archives inside archives, behave like folders.

### Keys

```
openrage --game "/path/to/Grand Theft Auto V" keys
openrage keys --magic /path/to/magic.dat       # GTA V Enhanced: your own key blob
```

### Common commands

```
openrage ls x64a.rpf/data
openrage find adder.yft
openrage search 0xB779A091                     # hash lookup
openrage extract common.rpf/data/cargens.meta -o out/
openrage model …/adder.yft --export adder.glb  # glTF for Blender
openrage oiv info mod.oiv                      # what a package would change
openrage oiv install mod.oiv
openrage undo                                  # revert the newest change
```

The [command line reference](@/cli.md) lists every command.

## Build from source

For contributors and anyone who wants the newest code.
You need a recent stable [Rust toolchain](https://rustup.rs) and [Git](https://git-scm.com/downloads).
The exact Rust version is pinned in `rust-toolchain.toml`; rustup picks it up automatically.

<div class="os-tabs">
<div class="os-panel" data-os="linux">

### Linux

The GUI needs the development packages of fontconfig, xkbcommon, Wayland and ALSA:

```
# Fedora
sudo dnf install fontconfig-devel libxkbcommon-devel wayland-devel alsa-lib-devel
# Debian / Ubuntu
sudo apt install libfontconfig1-dev libxkbcommon-dev libwayland-dev libasound2-dev
```

Then clone and build:

```
git clone https://github.com/Two-Play/openrage.git
cd openrage
cargo build --release
./target/release/openrage-gui
```

For a reproducible environment there is a container image based on Fedora's toolbx image (`dev/Containerfile`);
see [Development](@/development.md#reproducible-build-environment).

</div>
<div class="os-panel" data-os="windows">

### Windows

Rust on Windows needs the [Visual Studio Build Tools](https://visualstudio.microsoft.com/visual-cpp-build-tools/)
with *Desktop development with C++*; `rustup-init.exe` offers to install them. No other libraries are needed.
Then, in PowerShell:

```
git clone https://github.com/Two-Play/openrage.git
cd openrage
cargo build --release
.\target\release\openrage-gui.exe
```

</div>
</div>

Found a bug or missing a format? [Open an issue](https://github.com/Two-Play/openrage/issues).
