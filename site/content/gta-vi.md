+++
title = "GTA VI"
description = "GTA VI in OpenRAGE: the map in 3D, RPF9 archives, models, textures, animations, sounds, videos and metadata. A read-only preview."
template = "gta-vi.html"

[extra]
# **bold** parts are shown in the accent colour.
eyebrow = "Read-only preview"
headline = "Explore **GTA VI**"
lead = "Open RPF9 archives like folders, search the whole game and look at what is inside, on Linux and Windows."
features_title = "What you can do"
features_intro = "Everything is read from your own copy of the game. Nothing in it is changed."
status_title = "At a glance"
status_intro = "What OpenRAGE reads today. Scripts (`.#script`), particle effects (`.#pt`) and texture volumes are not read yet."

[[extra.features]]
icon = "🗺️"
title = "The map in 3D"
text = "The whole world from afar; details load where you fly."

[[extra.features]]
icon = "🗂️"
title = "Archives"
text = "RPF9 archives, nested ones too, with folders and names."

[[extra.features]]
icon = "🚗"
title = "Models"
text = "Skeletons, textures, normal and specular maps, glTF export."

[[extra.features]]
icon = "🏃"
title = "Animations"
text = "Clips play on vehicles, weapons, props and parts of people."

[[extra.features]]
icon = "🔊"
title = "Sounds and videos"
text = "Sounds with WAV export; videos in a player of their own."

[[extra.features]]
icon = "🔎"
title = "Search"
text = "Names, hashes and full text of the whole game."

[[extra.formats]]
what = "Archives"
files = "`.rpf` (RPF9)"
status = "browse and extract"

[[extra.formats]]
what = "Maps"
files = "`.#map`"
status = "in 3D: a place with its surroundings, or the whole map; glTF export"

[[extra.formats]]
what = "Models"
files = "`.#dr`, `.#ft`, `.#dd`"
status = "view, export glTF"

[[extra.formats]]
what = "Animations"
files = "`.#cd`"
status = "play"

[[extra.formats]]
what = "Textures"
files = "`.ttpf`, `.#td`"
status = "view, export"

[[extra.formats]]
what = "Collisions, navigation meshes"
files = "`.#bn`, `.#nv`"
status = "view in 3D"

[[extra.formats]]
what = "Metadata, game text"
files = "`.#mt`, `.#typ`, `.#ldbmt` and others"
status = "read as XML, search"

[[extra.formats]]
what = "Sounds, videos"
files = "`.awc`, `.bk2`"
status = "play; export WAV"
+++

## Setup

Choose the folder that contains `game_scarlett_master.exe` or RPF9 archives in the setup dialog, or under
**File → Switch game… → Add a game…**.

With the supported executable beside the archives, OpenRAGE reads their tables of contents with the keys in it. The
game is not started, and the keys are not stored. Without the executable, or with another build, archives are shown in
a recovery view: their files are found by their content, grouped by type and offset, and without folders.

> [!NOTE]
> OpenRAGE contains no game files and no keys, and does not help to obtain either; you need your own copy.

## The map

**Tools → Map of the game (3D)…** shows the whole world. The search index must be complete; the first time, all map
files are read, which takes a minute or two. A single `.#map` opens as XML; **Show in 3D** there puts its objects
together with those around it. Interiors are not filled yet.

**Fly (F)** flies through it: W A S D move, Q and E go down and up, Shift is faster, dragging looks around and the wheel
sets the speed. In the whole map, details are loaded where the camera is, as the game's own levels of detail tell.

## Names

Archives store hashes of their files' names. OpenRAGE finds the names in the executable, in the game's metadata, in the
files themselves and, once the search index is built, in all the other archives. Files whose name is not known are
listed under `entries/`.

## Command line

```
openrage --game <GTA VI> ls common.rpf
openrage --game <GTA VI> index
openrage --game <GTA VI> search walk
openrage --game <GTA VI> map --render map.png
```
