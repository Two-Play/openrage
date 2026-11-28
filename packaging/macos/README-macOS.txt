OpenRAGE for macOS (Apple silicon) - preview
============================================

This build is a preview: it is built and tested automatically, but has seen
little use on real Macs yet. Please report problems:
https://github.com/Two-Play/openrage/issues

Starting OpenRAGE
-----------------
The programs are not signed by an Apple developer account, so macOS blocks
them the first time. After unpacking, open Terminal in this folder and run:

    xattr -dr com.apple.quarantine .
    ./openrage-gui

"openrage" is the command-line tool. "plugins" holds the example plugins;
they stay off until you turn them on in Settings > Plugins.

Your game
---------
The games do not run on macOS themselves, so OpenRAGE cannot find them on its
own. Choose the game folder by hand: a copy from a Windows PC, an external
drive, or the folder inside CrossOver, Whisky or a similar tool.

Guide:       https://openrage.dev/
Source code: https://github.com/Two-Play/openrage
