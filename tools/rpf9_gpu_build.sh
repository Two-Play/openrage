#!/bin/sh
# Compile portable sources only; no downloads, game reads or GPU dispatch.
set -eu
: "${RPF9_GAME_DIR:?Set RPF9_GAME_DIR to the directory containing rpf9-hash-export}"
TOOLS=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
BUILD=${RPF9_GPU_BUILD_DIR:-"$RPF9_GAME_DIR/rpf9-hash-export/gpu-build"}
mkdir -p "$BUILD"
"${RPF9_GLSLANG:-glslangValidator}" -V "$TOOLS/rpf9_gpu.comp" -o "$BUILD/rpf9-gpu.spv"
"${CXX:-c++}" -O2 -std=c++17 "$TOOLS/rpf9_gpu.cpp" -lvulkan -o "$BUILD/rpf9-gpu-runner"
