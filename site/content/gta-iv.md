+++
title = "GTA IV"
description = "GTA IV and its episodes in OpenRAGE: setup, archives, textures, models, texts, placement files, maps, collisions and animations."

[extra]
docs = true
lead = "The Complete Edition on Steam, including *The Lost and Damned* and *The Ballad of Gay Tony*. Read-only for now."
toc = true
+++

## Setup

Choose the GTA IV folder in the setup dialog, or under **File → Switch game… → Add a game…**. The Steam folder
`Grand Theft Auto IV` and the `GTAIV` folder inside it both work.

OpenRAGE finds the archive key in your own `GTAIV.exe` by a checksum and keeps it in `gta4.keys` in your config folder.
That takes under a second; nothing else is needed. On the command line:

```
openrage --game "/path/to/Grand Theft Auto IV" keys
openrage ls pc/models/cdimages
```

## At a glance

| What | Files | Status |
|---|---|---|
| Archives | `.img` (IMG3), `.rpf` (RPF2, RPF3) | browse and extract |
| Textures | `.wtd` | view, export DDS/PNG |
| Models | `.wdr`, `.wdd`, `.wft` | view, export glTF |
| Texts | `.gxt` | view |
| Placement files and maps | `.wpl`, map archives | view as text and in 3D, export glTF |
| Collisions | `.wbn`, `.wbd`, in `.wft` | view, export glTF |
| Animations | `.wad` | play on peds |
| Scripts | `.sco` | read the disassembly |

Changing GTA IV files and the search index are not available yet.

## Archives

Most files are in `.img` archives, a few in `.rpf` archives (the audio archives are RPF3).

- Archives open like folders: file details, a text preview for plain files, the resource type and memory sizes.
- *Extract…* saves files, folders and whole archives. Files are saved as stored, like OpenIV does: resources as RSC5
  files with their header.
- RPF3 archives keep only a hash of each name, so their entries show as hexadecimal numbers.

**Checked:** `openrage verify` reads all 303 archives of the game (46,889 resources, no errors).

## Textures

Texture dictionaries (`.wtd`) open like GTA V's: preview, viewer and export as DDS or PNG. GTA IV stores DXT1/3/5 and
uncompressed 32-bit textures.

**Checked:** 6,400 dictionaries, 70,845 textures, all decoded.

## Models

Drawables (`.wdr`), drawable dictionaries (`.wdd`) and fragments (`.wft`) open in the preview and the 3D viewer and
export as glTF.

### Vehicles and peds

- Vehicles show their wheels on every wheel bone.
- Peds (`componentpeds.img`) are put together from the first variant of each component on their skeleton; switch the
  others on under *Parts*.

### Where textures come from

1. the dictionary embedded in the model,
2. the `.wtd` named after it,
3. the dictionary the game's `.ide` files give the model, and its parent dictionaries,
4. for vehicles, `vehshare.wtd`.

OpenRAGE indexes the `.ide` and `.wtd` files once per session (a second or two) and prefers an episode's own files for
TLAD and TBoGT models.

**Checked:** 26,711 files, 46,658 drawables, 61 million triangles, no errors.

## Texts, placement files and scripts

- **Texts** (`common/text/<language>.gxt`, and the episodes' own) open in a read-only window: every table as a
  `[NAME]` section of `0xHASH = text` lines, in GTA V's `.oxt` form.
- **Placement files** (`.wpl`) show as IPL-style text: one line per object with position, rotation, model name, flags
  and LOD, then the other sections. `.ide` and `.ipl` files are plain text already.
- **Scripts** (`.sco`) show their facts and the strings they use; double-click opens the disassembly. Most are
  encrypted with the archive key, which OpenRAGE already has. Functions are named `func_N` and natives shown by hash:
  GTA IV keeps neither name. Every jump target gets a label.

**Checked:** 5,498 text tables (671,627 strings, converting back identically), 1,151 placement files, 941 scripts
(14.5 million instructions; every jump, call and switch case lands on an instruction).

## Maps

Double-click a placement file (`.wpl`) to see its objects in 3D, with the object list and glTF export.

- A map archive (`pc/data/maps/…/<area>.img`) is mostly a set of streamed placement files. `openrage map <archive>`
  puts them together, for example a whole part of Algonquin with 2,483 objects.
- Models come from the archive next to the placement file first, then from anywhere in the game.
- Low-detail stand-ins (`lod…`, `slod…`) are left out unless a file holds nothing else.

**Checked:** of 435,116 placed objects, all but 1,434 have a model (their names are in no `.ide` file).

## Collisions

`.wbn` (one bound, usually a piece of a map area) and `.wbd` (a dictionary of bounds) open like GTA V's collisions,
coloured by material with the names from GTA IV's own `materials.dat`, and export as glTF.

Fragments (`.wft`: vehicles, peds, breakable props) carry their collision: switch on the hidden part *Collision* under
*Parts* to see it over the model, for example a vehicle's body and wheels or a ped's ragdoll of capsules.

**Checked:** 2,907 files, 36,857 bounds, 26 million triangles, no errors.

## Animations

Animation dictionaries (`.wad`, in `pc/anim/anim.img`, cutscenes in `cuts.img`) open like GTA V's clip dictionaries:
one clip per animation, with the ped models to play it on. Every GTA IV ped shares one skeleton.

- Translations, rotations and the mover are read; facial and custom tracks are skipped.
- Twist and roll helper bones follow the bone they rest on, so arms stay in one piece.

```
openrage clips <path to .wad>
openrage model <ped> --anim <wad>:<clip>[@seconds] --render pose.png
```

**Checked:** 1,772 dictionaries, 22,866 animations, every frame evaluated, no errors.
