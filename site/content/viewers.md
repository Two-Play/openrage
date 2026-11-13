+++
title = "Viewing files"
description = "What OpenRAGE shows for each GTA V format: textures, models, animations, audio, metadata as XML, maps, cutscenes, collisions, paths, particles, scripts, Scaleform movies, videos and game text."

[extra]
docs = true
lead = "Format by format: what you see, what you can export, and what is not there yet."
toc = true
+++

This page describes GTA V. [GTA IV](@/gta-iv.md) and [Red Dead Redemption 2](@/rdr2.md) have their own pages.
Changing files is described in [Modding](@/modding.md).

## Textures

Texture dictionaries (`.ytd`) open like folders: double-click one to list its textures with size, format and mip levels.

### View and export

- Selecting a texture shows it in the preview; selecting a `.ytd` previews its first texture.
- **Extract…** writes DDS files exactly like OpenIV and CodeWalker (DX10 headers for BC7 and other modern formats).
- **Save PNG…** writes images.
- With nothing selected, the whole dictionary is exported into a folder named after it.

### Supported layouts

| Game version | Resource version | Formats |
|---|---|---|
| GTA V Legacy | 13 | Direct3D 9 formats (DXT1/3/5, …) |
| GTA V Enhanced | 5 | DXGI formats, including BC7 |

