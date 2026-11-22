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

A drawable (`.ydr`), a vehicle with its wheels or one of its mod parts (`.yft`) and the drawables of a dictionary
(`.ydd`: peds, clothing) of GTA V Enhanced can get new geometry and textures from a Wavefront OBJ file:

- **In the explorer:** turn on the *OBJ tools* plugin (it comes with OpenRAGE, **Settings → Plugins**), unlock
  editing, right-click the `.ydr`, `.yft` or `.ydd` → **Replace with OBJ…** and choose the OBJ together with its
  `.mtl` and PNG textures. The change goes into the `mods` folder and is one entry in **History**. *OBJ tools* also exports models as
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
- Not yet: GTA V Legacy, and collisions (the original collision stays). The written drawables, vehicles with their
  wheels and dictionaries read back identically in OpenRAGE (`openrage verify --models --rebuild` rewrites every
  `.ydr`, `.yft` with its wheel models and every drawable of every `.ydd` of the game, with all bone weights, also with
  shader copies and embedded textures); please report how they behave in the game.

### Vehicles

A vehicle body is one model on the vehicle's skeleton: every part moves with a bone (doors, bonnet, boot, bumpers).
The import keeps that:

- **Parts by name:** an OBJ object or group (`o`, `g`) named like a bone goes on that bone, e.g. `door_dside_f`,
  `door_pside_f`, `bonnet`, `boot`, `bumper_f`. Blender's `.001` and the numbers *OBJ tools* adds (`bonnet_6`) are
  ignored. Everything else goes on the body (`bodyshell`). `replace-model` lists which part went where.
- **Wheels** are models of their own. A part named after the wheel bone that has the vehicle's wheel model (usually
  `wheel_lf`) replaces that model: it is moved into the bone's space, and the game copies it to every wheel bone
  (turned around on the right). Parts named after the other wheel bones (`wheel_rf`, `wheel_lr` …, which *OBJ tools*
  exports as copies) are left out. A vehicle with a rear wheel model of its own (`wheel_lr`) gets that one replaced
  the same way. The wheel's collision and the radius the game drives it with stay.
- **Export, edit, import:** *OBJ tools* exports a vehicle with one object per bone, so an export can be edited in
  Blender and brought back with every door still opening.
- The skeleton, physics, damage and the vehicle's detail levels stay; every level shows the new body (and wheel).
- **Mod parts** (`sultan_hood_1.yft` in `sultan_mods.rpf`) are fragments of their own, in the space of the bone they
  go on: import onto them like any `.yft`. They have no textures of their own, so the shared vehicle textures
  (`vehshare.ytd`) lend the texture structure.
- `openrage replace-model x64e.rpf/levels/gta5/vehicles.rpf/adder.yft car.obj --mods`.

### Peds and clothing

- **Drawable dictionaries (`.ydd`):** parts named after a drawable (`uppr_000_u`, as *OBJ tools* exports them)
  replace that drawable; the others keep theirs. `--drawable uppr_000_u` (name or index) puts the parts named after
  none on that drawable.
- **Bone weights:** peds and clothing are blended between bones, but OBJ has no place for weights. Every new vertex
  takes the bones and weights of the nearest vertex of the original drawable, so a changed shirt keeps moving with
  the body where it lies close to the original; parts far from any original vertex follow the nearest one and can
  look stiff. Exported and re-imported unchanged, every vertex keeps its weights (a vertex on a seam may take those
  of the other side).
- `openrage replace-model …/a_m_y_hipster_01.ydd ped.obj --mods`.

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
