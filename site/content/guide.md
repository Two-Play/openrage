+++
title = "User guide"
description = "Everything the OpenRAGE explorer can do: search, textures, models, audio, metadata, collisions, paths, cutscenes, scripts, game text, editing archives, .oiv packages and the mods folder."

[extra]
lead = "Everything the explorer can do, format by format."
toc = true
+++

New here? Start with [Getting started](@/getting-started.md). Prefer a terminal? See the [command line](@/cli.md).

## The explorer

Start `openrage-gui`, or `openrage-gui <virtual path>` to open a folder or file directly.

The explorer shows a folder tree in which archives (including nested ones) behave like folders, a sortable file list with format icons (textures, models, maps, data, audio, scripts …), and a preview pane (text with a fixed-width font, hex dump for binaries and resource pages, resource version and page sizes). Extraction runs in the background and can be cancelled; its output matches OpenIV's.

- **Menus and context menus:** every action is in the menu bar (File, Edit, View, Go, Help) with its keyboard shortcut; right-click a file for the actions that apply to it (open, view, extract, export, replace, rename, delete, copy name/path).
- **Double-click** opens folders, archives and texture dictionaries; models and textures open in their own **viewer window** (also Ctrl+Enter) with a details panel. Several viewers can be open at once, e.g. to compare models.
- **Text editor:** double-click a text file (`.xml`, `.meta`, `.dat`, `.ini`, …) to open it in its own window — read-only until edit mode is unlocked; **Save** (Ctrl+S) writes it into the mods folder, can be undone, and keeps the file's encoding, line endings and tabs. Right-click → *Open with system app* opens a read-only copy in your default program.
- **Clip dictionaries** (`.ycd`): selecting one shows what it animates (peds, vehicles, objects) and a list of fitting models, ranked by the bones they share with the clips; click one — or double-click the file — to open it in the viewer with the animation playing.
- **Overviews:** a texture dictionary previews up to 16 of its textures at once; a drawable dictionary that is not a ped shows all its objects side by side. Files that contain other files (texture dictionaries, audio containers) carry a badge on their icon.
- **History** opens in its own window (View → *History in its own window* switches back to the list).
- **Address bar** with clickable breadcrumbs like a file manager; click the free space (or Ctrl+L) to type a path.
- The info panel shows metadata as a table (type, size, version, format, mip levels, meshes, triangles …).
- **Keyboard:** arrow keys, Page Up/Down, Home/End and Enter in the list; Backspace or Alt+Up goes up; Ctrl+L address, Ctrl+F search, F2 rename, Delete, Ctrl+Z undo. Help → Keyboard shortcuts (F1) lists them all.
- **Edit mode, like OpenIV:** OpenRAGE starts read-only. **Read-only → Editing** in the toolbar (or Edit → Unlock editing…) asks once per session; only then can archives, textures and packages be changed. The bar above the list says whether you are looking at an original game archive (changes are copied to `mods`) or the copy in `mods`.
- **Light and dark mode:** follows the system, or pick one under View → Theme (the sun/moon button switches quickly). The choice is remembered.

## Settings

**Settings** (gear button or File → Settings…, Ctrl+,): light, dark or system theme; the accent color — the style's default, OpenRAGE orange (the logo's), a preset or any color of your own; the configured game (GTA V, GTA V Enhanced or RDR2, shown in the address bar) with *Switch game…*, *Add or change a game…* and whether to ask which game to open at start; and whether the history opens in its own window and the mods overlay is shown.

## About and licenses

**Help → About OpenRAGE** shows the version, OpenRAGE's license (with the full GPL-3.0 and, for the TFIT2 cipher,
AGPL-3.0 texts), a link to the source code, the credits from `NOTICE.md`, and every third-party crate in the program
with its license and full license text, filterable by name or license. The same list is on the
[licenses page](@/licenses.md) and in `THIRD_PARTY_LICENSES.md` next to the programs.

## Search

OpenRAGE keeps a search index of the whole game (built once in about 30 seconds, then updated incrementally — only archives that changed after a game update are re-read). The search field accepts:

