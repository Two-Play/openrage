# Scarlett RPF9 header observations

These are static observations of a user-supplied x86-64 PE file, inspected with
GNU `objdump` and a Python byte reader. The executable was read as data and was
not run. No game code, game bytes, cipher tables, or keys are included here.

Executable: `game_scarlett_master.exe`, 120,181,760 bytes.

SHA-256: `e95020b2fa4a721db3df59fdecf75621730ac559d8835252f6c3b658d5a08993`. Addresses below are virtual addresses at image base
`0x140000000`. In this build's `.text`, file offset = VA - image base - `0xc00`.
The addresses are build-specific research references, not stable APIs.

REA CLI 6.8.0 was available, but its doctor reported no configured native
decompiler engine (Ghidra, IDA, Hopper, or Kuna). The following native findings
therefore come from ordinary offline disassembly, not REA decompilation or
runtime evidence.

## Clear header fields

| Offset | Width | Observation |
| --- | --- | --- |
| `0x00` | u32 | Magic `0x52504639` |
| `0x04` | u32 | Full header extent used by loader mode 0; recovery data start |
| `0x08` | u32 | Initial header extent used by loader mode 1; the remaining header span is loaded separately |
| `0x24` | u32 | Declared entry/record count; used in layout size arithmetic |
| `0x2e` | u8 | Initial header transform selector; zero skips this transform |

All integer fields above are little-endian. The count is a declaration;
reading it does not validate or decode the entry table. A nonzero transform
selector does not establish that every payload is encrypted.

The loader at `0x141fb2600` reads 16 bytes and checks the magic
(`0x141fb268e`). At `0x141fb269e` it converts the caller's mode into an index
of zero or one and reads `[header + 4 + index * 4]` at `0x141fb26a7`. It
allocates that extent and reads the remainder after the initial 16 bytes.
For mode 1, `0x141fb26ec` loads the value at `+4`, `0x141fb26f1` loads `+8`,
and `0x141fb26ff` subtracts the latter from the former. The resulting byte
count is read at archive-relative offset `+8` (`0x141fb2730`–`0x141fb2744`).
Thus `+8` is a split boundary inside the header, not the archive data start.

The layout builder at `0x14087af70` reads `+0x24` into `r10d`
(`0x14087af9a`) and passes it to `0x14087aff0`. The latter computes successive
table ranges, including `count * 24` (`0x14087b05c`, `0x14087b06d`) and
`(count - subcount) * stride * alignment` (`0x14087b07a`–`0x14087b083`).
This supports exposing the raw count as declared entries, without claiming
those entries have been recovered.

The transform dispatcher at `0x140a348a0` reads byte `+0x2e`
(`0x140a348ae`), tests it through `0x140b85020` (a nonzero test), then
selects a family with `selector >> 1` (`0x140a348c0`). Selector zero takes
the success return without a transform. A nonzero selector's low bit
selects one combined span or two spans (`0x140a348d3`–`0x140a34962`).
Every root sample below has selector 9.

## Root archive sample inventory

Only the first 64 bytes and the full declared header were read for this
inventory. Hashes below identify the **stored full header**, not the complete
archive or its payload. All twelve root archives were sampled.

| Archive | File bytes | +4 | +8 | Declared entries | Selector | Stored header SHA-256 |
| --- | ---: | ---: | ---: | ---: | ---: | --- |
| `anim.rpf` | 4407626384 | 18016 | 15360 | 265 | 9 | `4d9c1d570f50458508172613902a3075b033e907914c943fddc903d877cce7b2` |
| `appdata0.rpf` | 8880 | 864 | 832 | 2 | 9 | `fdbfc25bd0c41027e9d69d4324291f997db29d39c9513a8f72a512c4113fe95c` |
| `audio.rpf` | 28543454672 | 4944 | 4016 | 103 | 9 | `833969e3c6f13247f9da028653618b7df7733e5e7e51ddb889ffb9293121213a` |
| `common.rpf` | 6642192 | 38272 | 34080 | 418 | 9 | `c00636117394cf649c86c5cb90c1e067132d3d16ef2c66029a517c40c8ba2260` |
| `data.rpf` | 481544304 | 69728 | 62736 | 699 | 9 | `294db409d725a280ca510afe9b41afc40e4c35095545c9f1025976739eda7f42` |
| `levels.rpf` | 76155065056 | 110704 | 89920 | 2078 | 9 | `c34165af2bd74fce0f94ec36973e67796205e73fd36b67bcc79f595fe4b86a57` |
| `movies.rpf` | 8832481776 | 27328 | 22176 | 515 | 9 | `7601da37efd4c06dfc820fe0fc2662ce1e6a2a83515cb066e40eb750cb40ba59` |
| `navmeshes.rpf` | 6707512496 | 1920 | 1648 | 29 | 9 | `fccaa6261127a1628762fe6678e63920d218bedec78c81ecb1b794ef14094290` |
| `packs.rpf` | 4798213264 | 17744 | 16192 | 172 | 9 | `cf37936da5693d17c31445ae0c8f825d645f929433d45d87f7c548bd41722890` |
| `script.rpf` | 4358960 | 608 | 576 | 2 | 9 | `6739bfa0ed3f2ca735db2d7b6b5c5dddfdb0c6d844c27ddbf921eebd57513f1b` |
| `shaders.rpf` | 411695600 | 34368 | 34336 | 3 | 9 | `15f896951d3608eb45ca446c9d8198513076c0c9f2c3dd79981af8eb083a9af1` |
| `textures.rpf` | 192585024 | 1936 | 1728 | 22 | 9 | `d730f6a42de5474a7d9d490c672d4a2c0e35409a238508caf0943fdc660052e9` |

