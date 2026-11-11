+++
# Landing page. Edit the text here; the layout is in templates/index.html.
# TOML note: plain keys must stay above the first [table] / [[list]] entry.
# Strings are Markdown (links, `code`, **bold**). Images are file names in docs/images/.
title = "OpenRAGE"
description = "OpenRAGE is a free, open-source explorer and modding toolkit for Rockstar's RAGE engine games on Linux and Windows: archives, textures, models, audio, animations, metadata and OpenIV-compatible mods."

[extra]
headline = "Explore and mod RAGE engine games"
lead = """
OpenRAGE is a free, open-source explorer and modding toolkit for Rockstar's RAGE engine games, on Linux and Windows.
Browse and search every archive, preview textures, models, sounds and animations, read metadata as XML,
edit game text and install `.oiv` packages — compatible with OpenIV's `mods` folder, with undo.
"""
notice = """
OpenRAGE is an independent community project. It is not affiliated with or endorsed by Rockstar Games or Take-Two Interactive.
It contains **no game files and no encryption keys**: keys are taken from your own copy of the game at runtime.
"""

# Section titles and intros
features_title = "Highlights"
features_intro = "Everything you need to look inside the game and change it safely — from the explorer or the command line."
screenshots_title = "Screenshots"
screenshots_intro = "The screenshots show OpenRAGE displaying files from the author's own copy of the game."
games_title = "Supported games"
games_intro = "GTA V comes first; Red Dead Redemption 2 archives can be read, GTA IV is planned."
details_summary = "Checked against the whole game"
download_intro = "Unpack the archive and run `openrage-gui` (or `openrage` for the command line). On first start the GUI detects your game and sets up the keys."

[extra.hero]
light = "explorer-light.png"
dark = "explorer-dark.png"
width = 1512
height = 945
alt = "OpenRAGE explorer showing the vehicles archive of GTA V with a 3D preview of a car"

# ---------------------------------------------------------------- Highlights

[[extra.features]]
icon = "🗂️"
title = "Explorer"
text = "Archives — even nested ones — behave like folders. Breadcrumbs, format icons, context menus, keyboard navigation, light and dark mode."

[[extra.features]]
icon = "🔎"
title = "Search everything"
text = "Names, hashes (`0xB779A091` → `adder`) and full text of the whole game in milliseconds, from an index that updates itself."

[[extra.features]]
icon = "🖼️"
title = "Textures"
text = "All formats of GTA V Legacy and Enhanced (incl. BC7); DDS/PNG export identical to OpenIV; replace, add and remove."

[[extra.features]]
icon = "🚗"
title = "Models"
text = "Complete vehicles and peds in a GPU viewer: orbit or fly, show/hide parts, paint materials, export glTF for Blender."

[[extra.features]]
icon = "🔊"
title = "Audio"
text = "Plays every sound and radio track of the game, with waveform, seeking and WAV export."

[[extra.features]]
icon = "🏃"
title = "Animations"
text = "Clip dictionaries play on skinned models; fitting models are suggested from the bones a clip moves."

[[extra.features]]
icon = "📄"
title = "Metadata"
text = "`.ymap`, `.ytyp`, `.ymt`, `.ymf`, `.cut` and `.pso` as XML (RSC meta, PSO and RBF), compatible with CodeWalker."

[[extra.features]]
icon = "🎬"
title = "Cutscenes"
text = "Play a whole cutscene: every actor animated in one scene, through the cutscene camera or a free camera."

[[extra.features]]
icon = "🧱"
title = "Collisions, paths, nav meshes"
text = "`.ybn`, `.ynd`, `.ynv`, `.yvr` and `.ywr` in 3D, coloured by material or kind."

[[extra.features]]
icon = "📜"
title = "Scripts and particles"
text = "Disassembles compiled scripts (`.ysc`); lists effects, textures and models of particle dictionaries (`.ypt`)."

[[extra.features]]
icon = "🌐"
title = "Game text"
text = "Edit `.gxt2` text tables in OpenIV's `.oxt` form and save them back losslessly."

