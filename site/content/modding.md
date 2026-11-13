+++
title = "Modding"
description = "Change the game safely with OpenRAGE: edit mode, editing archives, textures and game text, undo and history, .oiv packages and OpenIV's mods folder."

[extra]
docs = true
lead = "Every change goes to OpenIV's `mods` folder, never into your game files, and every change can be undone."
toc = true
+++

## Edit mode

OpenRAGE starts read-only, like OpenIV.

- **Read-only → Editing** in the toolbar (or Edit → Unlock editing…) unlocks changes, once per session.
- The bar above the list says whether you look at an original game archive or at its copy in `mods`.
- Changing an original archive copies it to `mods` first; your game files stay untouched.

## Editing archives

Inside an archive:

- **Import…**, or drop files onto the list
- **Replace…**, **Rename…**, **New folder…** and **Delete**

### Fast, even for large archives

The archive is copied to `mods` once and then edited **in place**: new data is appended and only the table of contents
is rewritten. A change to a 2 GB archive takes milliseconds. Changes inside nested archives rebuild just that nested
archive.

## Textures

In a texture dictionary, **Replace…**, **Add…** and **Remove** change textures:

- A DDS of the same format, size and mip count only overwrites the pixels.
- Anything else rebuilds the dictionary from its original structures.
- Archives are streamed while they are rewritten, so even multi-gigabyte archives need little memory.

## Models (preview)

A drawable of GTA V Enhanced (`.ydr`) can get new geometry and textures from a Wavefront OBJ file:

- **In the explorer:** turn on the *OBJ tools* plugin (it comes with OpenRAGE, **Settings → Plugins**), unlock
  editing, right-click the `.ydr` → **Replace with OBJ…** and choose the OBJ together with its `.mtl` and PNG
  textures. The change goes into the `mods` folder and is one entry in **History**. *OBJ tools* also exports models as
  OBJ with their textures, so you can export, edit (e.g. in Blender) and bring them back.
- **From the command line:** `openrage replace-model x64i.rpf/levels/gta5/props/…/prop_x.ydr model.obj --mods`
  (it finds the `.mtl` and textures next to the OBJ), or `openrage plugin run obj-tools replace <path>.ydr --pick
  model.obj --pick model.mtl --pick texture.png …`.

How it works:

- **Shaders:** every OBJ material gets its own copy of one of the drawable's shaders (in order; `--shader N` uses
  shader N for all). The copies keep the original shader's settings and the vertex layout it expects.
- **Textures:** a material's `map_Kd` (PNG) is embedded in the drawable and bound to its copy's colour slot, with all
  mip levels; a material with only a colour (`Kd`) gets a small texture of that colour; without either it keeps the
  original texture. Models without embedded textures borrow the texture structure of another model of the game.
  `--no-textures` keeps the original textures.
- **Detail levels:** `--lod near.obj --lod far.obj` (up to three) add further levels; plugins mark them with parts
  named `…_lod1`, `…_lod2`, `…_lod3`. Distances come from the original's levels where it has them, otherwise 50 m
  doubling per level; the last level is seen as far as the original's furthest.
- OBJ is converted from Y-up to the game's Z-up; missing normals are computed, tangents always. Meshes over 65,535
  vertices are split. `--mods` puts the result into the `mods` folder in place of the original (undo with **Undo** /
  `openrage undo`), `--out` writes the file elsewhere.
- Not yet: GTA V Legacy, `.yft`/`.ydd`, skinned models and collisions (the original collision stays). Textures are
  embedded uncompressed. The written drawables read back identically in OpenRAGE (`openrage verify --models
  --rebuild` rewrites every drawable of the game, also with shader copies and embedded textures); please report how
  they behave in the game.

## Text files and game text

- Text files open in the text editor. After unlocking, **Save** (Ctrl+S) writes them into `mods`, keeping encoding,
  line endings and tabs.
- Game text (`.gxt2`) and audio name tables (`.nametable`) are edited as text and converted back when saving. The
  conversion is lossless for every text table of the game.

## Undo and history

Every change is recorded in `mods/.openrage/journal`.

- **Undo** (Ctrl+Z) reverts changes step by step, back to the original files. OpenIV has no undo.
- **History** lists every change and installed package; it can open in its own window (View menu).

## OpenIV packages (.oiv)

### Installing

**Install .oiv…** (or `openrage-gui package.oiv`) shows the package — name, author, description — and exactly what
installing it would change, before anything is written. Changes go to `mods`; new loose files such as `.asi` plugins
go to the game folder, where loaders expect them.

### What is supported

Packages of format 2.x:

| Kind | Supported |
|---|---|
| Files | added to or deleted from the game and from archives, also nested ones |
| Archives | new archives and DLC packs (`createIfNotExist`) |
| Text edits | `add`, `insert`, `replace`, `delete` with Equal, StartWith or Mask |
| XML edits | by XPath: `add` (First, Last, Before, After), `replace`, `remove`, keeping the formatting |

### Uninstalling

Every installed package can be **uninstalled** from **History** (or `openrage oiv uninstall`). OpenIV cannot do that.

### Not supported yet

`<pso>` edits: such packages are refused with an explanation. `<defragmentation>` is skipped, as it only saves disk space.

## Mods folder

OpenRAGE understands the `mods` folder of OpenIV.asi, where a file at `mods/<path>` replaces `<path>` in the game. It
never changes that layout; it only shows what it means.

### Mods overlay

**Mods** in the toolbar shows the game the way it sees itself:

| Mark | Meaning |
|---|---|
| *mod* | the file is replaced by one in `mods` |
| *new* | the file exists only in `mods` |
| *has mods* | the folder contains mods |

### Comparing with the original

Inside a modded archive every folder is compared with the original: entries are marked *added* (green), *changed*
(orange) or *removed* (red, greyed out, with a preview of the original).

### All changes at once

**Changes** lists every difference of all mods across nested archives — including complete copies of game archives that
contain no change at all and can simply be deleted.
