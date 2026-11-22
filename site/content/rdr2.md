+++
title = "Red Dead Redemption 2"
description = "Red Dead Redemption 2 in OpenRAGE: setting up the keys from your own game, file names, archives, changing files through Lenny's Mod Loader, textures, models, peds, animations, metadata, audio and texts."

[extra]
docs = true
lead = "Explore and mod RDR2 with keys from your own game. Changes go into the replace folder of Lenny's Mod Loader, never into the game's archives."
toc = true
+++

## Setup

RDR2's keys are set up on the **command line** for now; after that you browse the game in the GUI. It takes three steps.

### 1. Get the keys

Unlike GTA V's, most RDR2 archive keys are not stored in the executable: they exist only while the game runs. OpenRAGE
takes them from your own game and never ships them. Choose one way:

**From a key file** — length-prefixed key blocks, such as the `secrets.bin` Swage writes:

```
openrage rdr2-keys --file secrets.bin "/path/to/Red Dead Redemption 2"
```

**From a memory dump of your running game:**

1. Start RDR2 in story mode (not Red Dead Online) and wait for the main menu, or load a save.
2. Dump the process. Windows: Task Manager → Details → right-click `RDR2.exe` → *Create dump file*.
   Proton/Linux: `gcore <pid of RDR2.exe>` (from gdb).
3. Run `openrage rdr2-keys --dump <dump file> "/path/to/Red Dead Redemption 2"`.

> [!WARNING]
> **Delete the dump afterwards.** It is several gigabytes and holds everything the game had in memory, including login data.

The keys are stored in OpenRAGE's config folder (`rdr2.keys`).

### 2. Find the file names

RDR2 archives store only a hash of each file name. Let OpenRAGE search the game's own files for matching names once
(about 7 minutes, cached):

```
openrage rdr2-names "/path/to/Red Dead Redemption 2"
```

This finds about 44 % of all names, including nearly all models, clip sets and particle files; the rest show as
`hash/XXXXXXXX.ext`. `--import <list>` adds names from a text file.

### 3. Add the game

In the GUI: **File → Switch game… → Add a game…** and choose the RDR2 folder.

### Supported versions

Only the Steam version of 2 April 2024 is known so far; other versions report which pieces are missing.

## At a glance

| What | Files | Status |
|---|---|---|
| Archives | `.rpf` (RPF8) | browse, extract, change (into `lml/replace`, with undo) |
| Textures | `.ytd` | view, export DDS/PNG, replace (same format, size and mips) |
| Models | `.ydr`, `.ydd`, `.yft` | view, export glTF |
| Peds and animals | `metapeds` outfits | put together with outfits, tints and face shapes |
| Animations | `.ycd` | play, partly approximated |
| Metadata | `.ymap`, `.ytyp`, `.ymt`, `.ymf`, `.meta` | view as XML |
| Audio | `.awc` | play, save WAV |
| Texts | `.yldb` | view |

The search index is not available for RDR2 yet.

## Archives

