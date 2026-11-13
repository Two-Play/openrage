# rage-core

Shared primitives for the RAGE engine file-format crates of
[OpenRAGE](https://github.com/Two-Play/openrage):

* `hash`: Jenkins one-at-a-time (`joaat`) hashes as used for names in RAGE games.
* `bytes`: bounds-checked little- and big-endian reads (`LeRead`, `BeRead`,
  `Cursor`) that return `None` instead of panicking, and `f16_to_f32`.
* `io`: positional file reads that work on Unix and Windows.
* `text`: small text helpers shared by the format crates.

```rust
use rage_core::bytes::LeRead;

let header = [0x52, 0x50, 0x46, 0x37, 0x10, 0, 0, 0];
assert_eq!(header.u32_at(4), Some(16));
assert_eq!(header.u32_at(6), None); // past the end
```

## Licence

GPL-3.0-only. See `LICENSE`.
