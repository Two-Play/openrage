# rage-rpf

Read and write RAGE archives, part of
[OpenRAGE](https://github.com/Two-Play/openrage):

| Format | Game | Read | Write |
|---|---|---|---|
| RPF7 (incl. nested archives, AES and NG encryption) | GTA V Legacy and Enhanced | yes | yes |
| IMG3, RPF2, RPF3 | GTA IV | yes | yes |
| RPF8 | Red Dead Redemption 2 | with the `rdr2` feature | no |

`vfs` resolves paths that cross archive boundaries
(`update/update.rpf/x64/data/lang/americandlc.rpf/global.gxt2`).

```rust,ignore
let keys = std::sync::Arc::new(rage_crypto::GtaVKeys::load(&keys_path)?);
let rpf = rage_rpf::Rpf7::open(&game.join("common.rpf"), Some(keys))?;
for id in rpf.children(rage_rpf::Rpf7::ROOT) {
    println!("{}", rpf.entry(id).name);
}
```

Untrusted archives are expected: nesting depth (`MAX_NESTING`) and unpacked
sizes (`MAX_UNPACKED`) are limited, and `vfs::safe_relative_path` must be used
for every extraction path.

## The `rdr2` feature and its licence

RPF8 decryption uses [`rage-tfit2`](../rage-tfit2), which is licensed under
the **AGPL-3.0-only** because it is ported from an AGPL project. The feature
is therefore **off by default**: with default features this crate and
everything it pulls in is GPL-3.0-only (or more permissive). Enabling `rdr2`
makes the AGPL apply to the resulting program.

## Process-wide defaults

`vfs::resolve` opens archives by itself, so it takes the GTA IV key
(`gta4::set_default_key`), the GTA IV audio names (`gta4::set_audio_names`)
and, with `rdr2`, the RDR2 keys and names (`rpf8::set_defaults`) from
process-wide registries that an application sets once at start-up. Code that
opens archives directly can pass keys explicitly (`Gta4Archive::open`,
`Rpf8::open_with_keys`) and never touch these registries.

## Licence

GPL-3.0-only (see `LICENSE`); the optional `rage-tfit2` dependency is
AGPL-3.0-only. RPF7 format knowledge comes from CodeWalker (MIT); see
`NOTICE.md`.