- **names** — any part of a file or folder name, exact matches first;
- **hashes** — `0xB779A091`, `B779A091`, `3078201489` or `-1216765807` resolve to `adder` via a dictionary built from all names and identifiers in the game's data, and find files with that name hash and game text with that label hash;
- **text** — words in XML/META/DAT/UGC files, audio name tables and the game's GXT2 text tables (prefix matching, all words must occur). Game text is filtered to one language, by default your system language.

Typing `?query` into the address bar searches as well; Back returns to the results.

## Textures

Texture dictionaries (`.ytd`) open like folders: double-click one (or type its path) to list its textures with size, format and mip levels. Selecting a texture shows it in the preview; selecting a `.ytd` previews its first texture. **Extract…** writes DDS files exactly like OpenIV/CodeWalker (classic DXT/ATI FourCC headers, DX10 headers for BC7 and other modern formats); **Save PNG…** writes images. With nothing selected, the whole dictionary is exported into a folder named after it.

**Replace…**, **Add…** and **Remove** change the dictionary the OpenIV way: the archive is copied to the `mods` folder (or the copy already there is used) and rewritten there — your game files are never touched. Replacing a texture with a DDS of the same format, size and mip count only overwrites its pixels; anything else rebuilds the dictionary from its original structures with a fresh page layout. Archives are streamed while they are rewritten, so even multi-gigabyte archives need little memory.

Both layouts are supported: GTA V Legacy (resource version 13, Direct3D 9 formats) and GTA V Enhanced (version 5, DXGI formats such as BC7).

## Models

Selecting a model (`.ydr` drawable, `.yft` fragment such as a vehicle, `.ydd` drawable dictionary) shows it in 3D: drag to rotate, scroll to zoom. Models are shown **complete**: vehicles with their wheels on every wheel bone and their liveries, peds assembled from their components (head, hair, upper body, legs, accessories — the first variant of each), parts rigidly attached to bones in place. Textures are taken from the model itself, the `.ytd` next to it (`adder.ytd`, `adder+hi.ytd`) and, for vehicles, `vehshare.ytd`.

Double-click opens the model in a **viewer window** (rendered on the GPU via wgpu — Vulkan, DirectX 12, Metal or OpenGL — with a software fallback):

