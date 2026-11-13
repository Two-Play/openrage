+++
title = "Red Dead Redemption 2"
description = "Red Dead Redemption 2 in OpenRAGE: setting up the keys from your own game, file names, archives, textures, models, peds, animations, metadata, audio and texts."

[extra]
docs = true
lead = "Explore RDR2 with keys from your own game. Read-only for now."
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
| Archives | `.rpf` (RPF8) | browse and extract |
| Textures | `.ytd` | view, export DDS/PNG |
| Models | `.ydr`, `.ydd`, `.yft` | view, export glTF |
| Peds and animals | `metapeds` outfits | put together with outfits, tints and face shapes |
| Animations | `.ycd` | play, partly approximated |
| Metadata | `.ymap`, `.ytyp`, `.ymt`, `.ymf`, `.meta` | view as XML |
| Audio | `.awc` | play, save WAV |
| Texts | `.yldb` | view |

Changing RDR2 files and the search index are not available yet.

## Archives

Archives open like folders — named folders from the file paths, nested archives inside them — with file details, a
text preview and *Extract…*. Files are decrypted and decompressed with the open-source Oodle decoder
[oozextract](https://github.com/lvlvllvlvllvlvl/oozextract). The command line works on RDR2 paths too:

```
openrage --game "/path/to/Red Dead Redemption 2" ls levels_1.rpf/levels/rdr3
```

**Checked:** all 89 archives, 4,874 nested archives and 493,592 files.

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
