+++
title = "Getting started"
description = "Switch from OpenIV in minutes: download OpenRAGE for Linux or Windows and set up GTA V, GTA IV or Red Dead Redemption 2 with keys from your own copy."

[extra]
docs = true
lead = "Download OpenRAGE and set up your game in a few minutes."
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

## Set up your game

### GTA V

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

### GTA IV

Choose the GTA IV folder in the same setup dialog; the key is taken from your `GTAIV.exe` in under a second.
More on the [GTA IV page](@/gta-iv.md).

### Red Dead Redemption 2

RDR2's keys come from your running game and are set up on the command line, in three steps.
See [Red Dead Redemption 2](@/rdr2.md#setup).

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

## Next steps

| I want to … | Read |
|---|---|
| find my way around and search the game | [Explorer and search](@/guide.md) |
| see what each file format shows | [Viewing files](@/viewers.md) |
| change files and install `.oiv` packages | [Modding](@/modding.md) |
| use a terminal or scripts | [Command line](@/cli.md) |
| build OpenRAGE myself | [Build from source](@/development.md#build-from-source) |

Found a bug or missing a format? [Open an issue](https://github.com/Two-Play/openrage/issues).