- **Orbit** (drag, wheel) or **Fly** (F): W A S D to move, Q/E down/up, Shift faster, drag to look around.
- **Parts:** grouped (body, wheels, ped components with their variants); show or hide groups or single parts.
- **Animation:** peds, vehicles (convertible roofs, doors, …) and props play clips from clip dictionaries (`.ycd`): search for a dictionary (e.g. `move_m@generic`, suggested for peds), pick a clip, play/pause, scrub, speed, loop. Skinned meshes, parts bound to bones and fragment parts (wheels) all follow the skeleton; skinning runs on the GPU.
- **Orientation compass:** the game's X/Y/Z axes in red/green/blue; click a letter to look along that axis.
- **Materials:** every shader with a readable name (primary paint, glass, tyres, interior, …); hide it or give it a colour — paint the car. Preview and glTF export follow.
- **Export glTF…** writes a binary glTF 2.0 file (`.glb`) with one node per shown part and the textures embedded, which Blender and other tools open directly (Z-up is converted to glTF's Y-up).

Not yet: vehicle mods and tuning parts, map placements in 3D (they can be read as XML).

## Audio

Audio containers (`.awc`) open like folders: effects and dialogue list their sounds, music and radio tracks show one multi-channel stream. Selecting a sound shows its waveform, length, sample rate and codec; **Play** (or double-click, or Space) plays it, clicking the waveform jumps. **Save WAV…** and **Extract…** write `.wav` files (16-bit PCM, like OpenIV).

Supported: PCM and IMA ADPCM (GTA V Legacy), MPEG audio (GTA V Enhanced), single sounds and block-based multi-channel streams, and Rockstar's audio encryption (whole files, sound data or stream blocks). The audio key comes from the key blob used during setup; key caches from older versions are upgraded automatically.

## Metadata (XML)

Map placements (`.ymap`), archetypes (`.ytyp`), `.ymt`, manifests (`.ymf`), cutscenes (`.cut`) and `.pso` files are shown as XML, formatted like CodeWalker's export: the preview shows the start; double-click (or **Show as XML**) opens a window with two views:

- **Structure:** the file as a tree — entities, archetypes, cutscene objects, … labelled with their type and name (`CEntityDef #3 · dt1_tc_ufocore`) — and the selected node's values as a table with readable names (`fTotalDuration` → *Total duration*), vectors as `x, y, z` and flags with their hex value. A filter finds names and values anywhere in the file.
- **XML:** the XML with syntax colors and line numbers; **Open as text** shows it in the text editor for copying.

**Export XML…** saves `name.ymap.xml`. All three binary encodings are read: RSC meta resources, PSO and RBF. Hashes are shown as names where the name is known: field and type names from a built-in list, model, texture and other asset names from the search index; unknown ones as `hash_XXXXXXXX`. Saving XML back into the game files will come later.

## Particles

Particle dictionaries (`.ypt`) list their effects (the names the game and scripts use, e.g. `bul_gravel_heli`), emitter and particle rules in the info panel. Their models — debris, shells, bottles, sparks — are shown side by side at the same size and open in the viewer; double-click the file to browse its textures like a texture dictionary (view, Save PNG, Extract; changing them is not supported yet).

## Collisions

Collision files (`.ybn`) open like models: the triangle meshes and the spheres, capsules, boxes and cylinders the game uses for physics, with each composite part in place. Surfaces are coloured by their material, whose names come from your game's `materials.dat` (CONCRETE, METAL_SOLID_MEDIUM, GRASS, …); in the viewer's Materials tab every material can be hidden or recoloured, and the collision exports to glTF like any model.

## Paths

Path node files (`.ynd`) show the road and footpath network of their map area: vehicle nodes in blue, pedestrian nodes in orange, with the links between them. Vehicle recordings (`.yvr`, the routes cars drive in missions) and waypoint recordings (`.ywr`, the routes people walk) show their route with start and end marked. Nav meshes (`.ynv`) show where peds can walk, coloured by kind: ground, pavement, road, water, interiors, train tracks and steep slopes. Each part can be hidden or recoloured in the viewer's Materials tab; a few of the game's recordings contain garbage points far outside the map, which are left out.

## Maps

A map placement file (`.ymap`) opens in the metadata window with **Show in 3D**: a scene of every object the file places, with its model and textures, interiors included. The game keeps the detailed objects of an area in its `…_strm_N.ymap` files and the distant stand-ins in the others; a file that only holds stand-ins shows those. The list on the right names every model with how often it is placed, to show or hide it; **Export glTF…** writes the shown objects as one `.glb` scene for Blender, centred on the origin. On the command line, `openrage map <ymap> [--render map.png] [--export map.glb]` does the same, and `openrage map --check` reads every map file of the game (11,052 files, 1,697,603 objects, 145 of them without a model: mostly breakable objects and a few stand-ins).

## Cutscenes

A cutscene (`.cut`) opens in the metadata window with a **Cutscene** tab: its length and its cast — people, vehicles, props, the camera, lights and audio. **Play whole cutscene** opens the scene: every person, vehicle and prop with its animation, placed where the cutscene puts it, seen through the cutscene camera (with its field of view) or — drag or scroll — a free camera; play, pause and scrub through it. **Play animation** opens a single actor in the viewer instead. The cutscene's sections (`name-0.ycd`, `name-1.ycd`, … 4 seconds each) are joined into one clip per actor. Online and story characters whose parts are streamed from a folder (`mp_m_freemode_01`, `cs_lestercrest_2`, …) are assembled from the first variant of each component, and props get their textures through the archetype file the cutscene names. The map around the scene is loaded too — buildings, interiors (with their rooms and furnishings from the interior's entity sets) and props within reach, each with its textures — and the cutscene's lights shine on it (their brightness is scaled relative to each other, an approximation of the game's lighting); both can be switched off. Without a GPU the scene is drawn in software at a lower resolution, without lights. Not shown: particle effects (smoke, sparks), which would need the game's particle simulation, and decals (dirt and stains), which need blending.

Finding the surroundings uses a world cache (every map placement file with its extents, every archetype with its texture dictionary and interior contents), built once in about half a minute and stored next to the search index; `openrage world <x> <y> <z>` lists what stands at a place.

## Scripts

