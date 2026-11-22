+++
title = "Modding"
description = "Change the game safely with OpenRAGE: edit mode, editing archives, textures and game text, undo and history, .oiv packages and OpenIV's mods folder."

[extra]
docs = true
lead = "Every change goes to OpenIV's `mods` folder (RDR2: the replace folder of Lenny's Mod Loader), never into your game files, and every change can be undone."
toc = true
+++

## Edit mode

> [!NOTE]
> **Red Dead Redemption 2** works the same way, but changes go into `lml/replace`, the replace folder of
> [Lenny's Mod Loader](https://www.rdr2mods.com/downloads/rdr2/tools/76-lennys-mod-loader-rdr/): changed files
> loose, changed nested archives as copies. See [Changing files](@/rdr2.md#changing-files) for what LML allows.

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

### Textures inside models

Many models (`.ydr`, `.yft`, `.ydd`) carry textures of their own; about 60,000 in GTA V Enhanced. Selecting one shows
a **Textures (N)** button that opens them as a list, changed like a dictionary:

- A replacement whose pixel data is the same size or smaller (the original's resolution and format, or less) is
  written where the old texture was. This always works.
- Anything that needs more room — a larger texture, an added one — goes into memory pages added at the end of the
  model; nothing that is already there moves. A model can only grow by pages no larger than its smallest one, so a
  much larger texture may not fit. OpenRAGE then says how much room there is.
- Shaders find textures by name, so replacing, adding and removing need no other change. A `.ydd` has a list per
  drawable: a replacement or removal applies to every list with that name; new textures can only be added to models
  with one list.

```
openrage textures x64b.rpf/levels/gta5/generic/icons.rpf/prop_mk_bike_logo_1.ydr                       # list
openrage textures …/prop_mk_bike_logo_1.ydr --set prop_bike_logo_1prop_bike_1_a=logo.dds                 # replace
openrage verify --textures --rebuild           # also edits every model's textures in memory and reads them back
```

**Checked** on GTA V Enhanced: in each of 60,257 models with textures, a texture written over itself leaves the file
byte for byte, and a texture four times as large (fitting in 10,170 models) and an added one read back with every
other texture, mesh, shader and bone unchanged. GTA V Legacy uses the same structures and is covered by tests only.

## Models (preview)

A drawable (`.ydr`) or a vehicle body (`.yft`) of GTA V Enhanced can get new geometry and textures from a Wavefront
OBJ file:

- **In the explorer:** turn on the *OBJ tools* plugin (it comes with OpenRAGE, **Settings → Plugins**), unlock
  editing, right-click the `.ydr` or `.yft` → **Replace with OBJ…** and choose the OBJ together with its `.mtl` and PNG
  textures. The change goes into the `mods` folder and is one entry in **History**. *OBJ tools* also exports models as
  OBJ with their textures, so you can export, edit (e.g. in Blender) and bring them back.
- **From the command line:** `openrage replace-model x64i.rpf/levels/gta5/props/…/prop_x.ydr model.obj --mods`
  (it finds the `.mtl` and textures next to the OBJ), or `openrage plugin run obj-tools replace <path>.ydr --pick
  model.obj --pick model.mtl --pick texture.png …`.

How it works:

- **Shaders:** every OBJ material gets its own copy of one of the drawable's shaders. A material named after a
  shader's texture (as *OBJ tools* exports them, e.g. `vehicle_badges_adder_badges`) goes on that shader, others go
  on the shaders in order; `--shader N` uses shader N for all. The copies keep the original shader's settings and the
  vertex layout it expects.
- **Textures:** a material's `map_Kd` (PNG) is embedded in the drawable and bound to its copy's colour slot, with all
  mip levels; a material with only a colour (`Kd`) gets a small texture of that colour; without either it keeps the
  original texture, as does a shader without a colour slot (vehicle paint, licence plates). Models without embedded
  textures borrow the texture structure of another model or texture dictionary of the game.
  Textures are block compressed like the game's own (BC1, or BC3 when they have transparency), with mip levels;
  a 512×512 texture takes about 170 KB instead of 1.4 MB. Sizes that are not multiples of 4 stay uncompressed.
  `--no-textures` keeps the original textures; in the explorer, pick only the PNGs you changed.
- **Detail levels:** `--lod near.obj --lod far.obj` (up to three) add further levels; plugins mark them with parts
  named `…_lod1`, `…_lod2`, `…_lod3`. Distances come from the original's levels where it has them, otherwise 50 m
  doubling per level; the last level is seen as far as the original's furthest.
- OBJ is converted from Y-up to the game's Z-up; missing normals are computed, tangents always. Meshes over 65,535
  vertices are split. `--mods` puts the result into the `mods` folder in place of the original (undo with **Undo** /
  `openrage undo`), `--out` writes the file elsewhere.
- Not yet: GTA V Legacy, `.ydd`, peds and other models blended between bones, and collisions (the original collision
  stays). The written drawables and vehicles read back identically in OpenRAGE (`openrage verify --models --rebuild`
  rewrites every `.ydr` and `.yft` of the game, also with shader copies and embedded textures); please report how
  they behave in the game.

### openFormats (OpenIV)

OpenIV's text formats go both ways: export a model or texture dictionary, edit the text files or textures (in a text
editor, a script or a tool that reads openFormats), and import them back.

- **In the explorer:** right-click a `.ydr`, `.yft`, `.ydd` or `.ytd` → **Export openFormats…** and choose a folder.
  To bring the files back, unlock editing, right-click the same file → **Import openFormats…** and pick the `.odr`,
  `.oft` or `.otd`. The change goes into the `mods` folder and is one entry in **History**.
- **From the command line:** `openrage export-openformats x64i.rpf/levels/gta5/props/…/prop_x.ydr -o out/`, then
  `openrage import-openformats x64i.rpf/levels/gta5/props/…/prop_x.ydr out/prop_x.odr --mods` (or `--out new.ydr`).

What an import takes over (GTA V Enhanced models; texture dictionaries of both versions):

- **Meshes and detail levels** of the `.mesh` files: positions, normals, colours, texture coordinates and, for
  vehicles, the bone each vertex moves with. They are written like an OBJ import (above): every geometry keeps a
  shader of the model and its vertex layout; meshes attached to other bones are moved onto the model's.
- **Textures:** changed `.dds` files of a model's own textures replace them (any size and format); a `.otd` onto a
  `.ytd` replaces, adds and removes textures.
- **Kept from the game file:** the shaders (an `.odr` must name the model's shaders; changed parameters are reported
  and not taken over), the skeleton, the physics and the parts of a fragment (wheels), collisions. Unchanged meshes
  stay as they are, so a texture edit changes only the texture. `.odd` files cannot be imported yet.
- The import says what it took over and what it kept (status bar and **History**, or the command's output).

The files follow OpenIV's own exports (a prop exported by OpenRAGE matches OpenIV's `.odr`, `.mesh` and `.skel`
line for line, apart from the last digit of some numbers). Not taken from OpenIV, so possibly different there: the
embedded texture dictionary line of an `.odr`, the `.oft` layout (OpenRAGE writes the main drawable and its parts,
not the physics) and vertex layouts OpenIV's exports have not shown, which OpenRAGE names with their layout
(`N40597755555555996996`) instead of OpenIV's code. Whether OpenIV reads OpenRAGE's exports has not been tested yet.

**Checked** on GTA V Enhanced with `openrage verify --models --openformats`: every model and texture dictionary
of the game is written as openFormats and read back unchanged, and every drawable and fragment is imported back
onto itself and reads back with the same meshes, shaders, bones and textures.

### Vehicles

A vehicle body is one model on the vehicle's skeleton: every part moves with a bone (doors, bonnet, boot, bumpers).
The import keeps that:

- **Parts by name:** an OBJ object or group (`o`, `g`) named like a bone goes on that bone, e.g. `door_dside_f`,
  `door_pside_f`, `bonnet`, `boot`, `bumper_f`. Blender's `.001` and the numbers *OBJ tools* adds (`bonnet_6`) are
  ignored. Everything else goes on the body (`bodyshell`). `replace-model` lists which part went where.
- **Wheels** are models of their own: parts named after a wheel bone (`wheel_lf` …) are left out, and the original
  wheels stay.
- **Export, edit, import:** *OBJ tools* exports a vehicle with one object per bone, so an export can be edited in
  Blender and brought back with every door still opening.
- The skeleton, physics, damage, the wheels and the vehicle's detail levels stay; every level shows the new body.
- `openrage replace-model x64e.rpf/levels/gta5/vehicles.rpf/adder.yft car.obj --mods`.

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
| PSO edits | `<pso>`: binary metadata (`.ymt`, `.ymf`, `.ytyp`, … in PSO form) is converted to XML, edited with the same commands as `<xml>`, and written back as PSO |

PSO edits work on the XML OpenRAGE shows for the file (**View as XML**, `openrage xml`), the same as CodeWalker's:
hashes appear as their names where the name is known, otherwise as `hash_XXXXXXXX`; names that an XPath compares with
(`Item[modelName = 'adder']`) are always recognised. The file keeps its own structure definitions, so the edit can
change values and add or remove items, but not invent fields the file's structures do not have; the preview says so
before anything is written. Written back unchanged, 80% of GTA V's PSO files come out byte for byte the same and every
one reads back to the same XML (`openrage verify --meta --rebuild`); the rest differ only in bytes the XML does not
carry, such as the unused fourth number of a 3D vector.

### Uninstalling

Every installed package can be **uninstalled** from **History** (or `openrage oiv uninstall`). OpenIV cannot do that.

### Not supported yet

`<pso>` edits of metadata in resource form (`RSC7` `.ymt` files such as ped variations) are refused with an
explanation; only PSO files can be edited that way. `<defragmentation>` is skipped, as it only saves disk space.

## ASI mods (GTA V)

ASI mods (trainers, scripts for Script Hook V) are DLLs that a **loader** in the game folder loads when GTA V starts.
**Mods → ASI mods…** opens a window (or `openrage asi` in a terminal) that shows what is installed and why mods
would not load:

- the loader and whether it fits the game: GTA V Enhanced needs its own loader (`xinput1_4.dll` from Script Hook V for
  Enhanced); the older `dinput8.dll` loader is for GTA V Legacy. OpenRAGE tells them apart by what the files say about
  themselves, not by their names;
- whether Script Hook V is installed;
- each `.asi` file, with a switch: **off** puts it aside in `.openrage/disabled-asi` in the game folder, **on** puts it
  back. Nothing is deleted. **Install ASI mod…** copies one into the game folder, recorded in History (undo removes it).

**Linux (Proton):** Wine uses its own copy of the DLL the loader stands in for, so no ASI is ever loaded, unless the
game's prefix tells Wine to take the game's DLL first. **Let Proton load the ASI loader** sets exactly that for the
game's prefix (`compatdata/<app id>/pfx/user.reg`, backed up first as `user.reg.openrage-backup`), and can be taken
back the same way. The game must be closed. When the loader is there but Proton would not load it, a hint above the file
list says so, with the same button; dismiss it with ✕ (remembered for that game). Instead, the launch option `WINEDLLOVERRIDES="xinput1_4=n,b" %command%`
(with the loader's name) does the same for one game.

> [!WARNING]
> ASI mods are for story mode only. GTA Online bans modified games, and the anti-cheat BattlEye that GTA V Enhanced
> starts with may stop ASI loaders.

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