Replacing, adding and removing textures: see [Modding](@/modding.md#textures).

## Models

Drawables (`.ydr`), fragments such as vehicles (`.yft`) and drawable dictionaries (`.ydd`) show in 3D: drag to
rotate, scroll to zoom.

### Complete models

- **Vehicles** come with their wheels on every wheel bone and their liveries.
- **Peds** are put together from their components (head, hair, upper body, legs, accessories), the first variant of each.
- **Textures** come from the model itself, the `.ytd` next to it (`adder.ytd`, `adder+hi.ytd`) and, for vehicles,
  `vehshare.ytd`.

### The viewer window

Double-click a model to open it in the viewer. It renders on the GPU (Vulkan, DirectX 12, Metal or OpenGL) with a
software fallback.

| Tab or control | What it does |
|---|---|
| Orbit / **Fly** (F) | Drag and scroll, or fly with W A S D, Q/E for down/up, Shift for faster |
| **Parts** | Show or hide groups (body, wheels, ped components and their variants) or single parts |
| **Materials** | Every shader with a readable name; hide it or give it a colour — paint the car |
| **Animation** | Play clips from clip dictionaries: search, play, scrub, speed, loop |
| Compass | The game's X/Y/Z axes; click a letter to look along that axis |

### Export

**Export glTF…** writes a binary glTF 2.0 file (`.glb`) with one node per shown part and the textures embedded.
Blender and most other 3D tools open it directly.

### Not yet

Vehicle mods and tuning parts.

## Animations

Clip dictionaries (`.ycd`) show what they animate (peds, vehicles, objects) and a list of fitting models, ranked by the
bones they share with the clips. Click one, or double-click the file, to open it in the viewer with the animation playing.

In the viewer, peds, vehicles (convertible roofs, doors, …) and props play clips; search for a dictionary such as
`move_m@generic`. Skinning runs on the GPU.

## Audio

Audio containers (`.awc`) open like folders: effects and dialogue list their sounds, music and radio show one
multi-channel stream.

### Play and save

- Selecting a sound shows its waveform, length, sample rate and codec.
- **Play** (or double-click, or Space) plays it; clicking the waveform jumps.
- **Save WAV…** and **Extract…** write 16-bit `.wav` files, like OpenIV.

### Supported

PCM and IMA ADPCM (GTA V Legacy), MPEG audio (GTA V Enhanced), single sounds and multi-channel streams, and Rockstar's
audio encryption. The audio key comes from the key blob used during setup.

## Metadata (XML)

Map placements (`.ymap`), archetypes (`.ytyp`), `.ymt`, manifests (`.ymf`), cutscenes (`.cut`) and `.pso` files show
as XML, formatted like CodeWalker's export. Double-click (or **Show as XML**) opens a window with two views.
Files that are XML text already (`handling.meta`, `vehicles.meta`, `.xml`, …) open in the same window.

### Structure

The file as a tree — entities, archetypes, cutscene objects — labelled with type and name
(`CEntityDef #3 · dt1_tc_ufocore`, `CHandlingData #2 · ADDER`). The selected node's values show as a table with readable names
(`fTotalDuration` → *Total duration*). A filter finds names and values anywhere in the file.

### XML

The XML with syntax colours and line numbers. **Open as text** shows converted XML in the text editor for copying;
for XML text files the button reads **Edit as text** and opens the file itself for editing. **Export XML…** saves
`name.ymap.xml`.

### Names and encodings

- All three binary encodings are read: RSC meta resources, PSO and RBF.
- Hashes show as names where the name is known; unknown ones as `hash_XXXXXXXX`.
- Saving XML back into the game files will come later.

## Maps

A map placement file (`.ymap`) opens in the metadata window with **Show in 3D**: every object the file places, with its
model and textures, interiors included.

- The detailed objects of an area are in its `…_strm_N.ymap` files; the others hold distant stand-ins.
- The list on the right names every model with how often it is placed, to show or hide it.
- **Export glTF…** writes the shown objects as one `.glb` scene.

## Cutscenes

A cutscene (`.cut`) opens in the metadata window with a **Cutscene** tab: its length and its cast — people, vehicles,
props, the camera, lights and audio.

### Play the whole scene

**Play whole cutscene** shows every actor with its animation, where the cutscene puts it:

- through the cutscene camera, with its field of view, or with a free camera (drag or scroll);
- with play, pause and scrubbing;
- with the map around it — buildings, interiors and props in reach — lit by the cutscene's lights.

**Play animation** opens a single actor in the viewer instead.

### Not shown

Particle effects (smoke, sparks) and decals (dirt and stains). Without a GPU the scene is drawn in software, smaller
and without lights.

## Collisions, paths and nav meshes

| Files | What you see |
|---|---|
| Collisions (`.ybn`) | Meshes, spheres, capsules, boxes and cylinders, coloured by material (CONCRETE, METAL, GRASS, …) |
| Path nodes (`.ynd`) | The road (blue) and footpath (orange) network of a map area |
| Recordings (`.yvr`, `.ywr`) | Routes cars drive and people walk in missions, with start and end |
| Nav meshes (`.ynv`) | Where peds can walk, coloured by ground, pavement, road, water, interiors, tracks, slopes |

All open in the viewer. The **Materials** tab hides or recolours each kind; collisions export to glTF.

## Particles

Particle dictionaries (`.ypt`) list their effects (such as `bul_gravel_heli`), emitters and particle rules. Their
models — debris, shells, bottles — show side by side; double-click the file to browse its textures.

## Scripts

Compiled scripts (`.ysc`) show their size, functions, natives and strings. Double-click opens the disassembly: one
instruction per line, functions, labels at jump targets, text next to every `STRING` and the hash of every native call.

Native names appear once you load a list in the NativeDB format (`natives.json`): **Help → Load native names…**.
OpenRAGE ships none.

## Scaleform movies and videos

- **Scaleform movies** (`.gfx`, menus and HUD) show their size, frame rate, exported symbols and images.
  **Export as SWF…** turns one into a Flash file for tools such as JPEXS or Ruffle.
- **Bink videos** (`.bik`) show size, frame rate, length and audio tracks, and with FFmpeg installed a frame from the
  middle. Double-click plays the video in your video player (VLC, mpv, …).

## Text tables

- **Game text** (`.gxt2`) opens in the text editor in OpenIV's `.oxt` form: one `0xHASH = text` line per string.
- **Audio name tables** (`.nametable`) show one name per line; `.ugc` files open as JSON.
- **Export as text…** writes `.oxt` or `.txt`. Saving back is described in [Modding](@/modding.md#text-files-and-game-text).
