+++
# Landing page. Edit the text here; the layout is in templates/index.html.
# TOML note: plain keys must stay above the first [table] / [[list]] entry.
# Strings are Markdown (links, `code`, **bold**). Images are file names in docs/images/.
title = "OpenRAGE"
description = "OpenRAGE is a free, open-source explorer and modding toolkit for Rockstar's RAGE engine games on Linux and Windows: archives, textures, models, audio, animations, metadata and OpenIV-compatible mods."

[extra]
headline = "Explore and mod RAGE engine games"
# One short line under the headline; the longer description belongs in features_intro.
lead = "Free, open-source explorer and modding toolkit for Rockstar's RAGE engine games."
notice = """
OpenRAGE is an independent community project. It is not affiliated with or endorsed by Rockstar Games or Take-Two Interactive.
It contains **no game files and no encryption keys**: keys are taken from your own copy of the game at runtime.
"""

# Section titles and intros
features_title = "Highlights"
features_intro = "What OpenRAGE can open, show and change today."
screenshots_title = "Screenshots"
screenshots_intro = "The screenshots show OpenRAGE displaying files from the author's own copy of the game."
games_title = "Supported games"
games_intro = "GTA V comes first; Red Dead Redemption 2 archives and textures work, GTA IV is planned."
details_summary = "Checked against the whole game"
download_intro = "Unpack the archive and run `openrage-gui` (or `openrage` for the command line). On first start the GUI detects your game and sets up the keys."
why_title = "Why OpenRAGE"
why_intro = "OpenRAGE keeps what OpenIV got right — the `mods` folder, `.oiv` packages, the export formats — and adds this."
compare_title = "OpenRAGE and OpenIV side by side"
compare_note = """
OpenIV supports more games and can import models; both tools share the same `mods` folder, so you can use them side by side.
Out of date? [Open an issue](https://github.com/Two-Play/openrage/issues).
"""


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
text = "Archives, even nested ones, open like folders."

[[extra.features]]
icon = "🔎"
title = "Search"
text = "Names, hashes and full text of the whole game in milliseconds."

[[extra.features]]
icon = "🖼️"
title = "Textures"
text = "Every GTA V format incl. BC7: view, export, replace, add, remove."

[[extra.features]]
icon = "🚗"
title = "Models and animations"
text = "Complete vehicles and peds on the GPU, animated, with glTF export."

[[extra.features]]
icon = "🔊"
title = "Audio"
text = "Every sound and radio track, with waveform and WAV export."

[[extra.features]]
icon = "📄"
title = "Metadata and game text"
text = "Maps and archetypes as XML; `.gxt2` text editable without loss."

[[extra.features]]
icon = "🎬"
title = "Cutscenes"
text = "Whole cutscenes in one scene, seen through the cutscene camera."

[[extra.features]]
icon = "🧱"
title = "Collisions and paths"
text = "Collisions, road networks and nav meshes in 3D."

[[extra.features]]
icon = "📜"
title = "Scripts and particles"
text = "Disassembled scripts; particle effects with their textures and models."

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
archives_status = "ok"
archives = "RPF8 read: decryption with keys from your own game, Oodle decompression, browsing and extracting in the GUI"
formats_status = "partial"
formats = "textures (preview, viewer, DDS/PNG export); models and other formats follow"

[[extra.games]]
name = "GTA IV"
archives_status = "planned"
archives = "RPF3"
formats_status = "none"

# ------------------------------------------------------------- Why OpenRAGE

[[extra.reasons]]
icon = "🔓"
title = "Open source"
text = "GPL-3.0: read, check and improve the code."

[[extra.reasons]]
icon = "🐧"
title = "Linux and Windows"
text = "Native on both, Steam and Proton installs included."

[[extra.reasons]]
icon = "🛠️"
title = "Actively developed"
text = "Supports GTA V Enhanced. OpenIV's last release, 4.1, is from January 2022."

[[extra.reasons]]
icon = "↩️"
title = "Undo everything"
text = "Preview what an `.oiv` changes, undo any edit, uninstall packages."

[[extra.reasons]]
icon = "✅"
title = "Tested on the whole game"
text = "Every format is checked against every file of the game."

[[extra.reasons]]
icon = "🔁"
title = "No lock-in"
text = "Same `mods` folder, `.oiv` format and export output as OpenIV."

# Comparison table. `edge` marks the cell that is ahead: openrage, openiv or empty.

[[extra.compare]]
feature = "Price"
openiv = "Free"
openrage = "Free"

[[extra.compare]]
feature = "Source code"
openiv = "Closed"
openrage = "Open, GPL-3.0"
edge = "openrage"

[[extra.compare]]
feature = "Operating systems"
openiv = "Windows"
openrage = "Linux and Windows"
edge = "openrage"

[[extra.compare]]
feature = "Last release"
openiv = "4.1, January 2022"
openrage = "Actively developed"
edge = "openrage"

[[extra.compare]]
feature = "GTA V Enhanced"
openiv = "Released after OpenIV's last update"
openrage = "Supported"
edge = "openrage"

[[extra.compare]]
feature = "Other games"
openiv = "GTA IV, EFLC, Max Payne 3"
openrage = "RDR2 archives and textures; GTA IV planned"
edge = "openiv"

[[extra.compare]]
feature = "OpenIV `mods` folder"
openiv = "Yes"
openrage = "Yes, same layout"

[[extra.compare]]
feature = "`.oiv` packages"
openiv = "Install"
openrage = "Install, preview, uninstall"
edge = "openrage"

[[extra.compare]]
feature = "Undo"
openiv = "No"
openrage = "Every change"
edge = "openrage"

[[extra.compare]]
feature = "Search"
openiv = "File names"
openrage = "Names, hashes, full text"
edge = "openrage"

[[extra.compare]]
feature = "Model export"
openiv = "openFormats"
openrage = "glTF (Blender)"

[[extra.compare]]
feature = "Model import"
openiv = "Yes"
openrage = "Not yet"
edge = "openiv"

[[extra.compare]]
feature = "Cutscenes"
openiv = "Files only"
openrage = "Play the whole scene"
edge = "openrage"

[[extra.compare]]
feature = "Command line"
openiv = "No"
openrage = "Yes"
edge = "openrage"
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
