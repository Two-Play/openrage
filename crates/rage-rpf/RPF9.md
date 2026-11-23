# RPF9 Scarlett reader

This branch reads the encrypted TOC of the analysed Scarlett 1.0.2.1 build,
lists logical entries by their stored 64-bit hashes, and exports stored bytes
or supported decoded payloads. It reads cipher material from the user's PE;
no game keys, tables, executable bytes or archive fixtures are distributed.

```sh
openrage rpf9 common.rpf --exe game_scarlett_master.exe --list
openrage rpf9 common.rpf --exe game_scarlett_master.exe --extract 0 --output entry.bin
openrage rpf9 script.rpf --exe game_scarlett_master.exe --extract-raw 0 --output nested.rpf
openrage rpf9 pack/story000/story000.rpf --exe game_scarlett_master.exe --nested 419 --list
openrage rpf9 common.rpf --exe game_scarlett_master.exe --list --names candidates.txt
```

Outputs must not already exist. `--extract-raw` retains encryption, compression
and allocation padding. `--extract` uses the logical entry's crypto and block
descriptors; it does not infer file boundaries from signatures. Unknown codecs
and unsupported layouts produce explicit errors.

`--nested` opens a directly stored child archive without exporting it first.
Resource exports skip the physical 16-byte prefix and prepend the native
synthetic `RSC8` header to the decoded data. The reader supports multiple
external metadata columns, including archives containing only resources.

## Candidate name resolution

Like OpenRAGE's RPF8 resolver, `--names` tests an explicit list of candidate
paths against the TOC. The list is UTF-8, one path per line, limited to 8 MiB
with a 4096-byte line limit and at most 250,000 indexed keys.
It tries normalized paths, paths without mount prefixes, basenames and stems.
Distinct matches are preserved rather than choosing between collisions.

The name lookup column is `73bcb72dce74f11f`, **not** `b6f5bdb2c66da2e9`.
The latter remains an opaque stored identifier. Native `0x144F66DE0` uses
MurmurHash3 x64-style mixing with seed zero, returning the low 64 bits.
The table at `0x1465B23B0` folds ASCII case and maps backslashes to slashes.
Its finalizer uses the low byte of the length. Some matched paths include
extensions; others do not. The application derives known resource extensions
from the TOC file type; unknown types retain the candidate spelling.

A bounded local experiment decoded 256 small appdata/common entries, with
6,608,262 bytes charged against the output budget. Their 18,519 candidate
strings produced 1,071 matching entries across the 22 outer archives
(1,062 distinct name hashes). These are candidate path matches, not a claim
that every original filename or directory tree has been recovered.
Encrypted trailer/name-table decoding is still unsupported.

## Bounded I/O

PE loading seeks to the headers, key registry and selected key/table blobs;
it does not read the complete executable. Each PE read is capped at 1 MiB.
Archive loading reads the 64-byte header and at most 64 MiB of head metadata;
there is no archive-sized allocation. Payload reads and decompression use
64-KiB buffers and validate declared input/output lengths. The reader never
loads a complete multi-gigabyte archive into memory. Raw copying streams to
the caller's writer. Callers choosing a `Vec` writer still own that output's
memory use.

## Native evidence and corrections

Addresses below are VAs in the reference PE, base `0x140000000`.

The former TFIT prefix proof used the wrong final permutation. Native block
`0x145642C50` uses `[0,13,10,7,4,1,14,11,8,5,2,15,12,9,6,3]`.
The corrected prefix is a property directory with IDs 1, 5, 6, 9 and relative
offsets 0, 0x18, 0x30, 0x140. The old pseudo-plaintext is not a valid vector.

`0x14087AFF0` and `0x14087B0F0` construct the physical views. For the supported
outer profile, the global TFIT-CBC profile decrypts `[0x40,0x1a0)` with
continuous chaining across the property directory and values.
`0x140A348A0` / `0x1406BD160` establish this chaining. Property 1 supplies
the archive's TOC key selector, algorithm 0x10 and IV at descriptor +6.
`0x1406BD090` decrypts `[0x1a0,w08)` with that profile, while `0x1408EC360`
decrypts `[w08,w04)` in a fresh provider with the original IV.

For general layouts, let `P = 0x40 + align16(property_count * 8)` and
`S = P + H[0x1c] - H[0x30]`. The outer span is `[0x40,S)`; the inner
spans are `[S,w08)` and `[w08,w04)`, with independent IV resets. `H[0x30]`
is the length of the inner-encrypted property suffix, not an alignment.