Archives open like folders — named folders from the file paths, nested archives inside them — with file details, a
text preview and *Extract…*. Files are decrypted and decompressed with the open-source Oodle decoder
[oozextract](https://github.com/lvlvllvlvllvlvl/oozextract). The command line works on RDR2 paths too:

```
openrage --game "/path/to/Red Dead Redemption 2" ls levels_1.rpf/levels/rdr3
```

**Checked:** all 89 archives, 4,874 nested archives and 493,592 files.

## Changing files

RDR2 has no OpenIV.asi. Its mods are loaded by **[Lenny's Mod Loader](https://www.rdr2mods.com/downloads/rdr2/tools/76-lennys-mod-loader-rdr/)**
(LML), and OpenRAGE writes for it: its **replace folder** loads `lml/replace/<archive>/<path>` in place of the file
`<path>` inside the game's `<archive>.rpf`. The game's archives are never changed. Install LML (with its
`vfs.asi`) to see the changes in the game.

| You change | OpenRAGE writes |
|---|---|
| a file inside a game archive, e.g. `update_4.rpf/x64/data/ui/blipdata.ymt` | that file, loose: `lml/replace/update_4/x64/data/ui/blipdata.ymt` |
| anything inside a nested archive, e.g. `update_4.rpf/x64/data/ui/startup_data.rpf/settings/…` | the nested archive, copied there once and then edited in place: `lml/replace/update_4/x64/data/ui/startup_data.rpf` |

- **Edit mode** works as for GTA V: unlock editing, then **Import…**, **Replace…**, **Rename…** and **Delete** in an
  archive's folder, or **Replace…** on a texture. The command line has `put`, `rm`, `mv`, `history` and `undo`.
- **Delete and Rename** work inside nested archives (and on files OpenRAGE put into `lml/replace`). LML cannot hide or
  rename a file of the game's own archives.
- **Undo** and **History** work as for GTA V; the journal is in `lml/replace/.openrage`.
- **Names:** LML finds a replacement by the file's real path, so files that show only as `hash/XXXXXXXX` cannot be
  replaced at the top of an archive (inside nested archives they can). Run `rdr2-names` first; OpenRAGE also tries the
  `x64/…` form of every name it found, which names most files of the `update_*.rpf` archives. Names given to new or
  renamed entries are kept in `lml/replace/.openrage/names.txt`.
- **Resources** (textures, models and other `RSC8` files) are replaced with an RSC8 file: *Extract…* saves resources
  in that form (header and pages, uncompressed), so an extracted file can go back after editing.
- **Textures:** a DDS of the same format, size and mip count is written into the dictionary in place (RDR2's padded
  layout). Adding and removing textures, or other sizes, are not supported yet.

```
openrage --game <RDR2> put blipdata.ymt update_4.rpf/x64/data/ui/blipdata.ymt
openrage --game <RDR2> put flow.ymt update_4.rpf/x64/data/ui/startup_data.rpf/settings/flow/settings_group_flow.ymt
openrage --game <RDR2> history
openrage --game <RDR2> undo
```

### How nested archives are written

OpenRAGE's RPF8 writer edits an archive in place, like the GTA V editor: new data is appended, the header and entry
table at the start are rewritten, and entries in the way of a grown table move to the end unchanged. In detail:

- The entry table is written **unencrypted** (the header's decryption tag says so). RDR2's cipher can only be
  decrypted with the game's keys, not encrypted again. The game reads plain tables: a public mod (DualSense for RDR2)
  ships a nested archive rebuilt this way through LML's replace folder.
- Untouched entries keep their stored bytes, still encrypted, compressed and signed. New data is stored as is: not
  encrypted, not compressed, not signed.
- The signature block after the header is kept as it was; it no longer matches the changed table.
- The archive's own name table is rewritten when it is readable and kept byte for byte otherwise.

```
openrage --game <RDR2> verify --rebuild            # every archive: edit a copy, read it back, undo
```

**Checked:** `verify --rebuild` over the whole game (Steam version, November 2026): 4,905 archives edited and undone
(467,546 entries, every nested archive included) and 81,021 texture dictionaries (379,191 textures) written in place
and read back, 0 failed. 58 archives over 512 MB were not copied whole; their nested archives were checked.

**Not checked in the game:** these changes were tested in OpenRAGE only, not in RDR2 itself. In particular, whether
LML picks up files from `lml/replace` for archives in subfolders (`x64/dlcpacks/…`, `x64/audio/sfx/…`), whether the
game accepts unencrypted, unsigned new entries of every file type (one public mod reports that the game crashed on an
uncompressed new UI scene and needed Oodle compression, which OpenRAGE cannot write yet), and whether the game takes
a changed nested archive's table from the file or from its packfile cache (`pfm.dat`).

## Textures

Texture dictionaries (`.ytd`) open like GTA V's: preview, viewer and export as DDS or PNG. RDR2 stores each mip level
in a padded grid of blocks, which OpenRAGE repacks.

**Checked:** 417,028 textures, no errors.

## Models

Drawables, drawable dictionaries and fragments open in the viewer and export as glTF, textured. RDR2 models mostly use
shared texture dictionaries elsewhere in the game; `rdr2-names` indexes them once (162,256 textures) so the viewer
finds them.

**Not yet:** moving parts of fragments, skeleton animations of models and cloth.

## Peds and animals

RDR2's people and animals ("metapeds") are put together from parts. A ped's outfit file
(`…/metapeds.rpf/<ped>.ymt`) opens like a model.

### Outfits

- Every ped has a base skeleton and a list of outfits; each outfit names a model, textures and colours per part.
- The viewer's *Parts* tab has an **Outfit** list to switch outfits.
- Clips play on the whole ped.

### Colours and shapes

- **Tints:** hair, eyes, many clothes and animal coats are coloured from tint palettes. OpenRAGE bakes them into
  ordinary textures, so the glTF export carries the colours.
- **Face and body shapes:** an outfit's values (head width, build, muscles) drive the skeleton's expressions, which
  OpenRAGE runs and bakes into the parts. Faces are approximate: 1,536 of 1,603 expressions are decoded.

### Command line

```
openrage --game <RDR2> rdr2-ped                         # list the peds
openrage --game <RDR2> rdr2-ped cs_johnmarston          # outfits and parts of one ped
openrage model …/metapeds.rpf/<ped>.ymt --outfit 2 --export ped.glb
```

**Checked:** every part of every ped (1,271 peds, 13,381 outfits, 122,703 parts) resolves; all peds load.

## Animations

Clip dictionaries (`.ycd`) open like GTA V's: select one to list its clips, or paste its path into the viewer's
animation search to play a clip on a model.

RDR2 uses one kind of animation data whose key values are not fully understood yet. Those tracks are interpolated, so
the timing is right to four frames but fast motion comes out softened; the viewer says so.

**Checked:** 359,466 clips, every frame evaluated.

## Metadata

Map placements, archetypes and other metadata (`.ymap`, `.ytyp`, `.ymt`, `.ymf`, `.meta`) show as XML, like GTA V's.
RDR2 uses the same three encodings with its own changes: wider pointers, key/value maps, byte flags, bit sets and new
hash types.

**Checked:** 109,148 files convert; 14 parts in 8 files are not read.

## Audio

Sound containers (`.awc`, in `x64/audio/sfx/*.rpf`) open like folders, with each sound to play and save as WAV. Most
sounds are Vorbis (decoded with [lewton](https://github.com/RustAudio/lewton)); cutscene dialogue and music are
multi-channel streams.

**Checked:** 438,265 sounds, 309 hours, no errors.

## Texts

RDR2's game text (`.yldb`, its counterpart of `.gxt2`) lives in `data_0.rpf/data/lang/<language>_rel.rpf`, one table per
mission, character or menu area. Tables open in a read-only window in the same `.oxt` form as GTA V's
(`0xHASH = text`); they cannot be saved back yet.

**Checked:** 55,237 tables, 5.4 million strings in 13 languages, no errors.

## How the keys are found

For the curious: the cipher tables and the IV come from your `RDR2.exe`. OpenRAGE finds their 565 pieces by SHA-256
fingerprints (checksums only). All archive keys start with the same 32 bytes; every place in the dump or key file that
starts that way is tried on your archives, and the keys that open them are kept.
