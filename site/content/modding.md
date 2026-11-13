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

A drawable of GTA V Enhanced (`.ydr`) can get new geometry from a Wavefront OBJ file. For now from the
[command line](@/cli.md#editing-packages-and-the-mods-folder):

```
openrage replace-model x64i.rpf/levels/gta5/props/…/prop_x.ydr model.obj --mods
```

- The drawable keeps its shaders and textures: OpenRAGE copies them, with the vertex layout each shader expects,
  from the original file and writes only the new geometry. The OBJ materials go to the drawable's shaders in order
  (`--shader N` puts everything on one); the output lists which went where.
- The model gets one detail level, visible as far as the original's furthest. Meshes over 65,535 vertices are split.
- OBJ is converted from Y-up to the game's Z-up; missing normals are computed, tangents always.
- `--mods` puts it into the `mods` folder in place of the original (undo with **Undo** / `openrage undo`), `--out`
  writes the file elsewhere.
- Not yet: GTA V Legacy, `.yft`/`.ydd`, skinned models, new textures and collisions (the original collision stays).
  The written drawables read back identically in OpenRAGE (`openrage verify --models --rebuild` writes every drawable
  of the game anew and compares); please report how they behave in the game.

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
