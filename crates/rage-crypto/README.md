# rage-crypto

Cryptography of RAGE engine archives, part of
[OpenRAGE](https://github.com/Two-Play/openrage):

* GTA V (Legacy and Enhanced): the AES key, the NG cipher with its keys and
  tables, and the name hash that picks an NG key per file.
* GTA IV: the AES-256 key (16 rounds per block).
* Rockstar's XXTEA variant (audio containers).

**This crate contains no Rockstar key material.** Keys are found at runtime in
files the user already owns: the game executable (`scan_executable`,
`GtaVKeys::from_scan`, `Gta4Key::find_in_executable`), or a CodeWalker key blob
(`decode_codewalker_magic`). The executable is located by SHA-1 fingerprints
(`key_hashes`), which do not contain or allow reconstructing any key.

Key caches are read from and written to a path the caller chooses:

```rust,ignore
let exe = std::fs::read("GTA5.exe")?;
let keys = rage_crypto::GtaVKeys::from_scan(&rage_crypto::scan_executable(&exe))
    .expect("keys not found");
keys.save(&my_cache_dir.join("gtav.keys"))?;
let keys = rage_crypto::GtaVKeys::load(&my_cache_dir.join("gtav.keys"))?;
```

The crate keeps no global state.

## Licence

GPL-3.0-only (see `LICENSE`). Format knowledge and the key fingerprints come
from CodeWalker (MIT); see `NOTICE.md`.
