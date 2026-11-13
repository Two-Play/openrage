# rage-resource

RAGE resources, part of [OpenRAGE](https://github.com/Two-Play/openrage):
the paged, pointer-linked binary format behind most game assets (textures,
models, maps, …).

* RSC7 (GTA V, RDR2): inflate the pages, follow virtual pointers
  (`0x5…` system memory, `0x6…` graphics memory), build new resources
  (`builder`, `pages`).
* RSC5 (GTA IV): header, page sizes, reading and writing.

Every read is bounds-checked and returns a `ResourceError` instead of
panicking; inflated pages are capped at `MAX_PAGES`.

```rust,ignore
let res = rage_resource::Resource::from_rsc7(&std::fs::read("x.ytd")?)?;
let count = res.u16(0x5000_0028)?;
```

## Licence

GPL-3.0-only. See `LICENSE`.
