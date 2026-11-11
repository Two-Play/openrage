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
features_intro = """
Browse and search every archive, preview textures, models, sounds and animations, read metadata as XML,
edit game text and install `.oiv` packages — compatible with OpenIV's `mods` folder, with undo.
"""
screenshots_title = "Screenshots"
screenshots_intro = "The screenshots show OpenRAGE displaying files from the author's own copy of the game."
games_title = "Supported games"
games_intro = "GTA V comes first; Red Dead Redemption 2 archives can be read, GTA IV is planned."
details_summary = "Checked against the whole game"
download_intro = "Unpack the archive and run `openrage-gui` (or `openrage` for the command line). On first start the GUI detects your game and sets up the keys."
why_title = "Why OpenRAGE"
why_intro = """
OpenIV has been the standard tool for RAGE games for years, and OpenRAGE keeps what it got right:
the same `mods` folder, the same `.oiv` packages, the same export formats. This is what it adds.
"""
compare_title = "OpenRAGE and OpenIV side by side"
compare_note = """
OpenIV is a mature tool with wider game support and model import. You can use both:
they share the same `mods` folder. Compared with OpenIV's public releases;
if something is out of date, please [open an issue](https://github.com/Two-Play/openrage/issues).
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

# ------------------------------------------------------------- Why OpenRAGE

[[extra.reasons]]
icon = "🔓"
title = "Open source"
text = "GPL-3.0. Read the code, check what it does with your files, fix it or fork it. The tool cannot vanish with a single team."

[[extra.reasons]]
icon = "🐧"
title = "Native on Linux and Windows"
text = "One code base, native builds for both. Works with Steam and Proton installs; no Wine needed for the tool itself."

[[extra.reasons]]
icon = "↩️"
title = "Undo everything"
text = "Every change is journaled and can be undone step by step, back to the original files. Installed `.oiv` packages can be uninstalled."

[[extra.reasons]]
icon = "👀"
title = "See before you install"
text = "An `.oiv` package shows exactly which files and lines it would change before anything is written."

[[extra.reasons]]
icon = "🔎"
title = "Search the whole game"
text = "Names, hashes (`0xB779A091` → `adder`) and full text of every archive, in milliseconds."

[[extra.reasons]]
icon = "🎬"
title = "More than a file browser"
text = "Play whole cutscenes in one scene, view collisions, road networks and nav meshes in 3D, disassemble scripts, export models as glTF for Blender."

[[extra.reasons]]
icon = "✅"
title = "Tested on the whole game"
text = "Every format is checked against every file of the game: hundreds of thousands of textures, models, sounds and clips."

[[extra.reasons]]
icon = "🔁"
title = "No lock-in"
text = "Same `mods` layout, `.oiv` format and DDS/WAV output as OpenIV. Switch back and forth whenever you like."

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
openrage = "Linux and Windows, native"
edge = "openrage"

[[extra.compare]]
feature = "Games"
openiv = "GTA IV, EFLC, GTA V, Max Payne 3"
openrage = "GTA V; RDR2 archives; GTA IV planned"
edge = "openiv"

[[extra.compare]]
feature = "OpenIV `mods` folder"
openiv = "Yes"
openrage = "Yes, same layout"

[[extra.compare]]
feature = "`.oiv` packages"
openiv = "Install"
openrage = "Install with a preview of every change, uninstall"
edge = "openrage"

[[extra.compare]]
feature = "Undo"
openiv = "No"
openrage = "Every change, step by step"
edge = "openrage"

[[extra.compare]]
feature = "Search"
openiv = "File names"
openrage = "Names, hashes and full text of the whole game"
edge = "openrage"

[[extra.compare]]
feature = "Model export"
openiv = "openFormats"
openrage = "glTF (Blender and most 3D tools)"

[[extra.compare]]
feature = "Model import"
openiv = "Yes, from openFormats"
openrage = "Not yet"
edge = "openiv"

[[extra.compare]]
feature = "Cutscenes"
openiv = "As files"
openrage = "Play the whole scene with every actor and the cutscene camera"
edge = "openrage"

[[extra.compare]]
feature = "Command line"
openiv = "No"
openrage = "Yes, for scripts and automation"
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
