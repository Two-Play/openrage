+++
title = "Getting started"
description = "Install OpenRAGE from a release or build it from source, then set up the archive keys from your own copy of GTA V or Red Dead Redemption 2."

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

Unpack the archive and start the GUI, or `openrage` for the command line:

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

## Build from source

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
see [Reproducible build environment](https://github.com/Two-Play/openrage#reproducible-build-environment) in the README.

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

## Set up GTA V

OpenRAGE needs the archive keys from *your own* game installation. On first start the GUI shows a setup dialog that
detects Steam installations and derives the keys. On the command line:

```
openrage --game "/path/to/Grand Theft Auto V" keys
```

The keys are extracted once and cached at `~/.config/openrage/gtav.keys` (Linux) or
`%APPDATA%\openrage\gtav.keys` (Windows). The game directory is remembered after this step.

- **GTA V Legacy** (`GTA5.exe`): all keys are found in the executable.
- **GTA V Enhanced** (`GTA5_Enhanced.exe`): the executable only contains the AES key; the NG keys are no
  longer stored in plain form. Pass a CodeWalker key blob you obtained yourself:
  `openrage keys --magic /path/to/magic.dat`. The blob is decrypted with the AES key from your executable and verified
  against `common.rpf` before it is cached.

> [!WARNING]
> **Never share the key cache.** It contains keys taken from your copy of the game.

## Set up Red Dead Redemption 2

RDR2 archives can be browsed in the GUI and on the command line. Unlike GTA V's, most RDR2 archive keys are not stored in the executable on disk;
they exist only while the game runs. OpenRAGE takes them from your own game and never ships them. There are two ways:

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

In the GUI, choose the RDR2 folder as the game folder: archives open like folders, with file details, a text preview
and *Extract…* for files and whole folders. **Textures** (`.ytd`) work as in GTA V: preview, viewer and export as DDS
or PNG. On the command line:

```
openrage rpf8 "/path/to/Red Dead Redemption 2"
openrage --game "/path/to/Red Dead Redemption 2" ls levels_1.rpf/levels/rdr3
openrage textures <path to .ytd> --export out/ --png
```

Only the Steam version of 2 April 2024 is known so far; other versions report which pieces are missing.
The search index and the other format viewers (models, …) are GTA V only for now.

## Next steps

### In the GUI

- Start `openrage-gui`. On first start it finds your Steam installation and sets up the keys.
- Archives, even nested ones, open like folders. Double-click a model, texture or sound to open its viewer.
- **Ctrl+F** searches names, hashes and the text of the whole game.
- OpenRAGE starts read-only. Switch **Read-only → Editing** in the toolbar to change files: changes go to the `mods`
  folder and can be undone with **Ctrl+Z** or from **History**.
- **Install .oiv…** shows what a package changes before anything is written.
- **F1** lists all keyboard shortcuts.

### On the command line

Paths are virtual: archives, including archives inside archives, behave like folders.

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

Edits never touch your game files: they go to OpenIV's `mods` folder. The
[README](https://github.com/Two-Play/openrage#readme) documents every part of the GUI and the complete
[command line](https://github.com/Two-Play/openrage#command-line).
Found a bug or missing a format? [Open an issue](https://github.com/Two-Play/openrage/issues).
