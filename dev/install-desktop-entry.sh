#!/bin/sh
# Adds OpenRAGE to the desktop's application menu for a build from source, so
# that the window and the task bar show its icon (on Wayland the icon comes
# only from a desktop entry named like the app id, `openrage`).
#
#   dev/install-desktop-entry.sh                  # target/release/openrage-gui
#   dev/install-desktop-entry.sh path/to/openrage-gui
#
# Removes nothing it did not add; undo with:
#   rm ~/.local/share/applications/openrage.desktop \
#      ~/.local/share/icons/hicolor/256x256/apps/openrage.png
set -eu
root=$(cd "$(dirname "$0")/.." && pwd)
exe=${1:-$root/target/release/openrage-gui}
[ -x "$exe" ] || { echo "no program at $exe (build it first: cargo build --release)" >&2; exit 1; }
exe=$(cd "$(dirname "$exe")" && pwd)/$(basename "$exe")
data=${XDG_DATA_HOME:-$HOME/.local/share}
mkdir -p "$data/applications" "$data/icons/hicolor/256x256/apps"
cp "$root/packaging/openrage.png" "$data/icons/hicolor/256x256/apps/openrage.png"
sed "s|^Exec=.*|Exec=\"$exe\" %f|" "$root/packaging/openrage.desktop" > "$data/applications/openrage.desktop"
command -v update-desktop-database >/dev/null && update-desktop-database "$data/applications" || true
command -v gtk-update-icon-cache >/dev/null && gtk-update-icon-cache -q -t "$data/icons/hicolor" || true
echo "Installed $data/applications/openrage.desktop (starts $exe)"