Compiled scripts (`.ysc`) show their name, code size, number of functions, natives, statics and their strings in the preview; double-click opens the disassembly: one instruction per line with its address, functions (with their names where the compiler kept them), labels at jump targets, the text next to every `STRING` and the hash of every native call. Native names appear once you load a list in the common NativeDB format (`natives.json`) with Help → **Load native names…** or `openrage natives <file>`; OpenRAGE ships none. Both opcode tables of GTA V (older builds, and newer ones with `STATIC_U24` and `IS_BIT_SET`) are recognised automatically by checking that every jump lands on an instruction.

## Scaleform movies and videos

Scaleform movies (`.gfx`, the game's menus and HUD) show their stage size, frame rate, the symbols they export and the texture images they load; **Export as SWF…** turns one into a Flash file for Flash tools such as JPEXS or Ruffle. Bink videos (`.bik`) show their size, frame rate, length and audio tracks, and — when FFmpeg is installed — a frame from the middle; double-click plays the video in your video player (VLC, mpv and other FFmpeg-based players play Bink).

## Text tables

Game text (`.gxt2`) opens in the text editor in OpenIV's `.oxt` form, one `0xHASH = text` line per string (label names are used where the search index knows them). Audio name tables (`.nametable`) show one name per line, and `.ugc` files open as the JSON they are. After **Unlock editing**, saving converts the text back to `.gxt2` or `.nametable` and writes it into the mods folder; the conversion is lossless for every text table of the game. **Export as text…** writes `.oxt` or `.txt`.

## Editing archives

Inside an archive, **Import…** (or dropping files onto the list, where the platform supports it), **Replace…**, **Rename…**, **New folder…** and **Delete** change the game the OpenIV way: the archive is copied to the `mods` folder once and then edited **in place** — new data is appended and only the table of contents is rewritten, so a change to a 2 GB archive takes milliseconds. Changes inside nested archives rebuild just that nested archive. Your game files are never touched.

Every change is recorded in `mods/.openrage/journal` and can be reverted with **Undo**, step by step, back to the original files (OpenIV has no undo).

## OpenIV packages (`.oiv`)

**Install .oiv…** (or `openrage-gui package.oiv`) shows the package — name, author, description — and exactly what installing it would change, before anything is written. Packages of format 2.x are supported: files added to or deleted from the game and from archives (also nested ones), new archives and DLC packs (`createIfNotExist`), text edits (`add`, `insert`, `replace`, `delete` with Equal/StartWith/Mask) and XML edits by XPath (`add` First/Last/Before/After, `replace`, `remove`) that keep the file's formatting. Changes go to the `mods` folder; loose files that are new (such as `.asi` plugins) go to the game folder, as loaders expect them there.

Every installed package can be **uninstalled** from **History** (or `openrage oiv uninstall`) — OpenIV cannot do that. Not supported yet: `<pso>` edits (such packages are refused with an explanation). `<defragmentation>` is skipped, as it only saves disk space.

## Mods folder (OpenIV compatible)

OpenRAGE understands the `mods` folder used by OpenIV.asi, where a file at `mods/<path>` replaces `<path>` in the game. It never changes that layout — it only shows what it means:

- **Mods** (toolbar toggle) shows the game the way it sees itself: replaced files are marked *mod*, files only in `mods` *new*, and folders containing mods *has mods*.
- Inside a modded archive every folder is compared with the original: entries are marked *added* (green), *changed* (orange) or *removed* (red, shown greyed out with a preview of the original).
- **Changes** lists every difference of all mods at once, across nested archives — including complete copies of game archives that contain no change at all and can simply be deleted.

## GTA IV

GTA IV (the Complete Edition on Steam, with *The Lost and Damned* and *The Ballad of Gay Tony* in `TLAD` and `TBoGT`) keeps
most of its files in `.img` archives (IMG3) and a few in `.rpf` archives (RPF2; the audio archives are RPF3). Their tables —
and, in some RPF2 archives, the files — are encrypted with one AES key, which OpenRAGE finds in your own `GTAIV.exe` by its
SHA-1 fingerprint (a checksum only) and keeps in `gta4.keys` in your config folder. Choosing the game folder in the setup
(or `openrage --game <folder> keys`) does that in under a second; the Steam folder `Grand Theft Auto IV` and the `GTAIV`
folder inside it both work.

Archives then open like folders, read-only: file details, a text preview for plain files, the resource type and memory
sizes for resources, and *Extract…* for files, folders and whole archives. Files are saved as stored, like OpenIV does:
resources (`.wtd`, `.wdr`, `.wdd`, `.wft`, `.wbd`, `.wbn`, `.wad`, …) as RSC5 files with their header. RPF3 archives keep
only a hash of each name, so their entries show as hexadecimal numbers. `openrage verify` reads all 303 archives of the
game (46,889 resources inflated, no errors).

**Textures** (`.wtd`) open like GTA V's texture dictionaries, read-only: preview, viewer and export as DDS or PNG
(`openrage textures <path> --export <dir> [--png]`). GTA IV stores DXT1/3/5 and uncompressed 32-bit textures; names
drop the `pack:/` prefix and `.dds` suffix the game keeps. `openrage verify --textures` reads every dictionary of the game
(6,400 dictionaries, 70,845 textures, all decoded).

**Models** — drawables (`.wdr`), drawable dictionaries (`.wdd`) and fragments (`.wft`) — open in the preview and the 3D
viewer like GTA V's and export as glTF (`openrage model <path> [--export out.glb] [--render out.png]`). Vehicles show
their wheels on every wheel bone; peds (`componentpeds.img`) are put together from the first variant of each component
(head, upper body, legs, …) on their skeleton, the others can be switched on under *Parts*. Textures come from the
dictionary embedded in the model, the `.wtd` named after it, the dictionary the game's `.ide` files give the model (and
its parent dictionaries from `txdp` sections) and, for vehicles, `vehshare.wtd`; OpenRAGE indexes the `.ide` files and
the `.wtd` files of all archives once per session (a second or two), preferring an episode's own files for TLAD and TBoGT
models. `openrage verify --models` reads every model of the game (26,711 files, 46,658 drawables, 61 million triangles,
no errors).