The 24-byte column descriptor is `(u64 hash, u32 count, u32 offset,
u16 stride, u16 region, u32 reserved)`. Native `0x144DB6600` resolves
row-major indices and all-ones sentinels. `0x1455B2600` selects the A/B data
base and applies the column offset and index. Logical entry order must not
be confused with physical record order.

| Column hash | Record |
|---|---|
| `33f70dfaf2c50989` | 24-byte ordinary extent |
| `ba180afb232d1b4b` | 20-byte resource extent |
| `286030a358a789fb` | 64-byte selective crypto descriptor |
| `e0c12d7a5d4884c5` | 8-byte relative block-table descriptor |
| `b6f5bdb2c66da2e9` | opaque 64-bit entry identifier |
| `73bcb72dce74f11f` | normalized 64-bit path hash |

Native getters `0x1400C6950`, `0x141330AD0`, `0x141331670` and
`0x1400F62E0` define offset, stored size, logical size and stream codec.
Ordinary offsets and stored sizes mask to 40 bits then multiply by 16;
logical size masks to 44 bits. Resource extents use separate packed fields.
The extent stream codec is byte +6 (resource +17), not byte +5.

Block tables override the stream codec. `0x14550DB40` decodes formats 2/3/4;
actual compressed bytes exclude block padding. Codec 3 is independently
zlib-validated. Each block resets decompression. Input length, decoded length,
checksum and stream termination are checked.

Payload algorithm 0x11 maps through `0x140536AC0`, `0x144C11250` and
`0x14171FD00` to TFIT-CTR `0x1455DC010`, block `0x1456437C0` and context
loader `0x1455DC890`. Its fixed tables are at `0x146040F30`, and its key
blob magic is `0x2718CE5D`; it is distinct from the metadata CBC primitive.

Crypto fields at +24/+32/+40/+48 are encrypted prefix, encrypted middle run,
clear middle run and encrypted suffix. Flags at +56 select the size coordinate.
`0x1428CEBC0` / `0x141616150` establish selective ranges. `0x140FC9040`
adds the absolute byte position divided by 16 to the original big-endian
128-bit IV; skipped clear bytes still advance the counter coordinate.

## Verification

Synthetic tests cover column indirection, missing records, corrupt indices,
size bounds, TFIT final permutation, CTR chunk boundaries, range boundaries,
and zlib truncation/checksum failures. No proprietary test fixture is needed.

Opt-in local validation:

```sh
RPF9_EXE=/path/to/game_scarlett_master.exe RPF9_ROOT=/path/to/game \
  cargo test -p rage-rpf --test rpf9_local -- --ignored --nocapture
```

The local corpus has 22 outer archives and 6,534 logical entries. Payload
validation selects bounded samples and streams decoded output to a sink.
It is not a claim that every byte in the corpus has been decoded.
Local nested validation covers 24 distinct metadata layouts.

## GUI, VFS and previews

The PR #144 interface and format readers are integrated with the exact TOC.
Place `game_scarlett_master.exe` and optionally `rpf9-names.txt` in the game
root (`RPF9_NAMES` overrides the dictionary path). RPF9 signature detection
takes precedence over copied GTA V/RDR2 executables. Opening an archive with
the reference executable uses its decrypted TOC and skips recovery scans.
Directly stored nested archives are navigable through the normal VFS.

Known paths can be opened without first adding them to the dictionary: lookup
normalizes the path, hashes it, checks the native file type and requires a
unique matching entry. Listing readable names still needs the dictionary.
For example: `openrage --game /path/to/game info common.rpf/data/ai/conversationlist.meta`.

Model previews use exact system/graphics partitions from resource flags.
TOC reads and model allocations are capped at 128 MiB; extraction streams.
Standalone `data/ttdb` TTPF pools support texture previews and PNG output;
the decoder has an explicit byte-budget API. Local bounded validation decoded
16 model resources and exported 16 textures, including BC1, successfully.
These samples do not establish support for all models or texture layouts.

The metadata export covers 1,255,039 entries across 4,373 archives (22 outer
and 4,351 direct nested). A local GPU dictionary run tested 18,959,548 candidates
and matched 80,958 distinct 64-bit hashes covering 101,484 entries. Candidate
preimages are not proof of original spelling; 32-bit-only matches remain
separate hypotheses. See `tools/` for reproducible dictionary tooling.

## Remaining scope

Trailer decoding, missing names, transformed nested archives and codecs not
yet verified remain separate work. Embedded texture-pool discovery, BC6H/BC7
BCPack decoding, volume textures, skeletons and model LOD support are incomplete.
No editing/repacking or support for other executable builds is claimed.
