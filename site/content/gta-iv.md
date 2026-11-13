+++
title = "GTA IV"
description = "GTA IV and its episodes in OpenRAGE: setup, archives, textures, models, texts, placement files, maps, collisions, animations, scripts, search and editing."

[extra]
docs = true
lead = "The Complete Edition on Steam, including *The Lost and Damned* and *The Ballad of Gay Tony*. Changes go into the `update` folder, or into the game files with the originals kept, and can be undone."
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
| Archives | `.img` (IMG3), `.rpf` (RPF2, RPF3) | browse, extract and change |
| Textures | `.wtd`, inside `.wdr` | view, export DDS/PNG; replace, add and remove |
| Models | `.wdr`, `.wdd`, `.wft` | view, export glTF |
| Texts | `.gxt` | view and edit |
| Placement files and maps | `.wpl`, map archives | view as text and in 3D, export glTF |
| Collisions | `.wbn`, `.wbd`, in `.wft` | view, export glTF |
| Animations | `.wad` | play on peds |
| Scripts | `.sco` | read the disassembly |
| Search | everything above, `.ide`, `.dat`, `.gxt` | names, hashes and full text |

Changes go into the `update` folder; see [Editing](#editing).

## Search

The search box works as for GTA V: file names, hashes (a model's hash from a placement file finds its `.wdr`) and the
full text of the game's `.ide`, `.dat`, `.ipl`, `.csv` and other text files and of its text tables (`.gxt`, with the
language filter). GTA IV keeps much of this outside its archives, so loose files are indexed with their text too. The
index is built in the background the first time you open the game (a few seconds) and updated when archives change.

```
openrage index
openrage search admiral                        # vehicles.img/admiral.wft, handling.dat, vehicles.ide, …
openrage search "Spieler ist unbesiegbar" --lang german
```

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

In edit mode, textures can be replaced from a DDS file, added and removed, as in GTA V; the changed dictionary goes
into the `update` folder (or the game files, see [Editing](#editing)) and can be undone. A replacement with the same
format, size and mip count only swaps the pixel data. Anything else rebuilds the dictionary: its other data stays as
it was, so fields OpenRAGE does not interpret are kept. Use DXT1, DXT3, DXT5 or 32-bit BGRA DDS files; textures that
only exist in newer formats (BC7 and others) cannot go into GTA IV.

**Textures inside models:** many props and cutscene objects (`.wdr`) carry their own textures; vehicles and peds
(`.wft`, `.wdd`) never do, theirs are in `.wtd` files. Selecting such a model shows a **Textures (N)** button that opens
them as a list, edited like a dictionary. The model's other data stays untouched: a replacement with a new size or
format adds its pixel data at the end, and every texture keeps its place, because the model's shaders point at it. A
texture a shader uses cannot be removed (replace it instead); added textures are only used once a shader refers to
them.

```
openrage textures pc/models/cdimages/vehicles.img/admiral.wtd --set admiral_badges=badges.dds   # replace
openrage textures pc/models/cdimages/vehicles.img/admiral.wtd --set my_decal=decal.dds --remove admiral_sign
openrage textures pc/anim/cutsprops.img/csak47.wdr --set cj_ak1=wood.dds        # a model's own textures
openrage verify --textures --rebuild           # rebuild every dictionary in memory and compare (writes nothing)
```

**Checked:** 6,400 dictionaries and 9,820 models with their own textures (136,125 textures), all decoded; every
dictionary rebuilt and read back identically, every model rebuilt byte for byte.

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
  and LOD, then the other sections. In edit mode, **Open in text editor** (right-click; double-click opens the map)
  lets you move, add and remove objects; saving turns the text back into the `.wpl` (into the `update` folder, with
  undo). Numbers are written exactly (floats always with a point, `5.0`), so an unchanged file saves byte for byte;
  model names become their hash, `0x…` hashes work too, and mistakes are reported with their line. Objects name their
  LOD by its position in the list, so add new ones at the end.
- **Object definitions** (`.ide`) and `.ipl` files are plain text: they open in the text editor and save the same way.
  New model names are picked up right after saving.
- **Scripts** (`.sco`) show their facts and the strings they use; double-click opens the disassembly. Most are
  encrypted with the archive key, which OpenRAGE already has. Functions are named `func_N` and natives shown by hash:
  GTA IV keeps neither name. Every jump target gets a label.

**Checked:** 5,498 text tables (671,627 strings, converting back identically), 1,151 placement files (458,048
records, each file converting from text back to the same bytes), 941 scripts
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
one clip per animation, with the models to play it on. Every GTA IV ped shares one skeleton, and so do the cutscene
characters (`cs_*` in `cutsprops.img`), whose faces have a fuller rig.

- Translations, rotations and the mover are read. A few tracks of other values that move no bone (flags and weights
  for the game) are skipped.
- **Faces:** `facials@…` (moods, gestures, pain, looks) and `visemes@…` (mouth shapes for speech) move the face bones
  like any other animation. The `…_lo` dictionaries fit the peds' face rig, the `…_hi` ones the cutscene characters':
  selecting one offers the fitting models.
- The cutscene characters' skin shaders (`gta_ped_skin…`) keep other data in the texture's alpha channel; OpenRAGE
  draws them opaque (also in the glTF export), so their heads show.
- Twist and roll helper bones follow the bone they rest on, so arms stay in one piece.

```
openrage clips <path to .wad>
openrage model <ped> --anim <wad>:<clip>[@seconds] --render pose.png
openrage model pc/anim/cutsprops.img/cs_andrei.wdd --anim "pc/anim/anim.img/facials@m_hi.wad:angry_c@0.6" --render angry.png
```

**Checked:** 1,772 dictionaries, 22,866 animations, every frame evaluated, no errors.

## Editing

GTA IV has no `mods` folder. With [FusionFix](https://github.com/ThirteenAG/GTAIV.EFLC.FusionFix) installed, the game
loads files from the `update` folder next to `GTAIV.exe` in place of the originals at the same path, so
`update/pc/models/cdimages/vehicles.img` replaces `pc/models/cdimages/vehicles.img`. OpenRAGE writes every change
there, as it uses the `mods` folder for GTA V; the game folder itself stays untouched. Without FusionFix the game
ignores the `update` folder, and OpenRAGE says so when you make a change.

- **Archives (`.img`, `.rpf`):** the first change copies the whole archive into the `update` folder; that copy is
  edited from then on. Import, replace, rename and remove entries as in GTA V; `.rpf` archives also have folders.
  Resources (`.wtd`, `.wdr`, `.wft`, …) keep their type and flags from their header. The table of an `.rpf` is written
  the way the game's own are (same order, encrypted again with the key from your `GTAIV.exe`); `openrage verify
  --rebuild` checks this against every `.rpf` of the game without writing anything.
- **Audio (`.rpf`, RPF3):** these archives store only a hash of each name, shown as eight hex digits. Replace an entry
  by importing a file under the same hex name; new entries need a hex name too.
- **Loose files** (`.dat`, `.ide`, `.gxt`, …) are written to the `update` folder under the same path.
- **Texts:** `.gxt` files open in the text editor and save into the `update` folder.
- **Changes and history:** **Changes** lists every file and archive entry that differs from the game, **History**
  every change OpenRAGE made; all of them can be undone. The browser hides the `update` folder and shows its files in
  place of the originals.

```
openrage put admiral.wtd pc/models/cdimages/vehicles.img/       # import into a copy of the archive in update/
openrage mv pc/models/cdimages/vehicles.img/a.wft pc/models/cdimages/vehicles.img/b.wft
openrage rm pc/models/cdimages/vehicles.img/biff.wft
openrage put niko_feet.wdr pc/models/cdimages/playerped.rpf/feet_000_u.wdr   # .rpf archives work the same way
openrage put 061b65da pc/audio/sfx/general.rpf/6b34fe60/                     # audio: entries named by hash
openrage put handling.dat common/data/                          # a loose file, written to update/common/data/
openrage mods --changes                                         # everything that differs from the game
openrage history
openrage undo
```

### Writing into the game files

Without FusionFix, or if you prefer it, changes can go into the game's own files instead. Tick **Write directly into
the game files** in the **Unlock editing** dialog, or run `openrage write-target game`; `openrage write-target update`
switches back. The choice is remembered per game.

- Before a game file is changed for the first time, OpenRAGE keeps its original in `update/.openrage/originals/` (the
  game does not load anything from there). Make sure there is room for a copy of each archive you change.
- Every change is still recorded in **History** and can be undone step by step. **Changes** lists them as
  "in the game".
- **History → Restore game files** (or `openrage originals --restore-all`) puts every kept original back; the
  changes made to those files then leave the history.
- If the `update` folder already holds a copy of a file, OpenRAGE refuses to change the game's own file: the game
  would load the copy, not your change. Change the copy, or remove it first.

```
openrage write-target game                                      # changes go into the game files from now on
openrage put admiral.wtd pc/models/cdimages/vehicles.img/       # edits pc/models/cdimages/vehicles.img itself
openrage originals                                              # files whose originals are kept
openrage originals --restore pc/models/cdimages/vehicles.img    # put one original back (or --restore-all)
openrage write-target update                                    # back to the update folder
```