**Texts** (`common/text/<language>.gxt`, and the episodes' own in `TLAD/` and `TBoGT/`) show in the preview and open in
a read-only window (double-click): every table as a `[NAME]` section of `0xHASH = text` lines, in the escapes of GTA V's
`.oxt` form (`openrage text <path> [-o file]`). **Placement files** (`.wpl`, on disk and inside the map archives) show as
IPL-style text: one line per object with position, rotation, model — by name, from the game's `.ide` files — flags and
LOD, then the other sections (garages, cull zones, blocks, …) field by field, as far as their meaning is known. `.ide`
and `.ipl` files are plain text already. **Scripts** (`.sco`) show their header (code size, variables, signature);
they cannot be disassembled yet. `openrage verify --text` reads every text table and checks that its text form converts
back identically (5,498 tables, 671,627 strings), every placement file (1,151 files, 458,048 records) and every script
header (941), with no errors.

**Maps**: double-click a placement file (`.wpl`) to see its objects in 3D, with the object list and glTF export of
GTA V's map scenes. A map archive (`pc/data/maps/…/<area>.img`) is mostly a set of streamed placement files;
`openrage map <archive>` puts all of them together (`--render`, `--export`), e.g. a whole part of Algonquin with
2,483 objects. Models come from the archive next to the placement file first, then from anywhere in the game (episode
files preferred for TLAD and TBoGT); many small objects are kept in drawable dictionaries (`.wdd`), which OpenRAGE
indexes once per session. Low-detail stand-ins (`lod…`, `slod…`) are left out unless a file holds nothing else.
Placement files store the inverse of each object's rotation, as GTA San Andreas and GTA V do. `openrage map --check`
counts, over all 1,151 placement files, how many of the 435,116 objects have a model: all but the 1,434 whose model
name no `.ide` file gives.

**Collisions** — `.wbn` (one bound, usually a piece of a map area) and `.wbd` (a dictionary of bounds, named after the
models they belong to) — open in the preview and the viewer like GTA V's `.ybn`, coloured by material with the names
from GTA IV's own `common/data/materials/materials.dat`, and export as glTF. They are the 32-bit ancestors of GTA V's
bounds: spheres, capsules, boxes, geometry with and without a BVH, and composites of these; polygons are quads or
triangles of quantised vertices, each with its material. `openrage verify --collisions` reads every collision file of
the game (1,079 files, 9,102 bounds, 26 million triangles, no errors). The collisions inside fragments (`.wft`) are
not read yet.

**Animations** (`.wad`, in `pc/anim/anim.img`, the cutscenes' in `cuts.img`) open like GTA V's clip dictionaries: one
clip per animation, with the ped models (`componentpeds.img`) to play it on — every GTA IV ped shares one skeleton —
and the viewer plays it (`openrage clips <path>`, `openrage model <ped> --anim <wad>:<clip>[@seconds] --render …`).
GTA IV stores every bone track as separate channels: raw floats, constants, quantised floats (bit-packed, scale and
offset) and constant quaternions or vectors; translations, rotations and the mover are read, other tracks (facial and
custom values) are skipped. The game turns its twist and roll helper bones (`Char_L_ForeTwist`, `L_ArmRoll`, …) itself;
OpenRAGE lets each follow the animated bone it rests on, so forearms and upper arms stay in one piece.
`openrage verify --animations` reads every animation of the game (1,772 dictionaries, 22,866 animations, 13.9 million
frames, 925,224 bone tracks, every frame evaluated, no errors). The search index is GTA V only for now.

## Red Dead Redemption 2

Setting up the keys is described in [Getting started](@/getting-started.md#set-up-red-dead-redemption-2). In both cases the cipher tables and the IV come from your `RDR2.exe`: OpenRAGE finds their 565 pieces by SHA-256 fingerprints (checksums only). All archive keys start with the same 32 bytes; every place in the dump or key file that starts that way is tried on your archives, and the keys that open them are kept. Only the Steam version of 2 April 2024 is known so far; other versions report which pieces are missing.

With the keys, `openrage rpf8 <game folder>` opens all 89 archives and the 4,874 archives nested in them, and reads all 493,592 files — decrypted, and decompressed with the open-source Oodle decoder [oozextract](https://github.com/lvlvllvlvllvlvl/oozextract). Entries carry only a hash of their name: `openrage rdr2-names <game folder>` searches the game's own files for matching strings (once, about 7 minutes) and caches the hits — 44 % of all names, and nearly all models (`.ydr`, `.ydd`, `.yft`), clip sets and particle files; the rest show as `hash/XXXXXXXX.ext`. With so many strings tried, some random letter runs match by chance; names that don't read like the game's other names are left out. `--import <list>` adds names from a text file (one per line).

Add the RDR2 folder as a game (File → Switch game… → *Add a game…*, after `rdr2-keys` and `rdr2-names`; with several games set up, OpenRAGE asks at start which one to open unless one is ticked as *Always open this game*): archives open like folders — named folders from the file paths, nested archives inside them — with file details, a text preview (XML and other text files, decrypted) and *Extract…* for files and whole folders. The CLI's `ls`, `info` and `extract` work on RDR2 paths too (`openrage --game <RDR2> ls levels_1.rpf/levels/rdr3`). **Textures** (`.ytd`) open like in GTA V: preview, viewer and export as DDS or PNG (`openrage textures <path> --export <dir> [--png]`). RDR2 stores each mip level in a padded grid of blocks; OpenRAGE repacks it, which `openrage rpf8 --textures <game folder>` checks over the whole game (417,028 textures, no errors). **Models** (`.ydr`, `.ydd`, `.yft`) open in the viewer and export as glTF, textured: RDR2 models mostly use shared texture dictionaries elsewhere in the game, which `openrage rdr2-names` indexes once (162,256 textures in 88,183 dictionaries) so the viewer finds them. `openrage rpf8 --models <game folder>` reads every model in the game. Fragment children (moving parts), skeleton animations and cloth are not read yet. **Texts** (`.yldb`, RDR2's counterpart of `.gxt2`) live in `data_0.rpf/data/lang/<language>_rel.rpf` — one table per mission, character or menu area, `global.yldb` for the shared strings — and in newer versions in `update_3.rpf`. They show in the preview and open in a read-only window (double-click) in the same `.oxt` text form as GTA V's (`0xHASH = text`); they cannot be saved back yet. `openrage rpf8 --text <game folder>` reads every one (55,237 tables, 5,404,020 strings in 13 languages, no errors). **Metadata** (`.ymap`, `.ytyp`, `.ymt`, `.ymf`, `.meta`) shows as XML in the preview and opens in the XML window like GTA V's — RDR2 uses the same three encodings (RSC meta resources, PSO, RBF) with its own changes: wider pointers, maps of key/value pairs, byte flags, bit sets and new hash types. `openrage rpf8 --meta <game folder>` converts every one (109,148 files; 14 parts in 8 files are not read, structures their own files don't describe). **Audio** lives in `x64/audio/sfx/*.rpf`; its containers (`.awc`, named by hash) open like folders with each sound to play and save as WAV, as in GTA V. RDR2 stores most sounds as Vorbis (decoded with [lewton](https://github.com/RustAudio/lewton)), cutscene dialogue and music as multi-channel streams. `openrage rpf8 --audio <game folder>/x64/audio` decodes every sound (438,265 sounds, 309 hours, no errors). **Animations** (`.ycd`, clip dictionaries of version 58 with their animations embedded) open like GTA V's: select a dictionary to list its clips, or paste its path (*Copy path*) into the viewer's animation search to play a clip on a model. Peds are put together from their parts and their base skeleton (`meta_base_<ped>.yft`). The sequences use GTA V's channels plus a tenth kind, decoded in part: per 64-frame chunk an exact start value, a mask of the groups of four frames that have keys (where the value changes) and one code per key; how the key values themselves are coded is not understood yet. Those channels are interpolated between the exact chunk starts across the frames that have keys and held elsewhere, so the timing of motion is right to four frames while fast motion within a chunk comes out softened (the viewer says so). `openrage rpf8 --anims <game folder>/anim_0.rpf` reads every clip dictionary (359,466 clips, 149,799 animations, every frame evaluated). **Peds** (people and animals, "metapeds") are put together from parts: `packs_0.rpf/packs/base/data/metapeds.ymt` lists every ped with its base skeleton, and each ped's outfit file (`…/data/metapeds/metapeds.rpf/<ped>.ymt`, newer copies in the updates and DLC packs) lists its outfits — per part a model, colour and normal textures, a material, a tint palette from `textures/graphics.ytd` and three palette rows, plus face shape values. The models sit in drawable dictionaries under hashes (often several in one `…_merge.ydd`), so OpenRAGE indexes the peds' 23,294 dictionaries once (half a minute, cached). `openrage --game <RDR2> rdr2-ped` lists the peds, `rdr2-ped cs_johnmarston [--outfit N]` shows a ped's outfits and where each part is, and `rdr2-ped --check` resolves every part of every ped (1,271 peds, 13,381 outfits, 122,703 parts; 7 references point to files the game does not have). A ped's outfit file opens like a model: the preview and the viewer show the ped put together from its parts on its base skeleton, with the outfit's colour and normal textures, and the viewer's *Parts* tab has an *Outfit* list to switch outfits (loaded in the background). The parts name their bones by tag (a table in each model, since most parts have no skeleton of their own), so clips play on the whole ped. Hair, eyes, many clothes and the animals' coats are coloured from tint palettes: their colour textures (flagged in `assets_albedo.ymt`) hold a shade in green and two masks in red and blue; the shade picks a column of the palette (`metaped_tint_*`, 64 × 256), each of the outfit's three tints a row, the first colouring everything and the others where their mask is set. OpenRAGE bakes these into ordinary textures when it puts the ped together, so the glTF export carries the colours too. Face and body shapes are applied too: an outfit's values (face features such as head width, body build, muscles) are inputs of the base skeleton's expressions (`anim/expressions.rpf/meta_base_<…>.yed`: `bcustom`, `meta_cust`, `muscles`), which OpenRAGE runs once and bakes into the parts. RDR2's expression programs use a changed instruction set (new jumps, registers and a weighted register blend); 1,536 of the game's 1,603 expressions are decoded, the rest use instructions not understood yet. Human heads are bound to face bones that no base skeleton has; OpenRAGE adds the 53 of them that one model in the game defines (the other 40 follow the head), so face shapes are approximate. `openrage model …/metapeds.rpf/<ped>.ymt [--outfit N] [--render out.png] [--export out.glb]` does the same on the command line, and `rdr2-ped --check --load` puts every ped together (all 1,271 peds; 133 of their 15,162 shaders miss their colour texture). The search index and the remaining format viewers are GTA V only for now.