[[extra.features]]
icon = "🧩"
title = "Mods, safely"
text = "Changes go to OpenIV's `mods` folder, never into your game files; every change and every `.oiv` package can be undone."

# --------------------------------------------------------------- Screenshots
# `dark` (optional) is shown instead of `image` in dark mode.

[[extra.screenshots]]
image = "viewer.png"
width = 1155
height = 798
alt = "Viewer window with a car painted blue and its list of materials"
title = "Model viewer"
caption = "materials, paint, parts and animations"

[[extra.screenshots]]
image = "metadata.png"
width = 1512
height = 945
alt = "Archetype file of a map shown as XML in the preview"
title = "Metadata as XML"
caption = "maps, archetypes, cutscenes, …"

[[extra.screenshots]]
image = "text-table.png"
width = 1512
height = 945
alt = "Game text table shown as OpenIV oxt text"
title = "Game text"
caption = "`.gxt2` as editable `.oxt`"

[[extra.screenshots]]
image = "explorer-dark.png"
dark = "explorer-light.png"
width = 1512
height = 945
alt = "OpenRAGE explorer in dark mode"
dark_alt = "OpenRAGE explorer in light mode"
title = "Light and dark mode"
caption = "follows the system or your choice"

# ---------------------------------------------------------- Supported games
# Status: ok, partial, planned or none.

[extra.status_labels]
ok = "✓ Supported"
partial = "◐ Read"
planned = "○ Planned"
none = "– Not yet"

[[extra.games]]
name = "GTA V"
variant = "Legacy & Enhanced"
archives_status = "ok"
archives = "RPF7 read, incl. nested archives; edit in place in the mods folder, with undo"
formats_status = "ok"
formats = "textures · models · audio · animations · metadata · text tables · cutscenes · scripts · collisions · paths"

[[extra.games]]
name = "Red Dead Redemption 2"
archives_status = "partial"
archives = "RPF8 read: decryption with keys from your own game, Oodle decompression (command line)"
formats_status = "none"

[[extra.games]]
name = "GTA IV"
archives_status = "planned"
archives = "RPF3"
formats_status = "none"

+++

<!-- This text is shown under "Checked against the whole game" in the Supported games section. -->

Every format is verified against the complete game (GTA V Enhanced):

| Format | Files checked | Result |
|---|---|---|
| Textures (`.ytd`) | 89,817 dictionaries, 341,701 textures | all decode; every dictionary rebuilds and reads back identically |
| Models (`.ydr`, `.yft`, `.ydd`) | 168,779 models | all load (583 million triangles) |
| Audio (`.awc`) | 7,758 containers, 252,742 sounds | all decode (279.8 hours) |
| Animations (`.ycd`) | 25,961 dictionaries, 332,771 clips | all evaluate |
| Metadata (`.ymap`, `.ytyp`, `.ymt`, `.ymf`, `.cut`, `.pso`) | 30,280 files | all convert to XML |
| Text tables (`.gxt2`, `.nametable`) | 19,858 files, 9.16 million strings | text → binary is byte-identical |
| Particles (`.ypt`) | 1,544 dictionaries, 12,987 effects | all read, incl. 1,694 textures and 1,223 models |
| Collisions (`.ybn`) | 14,692 files, 70,454 bounds | all read (101 million triangles, 4.7 million shapes) |
| Paths (`.ynd`, `.yvr`, `.ywr`) | 9,922 files, 232,982 nodes, 1.37 million recorded points | all read (521 hours of vehicle recordings) |
| Nav meshes (`.ynv`) | 10,567 files, 18.9 million polygons | all read |
| Scripts (`.ysc`) | 1,864 scripts, 383 million instructions | all disassemble, every jump lands on an instruction |
| Scaleform movies (`.gfx`), Bink videos (`.bik`) | 1,163 movies, 415 videos | all read (29,490 exported symbols, 207 minutes of video) |
