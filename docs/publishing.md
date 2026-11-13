# Publishing the format libraries

The format crates are prepared to be published on crates.io on their own, in
this order. Nothing has been published yet.

| Crate | Depends on (workspace) | Licence | State |
|---|---|---|---|
| `rage-crypto` | – | GPL-3.0-only | ready: `cargo package -p rage-crypto` passes |
| `rage-core` | – | GPL-3.0-only | ready |
| `rage-resource` | – | GPL-3.0-only | ready |
| `rage-tfit2` | – | AGPL-3.0-only | ready (needed only for `rage-rpf/rdr2`) |
| `rage-rpf` | `rage-core`, `rage-crypto`, `rage-resource`, optional `rage-tfit2` | GPL-3.0-only (AGPL with `rdr2`) | packages once its dependencies are on crates.io |
| `rage-formats` | `rage-core`, `rage-resource` | GPL-3.0-only | not prepared yet |

Each prepared crate has its own `README.md`, a `LICENSE` (a link to the
workspace licence; `cargo package` copies the file), and, where code is
derived from CodeWalker, a `NOTICE.md` with the MIT notice.

Workspace path dependencies carry `version = "0.2.0"`, so crates.io resolves
them once the dependency is published. `cargo package` of a crate that
depends on an unpublished workspace crate fails at "no matching package";
publish bottom-up (`rage-core`, `rage-crypto`, `rage-resource`, `rage-tfit2`,
then `rage-rpf`).

## Licensing of RDR2 support

`rage-tfit2` is a port of RDR2-RPF-Tool, which is AGPL-3.0. It stays
AGPL-3.0-only. To keep that licence away from users who only need GTA V or
GTA IV archives, RPF8 support in `rage-rpf` (`rpf8`, `rpf8_exe`, `rpf8_keys`,
`rpf8_names` and the RDR2 parts of `vfs`) is behind the `rdr2` feature:

* `rage-rpf` alone (default features): GPL-3.0-only, no AGPL code.
* `rage-rpf` with `rdr2`: the program includes AGPL code and is subject to the AGPL.
* The OpenRAGE workspace turns `rdr2` on (`[workspace.dependencies]`), so the
  CLI and GUI keep full RDR2 support, as before.

No licence was changed. The options for the owner, if a different outcome
is wanted:

1. **Keep it as it is.** Library users opt in to the AGPL with `rdr2`.
2. **Clean-room reimplementation** of the TFIT2-CBC cipher and the strided
   layout, from a written specification by someone who has not read the AGPL
   source. The new crate could then be GPL-3.0 (or more permissive) and the
   feature split would no longer matter for licensing.
3. **Ask the author of RDR2-RPF-Tool** for permission to use the port under
   GPL-3.0 (or a dual licence).
4. **Relicense the libraries** (e.g. MIT OR Apache-2.0) to reach more users.
   This needs the consent of every contributor to those crates, and does not
   change `rage-tfit2`, which stays AGPL.

## Paths and global state

The libraries no longer know where OpenRAGE keeps its files. Key caches,
name indexes and other caches are passed in as paths; OpenRAGE's own
locations live in `openrage_app::paths`.

`vfs::resolve` opens archives by itself, so it still reads keys from
process-wide registries that an application sets once at start-up:

* `rage_rpf::gta4::set_default_key` and `set_audio_names`
* `rage_rpf::rpf8::set_defaults` (with `rdr2`)
* `rage_formats::names::register` (extra names for XML export; only grows)

They are documented as such. Code that opens archives directly can pass keys
explicitly (`Rpf7::open`, `Gta4Archive::open`, `Rpf8::open_with_keys`).
Replacing them with an explicit context passed to `vfs::resolve` would change
every caller in the CLI and GUI; that is left for later.

## Still to decide before publishing

* **Crate names.** `rage-*` names may be taken or confusing on crates.io;
  check availability (`cargo search rage-core`) or pick a prefix.
* **Version.** The crates share the workspace version (0.2.0). Independent
  versions per crate (and a 0.1.0 start) are an option.
* **MSRV.** `rust-version = "1.99"` is the pinned toolchain, the only one
  checked. A lower MSRV (edition 2024 needs at least 1.85; the code uses
  `as_chunks` and let chains, so 1.88 is the likely floor) would need a CI job.
* **`publish = false`** on the crates that are not meant for crates.io
  (`openrage-*`, `rage-index`, `rage-mods`, and `rage-formats` for now), so
  they cannot be published by mistake.
* **Error messages.** `RpfError::KeysRequired` still says "run `openrage keys`
  first", an OpenRAGE hint inside a library message.
* **API review.** The public error enums are `#[non_exhaustive]` now; a pass
  over `pub` items that are only public for the app (e.g. `rpf8_names`
  internals, `key_hashes`) would avoid committing to them in 0.x releases.
