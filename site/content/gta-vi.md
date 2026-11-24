+++
title = "GTA VI"
description = "GTA VI in OpenRAGE: browse and search RPF9 archives, view textures, models, animations, collisions, navigation meshes and metadata, play sounds and videos."

[extra]
docs = true
lead = "Open GTA VI's RPF9 archives like folders, search the whole game and look at what is inside: models, textures, animations, sounds, videos and metadata. Read-only for now."
toc = true
+++

> [!NOTE]
> GTA VI support is a preview. It reads the build for Xbox Series (Scarlett) and changes nothing: archives cannot be
> edited. OpenRAGE contains no game files and no keys, and does not help to obtain either; you need your own copy.

## Setup

Choose the folder that contains `game_scarlett_master.exe` or RPF9 archives in the setup dialog, or under
**File → Switch game… → Add a game…**.

With the supported executable beside the archives, OpenRAGE reads their tables of contents with the keys in it. The
game is not started, and the keys are not stored. Without the executable, or with another build, archives are shown in
a recovery view: their files are found by their content, grouped by type and offset, and without folders.

On the command line:

```
openrage --game <GTA VI> ls common.rpf
openrage --game <GTA VI> index
openrage --game <GTA VI> search walk
```

## At a glance

| What | Files | Status |
|---|---|---|
| Archives | `.rpf` (RPF9) | browse and extract, nested archives too |
| Models | `.#dr`, `.#ft`, `.#dd` | view with skeleton, textures, normal and specular maps, export glTF |
| Maps | `.#map` | a place with its surroundings, or the whole map from afar, in 3D; export glTF |
| Animations | `.#cd` | play on vehicles, weapons, props and parts of people |
| Textures | `.ttpf`, `.#td` | view and export; volumes are not read |
| Collisions | `.#bn` | view in 3D |
| Navigation meshes | `.#nv` | view in 3D |
| Metadata, maps, archetypes | `.#mt`, `.#map`, `.#typ`, `.#as`, `.#cov`, `.#kit` and others | read as XML |
| Game text | `.#ldbmt` | read and search |
| Sounds | `.awc` | play, export WAV |
| Videos | `.bk2` | play in a window of their own |
| Search | everything above | names, hashes and full text |

Not read yet: scripts (`.#script`), particle effects (`.#pt`) and texture volumes.

## The map

**Tools → Map of the game (3D)…** shows the whole world from afar. The search index must be complete; the first time,
all map files are read, which takes a minute or two. A single `.#map` opens as XML; **Show in 3D** there puts its objects
together with those around it.

**Fly (F)** flies through it: W A S D move, Q and E go down and up, Shift is faster, dragging looks around and the wheel
sets the speed. In the whole map, details are loaded where the camera is, as the game's own levels of detail tell.
 Interiors are not filled yet.

On the command line: `openrage map --render map.png`, or `openrage map <path of a .#map>`.

## Names

Archives store hashes of their files' names. OpenRAGE finds the names in the executable, in the game's metadata, in the
files themselves and, once the search index is built, in all the other archives. Files whose name is not known are
listed under `entries/`.
