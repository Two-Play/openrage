+++
title = "Development"
description = "Building OpenRAGE, the reproducible build environment, releases and the crates the project is made of."

[extra]
lead = "For contributors: build environment, releases and the project layout."
toc = true
+++

## Build

See [Build from source](@/getting-started.md#build-from-source) for Linux and Windows. In short: install Rust with
rustup (the version is pinned in `rust-toolchain.toml`), on Linux the development packages of fontconfig,
xkbcommon, Wayland and ALSA, then `cargo build --release`.

## Reproducible build environment

For the system libraries there is a container image based on Fedora's toolbx image, `dev/Containerfile`:

```sh
dev/toolbox.sh            # build the image and create the "openrage" toolbox
toolbox enter openrage
cargo run --release -p openrage-gui
```

Inside the toolbox the GUI runs as usual (Wayland/X11, GPU, audio and your home folder, including the game, are shared). Without toolbx the image works with plain Podman or Docker:

```sh
cp rust-toolchain.toml dev/ && podman build -t localhost/openrage-dev -f dev/Containerfile dev
podman run --rm -v "$PWD":/src:Z -w /src localhost/openrage-dev cargo build --release
```

CI builds the image and OpenRAGE inside it whenever the environment or the dependencies change.

## Releases

Releases are made by pushing a version tag (`git tag v0.2.0 && git push origin v0.2.0`, matching the version in `Cargo.toml`); the *Release* workflow builds both systems and publishes the release. It can also be run by hand from the Actions tab to build the archives without publishing.

Once a release is published, the *Release bundle* workflow adds `openrage-<tag>-linux-windows.zip`: both versions in one
download (`linux/`, `windows/`, a short `README.txt` and the license files), for mod sites that host one file per release.
It reuses the release's archives instead of building again and adds its checksum to `SHA256SUMS.txt`. Run it by hand
(*Actions → Release bundle → Run workflow*) with a tag to make the bundle for an older release; with *upload* off it is
only kept as a workflow artifact.

## Licenses

OpenRAGE is GPL-3.0-only, so every dependency must come under a license that can be combined with it.
`deny.toml` lists the allowed licenses; CI runs `cargo deny check licenses` on every change. A new dependency
with another license fails CI: check that it is GPL-3-compatible before adding it to the list.

`THIRD_PARTY_LICENSES.md` (shipped with every release) and `crates/openrage-app/src/third_party_licenses.json`
(shown in *Help → About OpenRAGE*, by `openrage licenses` and on the [licenses page](@/licenses.md)) are generated
from `Cargo.lock` with [cargo-about](https://github.com/EmbarkStudios/cargo-about) (configured in `about.toml`).
After changing dependencies, regenerate them:

```
cargo install --locked --features cli cargo-about
cargo fetch --locked
python3 dev/third_party_licenses.py
```

CI checks that both files are current (`python3 dev/third_party_licenses.py --check`). The licenses page copies the
JSON into `site/data/` when the website is built; do the same (`mkdir -p site/data && cp
crates/openrage-app/src/third_party_licenses.json site/data/`) before `zola serve`.

Credits for format knowledge and ported code go into `NOTICE.md` by hand; the About dialog shows its sections.

## Project layout

| Crate | Purpose |
|---|---|
| `rage-core` | shared primitives: hashing (joaat), positional I/O |
| `rage-crypto` | AES / NG ciphers, Rockstar's XXTEA, key discovery in the user's executable, key cache |
| `rage-rpf` | RPF7 reader and OpenIV-style writer, virtual file system over nested archives |
| `rage-index` | SQLite search index: names, hash dictionary, FTS5 full text, incremental updates |
| `rage-resource` | RSC7 resources: page layout, virtual pointers into system/graphics memory |
| `rage-formats` | game formats: textures, models, audio, animations, metadata (meta/PSO/RBF → XML), text tables |
| `rage-mods` | OpenIV `mods` folder: overlay resolution, folder comparison, change lists |
| `openrage-app` | settings, key setup, game detection, extraction, model and format logic shared by CLI and GUI |
| `openrage-cli` | the `openrage` command-line tool |
| `openrage-gui` | the Slint explorer (`openrage-gui`) |

The UI language is English; the GUI is built with Slint's translation support so further languages can be added.
