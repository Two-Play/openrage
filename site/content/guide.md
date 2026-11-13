+++
title = "Explorer and search"
description = "The OpenRAGE explorer: folder tree, preview, viewer windows, menus and keyboard shortcuts, the search across the whole game, settings and plugins."

[extra]
docs = true
lead = "Find your way around the game: browse archives like folders and search everything at once."
toc = true
+++

New here? Start with [Getting started](@/getting-started.md). Prefer a terminal? See the [command line](@/cli.md).

## Starting

Start `openrage-gui`, or `openrage-gui <virtual path>` to open a folder or file directly.
With several games set up, OpenRAGE asks which one to open, unless one is ticked as *Always open this game*.
**File → Switch game…** changes the game later.

## The window

### Folder tree and file list

- The folder tree shows the game folder. Archives, including archives inside archives, open like folders.
- The file list is sortable and shows a format icon for each file (textures, models, maps, data, audio, scripts …).
- Files that contain other files (texture dictionaries, audio containers) carry a badge on their icon.
- The **address bar** has clickable breadcrumbs. Click the free space (or press Ctrl+L) to type a path.

### Preview and info panel

- Selecting a file shows it in the preview: text in a fixed-width font, images, 3D models, sounds, or a hex dump.
- The info panel lists the file's details: type, size, version, format, mip levels, meshes, triangles …
- A texture dictionary previews up to 16 of its textures at once; a drawable dictionary shows all its objects side by side.

### Viewer windows

- **Double-click** opens folders, archives and texture dictionaries.
- Models and textures open in their own **viewer window** (also Ctrl+Enter). Several can be open at once, for example
  to compare models.
- Text files (`.xml`, `.meta`, `.dat`, `.ini`, …) open in the **text editor**. Right-click → *Open with system app*
  opens a read-only copy in your default program.

What each format looks like is described in [Viewing files](@/viewers.md).

### Extracting

**Extract…** saves files, folders or whole archives. It runs in the background, can be cancelled, and its output
matches OpenIV's.

## Menus and keyboard

Every action is in the menu bar (File, Edit, View, Go, Help) with its shortcut. Right-click a file for the actions that
apply to it: open, view, extract, export, replace, rename, delete, copy name or path.

| Keys | Action |
|---|---|
| Arrow keys, Page Up/Down, Home/End | Move in the list |
| Enter | Open |
| Backspace or Alt+Up | Up one folder |
| Ctrl+L | Type a path |
| Ctrl+F | Search |
| F2 / Delete | Rename / delete (in edit mode) |
| Ctrl+Z | Undo |
| F1 | All keyboard shortcuts |

## Search

OpenRAGE keeps a search index of the whole game. It is built once in about 30 seconds; after a game update only the
archives that changed are read again.

### What you can search for

| Type | Example | Finds |
|---|---|---|
| Names | `adder` | any part of a file or folder name, exact matches first |
| Hashes | `0xB779A091`, `B779A091`, `3078201489`, `-1216765807` | the name behind the hash (`adder`), files with that name hash and game text with that label hash |
| Text | `weapon pistol` | words in XML/META/DAT/UGC files, audio name tables and game text; all words must occur |

Game text is filtered to one language, by default your system language.

### From the address bar

Typing `?query` into the address bar searches as well; Back returns to the results.

## Settings

**Settings** (gear button, File → Settings… or Ctrl+,) holds:

- the theme (light, dark or system) and the accent colour (a preset or any colour of your own);
- the games: *Switch game…*, *Add or change a game…* and whether to ask at start;
- whether the history opens in its own window and whether the mods overlay is shown;
- the [plugins](#plugins): which are on, their permissions, the plugin folder.

The sun/moon button in the toolbar switches the theme quickly.

## Plugins

Plugins add previews and actions for file types OpenRAGE does not handle itself. They are WebAssembly programs that run
in a sandbox: a plugin sees only the files you open with it and, if you allow it, reads the game's files. It cannot
change anything, open other files or go online.

- **Install:** **Settings → Plugins → Install plugin…** installs a plugin package (`.orp`); opening an `.orp` with
  OpenRAGE does the same. You first see what the plugin is, what it may do and its SHA-256 checksum (compare it with
  the one the author publishes). **Uninstall** removes it again. A plugin folder also works: put it into your plugin
  folder (`~/.config/openrage/plugins` on Linux, `%APPDATA%\openrage\plugins` on Windows, or `plugins` next to the
  program); **Open plugin folder** opens it, **Reload plugins** picks up new ones. The download comes with three example plugins: *Strings* (shows and
  exports the readable text of any file), *Text patch* (applies an OpenIV `.oxt` file to a `.gxt2`) and *OBJ tools*
  (exports models as OBJ with their textures, shows `.obj` files in 3D).
- **Enable:** **Settings → Plugins** lists each plugin with what it does, its licence and the permissions it asks
  for. New plugins are off; turning one on grants what it asks for. A new version that asks for more is off again
  until you confirm. Plugins that cannot be loaded are listed with the reason.
- **File list:** plugins name the file types they add and give them an icon.
- **Preview:** files OpenRAGE has no viewer for are shown by a plugin (text, tree, table, picture or a 3D model in
  the model view, with **Open viewer** and **Export glTF…**). The info panel
  names the plugin and shows its messages and errors. A plugin for every type (like *Strings*) only gets binary files.
- **Context menu:** plugin entries such as **Export strings…**. One result is saved with a save dialog, several into
  a folder you pick. Some actions also work on whole folders and archives (progress in the status bar, **Cancel**
  stops), and some ask for a file first, like the patch for **Apply text patch…**.
- **Changing files:** plugins with the *write-mods* permission can change game files. That only works while editing
  is unlocked; the changes go into the `mods` folder, never into the game's own files, and each run is one entry in
  **History** that you can undo.
- **Command line:** everything also works with [`openrage plugin`](@/cli.md#plugins).

Want to write one? See [Writing plugins](@/plugins.md).

## About and licenses

**Help → About OpenRAGE** shows the version, OpenRAGE's license (GPL-3.0, and AGPL-3.0 for the TFIT2 cipher), a link
to the source code, the credits, and every third-party crate with its license. The same list is on the
[licenses page](@/licenses.md).