## Members

With the header decoded, a member is described by one payload record (raw,
chunked or resource) and optional encryption and chunk records.

An encryption record is 64 bytes: u32 key, u16 algorithm (17: counter mode
with a big-endian 16-byte counter), the initial counter at `+6`, and a
pattern of byte counts: `+24` head, `+32` run, `+40` gap, `+48` tail. The
head is encrypted, then `run` bytes of every `run + gap`, then the tail. The
counter follows the position in the member, encrypted or not. The usual
pattern (`0x400`, `0x400`, `0x1c00`) encrypts the first 2 KiB and 1 KiB of
every 8 KiB after. The tail field was seen in one resource only.

A resource starts with a clear 16-byte prefix. The encryption pattern and the
chunks count from its end. Decoded, the system part is followed by the
graphics part.

A chunk record is 8 bytes: u32 start of the list *counted from the record*,
u16 number of chunks, u8 item size (8), u8 format. Members with lists at the
same distance share records. Items, with codec 0 (stored) or 3 (zlib):

| Format | Decoded size | Stored size | Offset | Codec |
| ---: | --- | --- | --- | --- |
| 2 | u32 × 16 | bits 32–57 × 16, less padding in bits 58–61 | follows | bits 62–63 |
| 3 | 65536 − bits 0–15 | 65536 − bits 16–31 | bits 32–59 × 16 | bits 60–63 |
| 4 | bits 0–23 | bits 24–47, less padding in bits 52–55 | follows | bits 56–63 |

A chunk that follows another starts on the next multiple of 16. Members
without a chunk record are stored as they are (layout 0) or as one raw
DEFLATE stream (layout 1).

All members of the archives above up to 256 MiB, and all members of the
archives inside them, were decoded with this: 790,347 members, with the zlib
checksums and every decoded length matching.

## Names

The table holds only hashes, of paths relative to the archive without the
extensions that have a type number. Names come from path strings in the
executable and in the files of `common.rpf` and `data.rpf`, checked against
the hashes. They cover most top-level files. Inside nested archives, drawables, fragments,
clip dictionaries and some other types hold their own name, which is checked
against the hash the same way (over 99% of drawables and fragments in a sample
of 1,826 archives). Some names follow a pattern: navigation meshes
(`navmesh[204][293]`), cover (`coverregion[199][257]`), height and surface
tiles, the terrain's collisions (`s09_w04_55`) and variants of other files
(`_hi`, `+hi`, `_lod`). Files without a name are listed as
`entries/<slot>_<hash>`.

## Remaining work

Names of the other types inside nested archives (most textures, maps and the
collisions of buildings) and of the audio archives, signature checks and archive writing.

## Local recovery validation

The updated CLI was built and tested against the local `common.rpf` above.
`info` reported 418 declared native slots and transform tag `0x09` without
scanning the archive. `verify common.rpf` recovered and decoded 277 entries
(47.9 MiB unpacked), with zero read/checksum/length errors. A recursive JSON
listing contained the same 277 entries. Extracting its recovery `xml` folder
wrote 259 files (40.6 MiB) with zero failures, into a temporary output folder.
These checks validate the recovery path only; 277 streams must not be
interpreted as 277 of the 418 native slots. No original archive was changed.
