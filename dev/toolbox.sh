#!/bin/sh
# Builds the OpenRAGE development image and creates (or recreates) the
# "openrage" toolbox from it. Run from anywhere inside the repository.
set -eu
root=$(git -C "$(dirname "$0")" rev-parse --show-toplevel)
cp "$root/rust-toolchain.toml" "$root/dev/rust-toolchain.toml"
trap 'rm -f "$root/dev/rust-toolchain.toml"' EXIT
podman build -t localhost/openrage-dev -f "$root/dev/Containerfile" "$root/dev"
if toolbox list --containers | grep -q ' openrage '; then
    echo "Removing the old openrage toolbox …"
    toolbox rm --force openrage
fi
toolbox create --image localhost/openrage-dev openrage
echo
echo "Done. Enter it with:  toolbox enter openrage"
