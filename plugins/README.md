# OpenRAGE plugins

Sandboxed WebAssembly plugins for [OpenRAGE](https://github.com/Two-Play/openrage).
How to write one: **[Writing plugins](https://two-play.github.io/openrage/plugins/)**.

| Folder | |
|---|---|
| `sdk/` | `openrage-plugin-sdk`: write plugins in Rust |
| `examples/strings/` | example: the readable text in any file, as a preview and an export (also of whole folders) |
| `examples/gxt2-patch/` | example: applies an OpenIV `.oxt` text file to a `.gxt2`, writing into the `mods` folder |
| `examples/obj-tools/` | example: exports game models as OBJ with PNG textures (`load-model`), shows `.obj` in 3D, replaces an Enhanced `.ydr` with an OBJ (`build-model`) |
| `tests/probe/` | test plugin for the host's sandbox and limits (not shipped) |

The interface itself is [`crates/openrage-plugins/wit/plugin.wit`](../crates/openrage-plugins/wit/plugin.wit).

```sh
rustup target add wasm32-wasip2
cargo build --release --target wasm32-wasip2   # in this folder
openrage plugin pack examples/strings --wasm target/wasm32-wasip2/release/strings.wasm   # → strings-0.1.0.orp
```

Everything in this folder and `plugin.wit` are licensed under MIT OR Apache-2.0
([LICENSE-MIT](LICENSE-MIT), [LICENSE-APACHE](LICENSE-APACHE)), unlike the rest of OpenRAGE (GPL-3.0-only), so plugins
may use any licence.
