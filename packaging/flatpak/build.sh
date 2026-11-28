#!/bin/sh
# Builds the OpenRAGE Flatpak bundle from already built programs, without
# compiling anything again:
#
#   packaging/flatpak/build.sh <dir> <version> <out.flatpak>
#
# <dir> holds `openrage`, `openrage-gui` and `plugins/` (the unpacked Linux
# release archive). Needs `flatpak` and the runtime org.freedesktop.Platform
# of $RUNTIME_VERSION (e.g. `flatpak install --user flathub
# org.freedesktop.Platform//26.08`).
#
# Install the result with `flatpak install --user openrage-….flatpak`; the
# command line runs as `flatpak run --command=openrage io.github.Two_Play.OpenRAGE`.
set -eu
src=$1 version=$2 out=$3
ID=io.github.Two_Play.OpenRAGE
RUNTIME_VERSION=${RUNTIME_VERSION:-26.08}
here=$(cd "$(dirname "$0")" && pwd)
packaging=$(dirname "$here")
root=$(dirname "$packaging")
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
build=$work/build repo=$work/repo

# Nothing is compiled, so the runtime also serves as the SDK.
flatpak build-init "$build" "$ID" org.freedesktop.Platform org.freedesktop.Platform "$RUNTIME_VERSION"
files=$build/files
install -Dm755 "$src/openrage" "$files/bin/openrage"
install -Dm755 "$src/openrage-gui" "$files/bin/openrage-gui"
# The example plugins next to the programs, where OpenRAGE looks for them.
cp -r "$src/plugins" "$files/bin/plugins"
sed -e "s/^Icon=.*/Icon=$ID/" -e "s/^StartupWMClass=.*/StartupWMClass=$ID/" \
    "$packaging/openrage.desktop" > "$work/$ID.desktop"
install -Dm644 "$work/$ID.desktop" "$files/share/applications/$ID.desktop"
install -Dm644 "$packaging/openrage.png" "$files/share/icons/hicolor/256x256/apps/$ID.png"
install -Dm644 "$packaging/openrage-mime.xml" "$files/share/mime/packages/$ID.xml"
sed -e "s/{VERSION}/$version/" -e "s/{DATE}/$(date -u +%F)/" \
    "$here/$ID.metainfo.xml" > "$work/$ID.metainfo.xml"
install -Dm644 "$work/$ID.metainfo.xml" "$files/share/metainfo/$ID.metainfo.xml"
for f in README.md LICENSE NOTICE.md THIRD_PARTY_LICENSES.md; do
    install -Dm644 "$root/$f" "$files/share/doc/openrage/$f"
done
install -Dm644 "$root/crates/rage-tfit2/LICENSE" "$files/share/doc/openrage/LICENSE-AGPL-3.0"

# Games live anywhere (Steam libraries on other drives, Flatpak Steam,
# Heroic, Lutris), mods go next to them and the Proton fix edits the game's
# prefix: OpenRAGE needs the whole file system, like OpenIV on Windows.
# The network is only used when the user asks to download CodeWalker's key
# blob (magic.dat) in the key setup.
flatpak build-finish "$build" \
    --command=openrage-gui \
    --share=ipc \
    --share=network \
    --socket=wayland \
    --socket=fallback-x11 \
    --socket=pulseaudio \
    --device=dri \
    --filesystem=host
flatpak build-export "$repo" "$build" stable
flatpak build-bundle "$repo" "$out" "$ID" stable \
    --runtime-repo=https://dl.flathub.org/repo/flathub.flatpakrepo
