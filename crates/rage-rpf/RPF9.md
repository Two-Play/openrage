# RPF9 on Xbox Series/Scarlett (experimental)

This note documents the RPF9 variant implemented by `rage-rpf::rpf9` and
`rage-rpf::rpf9_exe`. It comes from static analysis of one Xbox
Series/Scarlett 1.0.2.1 build and measurements over its 22 external archives.
No executable was run and no archive was modified during the analysis.

The implementation is intentionally a probe, not a complete archive reader.
The header and TFIT-CBC path below are reproduced and tested; the complete TOC
mapping and trailer semantics are not understood yet.

## On-disk header

The first 0x40 bytes are plaintext and little-endian. The magic bytes are
`39 46 50 52` (`9FPR` on disk), or `0x52504639` as a little-endian `u32`.

| Offset | Size | `Header` field | Observed meaning |
|---:|---:|---|---|
| `0x00` | 4 | magic | RPF9 magic, `0x52504639` |
| `0x04` | 4 | `metadata_end` | End of encrypted head metadata |
| `0x08` | 4 | `region_b_offset` | Start of encrypted head region B |
| `0x0c` | 4 | `trailer_size` | Encrypted bytes at the end of the file |
| `0x10` | 8 | `archive_id` | Distinct 64-bit value for every external archive |
| `0x18` | 4 | `format_constant` | `0x00047a00` in this build |
| `0x1c` | 4 | `region_constant` | `0x00000150` in this build |
| `0x20` | 2 | `field_20_low` | Unknown |
| `0x22` | 2 | `region_b_mode` | `1` for all 22 external archives; mode `2` also exists in nested data |
| `0x24` | 4 | `entry_count` | Number of entries represented by the metadata |
| `0x28` | 4 | `profile_tag` | Per-profile value; exact role unknown |
| `0x2c` | 1 | `index_width` | One byte up to 255 entries, otherwise two |
| `0x2d` | 1 | `byte_2d` | Unknown |
| `0x2e` | 1 | `crypto_profile` | `9` selects the recovered TFIT-CBC path |
| `0x2f` | 1 | `byte_2f` | Unknown |
| `0x30` | 4 | `alignment` | `0x10` |
| `0x34` | 12 | `reserved` | Zero in all measured external archives |

The validated ordering is:

```text
0x40 <= region_b_offset <= metadata_end <= file_size
```

All three sizes/offsets at `0x04`, `0x08`, and `0x0c` are 16-byte aligned.
For mode 1, all 22 archives satisfy exactly:

```text
region_b_size = align16(entry_count * (8 + index_width))
```

The encrypted head occupies `[0x40, metadata_end)`. The encrypted trailer
occupies `[file_size - trailer_size, file_size)`. Bytes between those regions
include file payloads, alignment, and nested RPF9 containers.

## First metadata span

The native loader derives the first encrypted span length as:

```text
align16(u16(header + 0x1a) * 8)
```

For this build, `u16(header + 0x1a)` is 4, so the span is 0x20 bytes beginning
at file offset 0x40. Decrypting this span is the current reproducible proof
that the selected material and block primitive are correct. It is not yet a
complete TOC decode.

## TFIT-CBC material

Crypto profile 9 uses a statically linked TFIT block transform in CBC mode.
The usable material is not an AES-style 16- or 32-byte key. It consists of:

- a 0x10f4-byte expanded key blob;
- a 16-byte IV;
- 0x5000 bytes of fixed decryption tables.

The key blob begins with magic `0x4e3b9b8e`, followed by 60 little-endian
round words and sixteen 256-byte final substitution tables. The fixed material
contains sixteen 256-byte initial tables and sixteen 256-entry `u32` round
tables.

The recovered native path for this build is:

```text
crypto profile selection
  -> TFIT-CBC provider 0x1455dbcf0
     -> block transform 0x145642c50
     -> CBC XOR helper 0x145643140
```

The block transform performs an initial table substitution, thirteen crossed
table rounds, then a final substitution/permutation. CBC XORs the first
transformed block with the IV and each following block with the preceding
ciphertext block.

## Material extraction from the user's PE

OpenRAGE does not ship the key blob, IV, tables, executable bytes, or archive
bytes. `rpf9_exe::extract_scarlett_1_0_2_1` parses the user's PE32+ section
table, maps virtual addresses to file offsets, and reads the material from the
analysed build.

The key registry contains 24 descriptors of three little-endian `u64` values:

```text
key_id, data_va, size
```

The RPF9 descriptor has ID `0x1000000102000000`. Its pointed data must have
size 0x10f4 and the expected blob magic. The implementation then verifies
SHA-256 fingerprints before allowing decryption:

| Component | VA/source | SHA-256 |
|---|---:|---|
| Descriptor registry | `0x1466e5900` (24 records) | structural validation |
| Key blob | descriptor-selected | `d7a6d8ddcba95fc4e5ce32ead7720435d80d8c4743f01cd50458defd38dd8db5` |
| IV | `0x1462fd470` | `70ad3893679946abe55fa553f19856c1f81401bee02380885084a418c64ff2e3` |
| Fixed tables | `0x146036f30` | `39dff6172873bac1b93b0aa26d2e31146404f3f40c2261492c9c45b313f1e38e` |

These virtual addresses and fingerprints are build-specific. A mismatch is a
hard error, not permission to guess offsets or decrypt with unverified data.

## Validation coverage

The implementation was exercised against the analysed PE and all 22 external
RPF9 archives in the corpus:

- all 22 passed the header ordering, alignment, width, constants, and compact
  region-B checks;
- all 22 selected crypto profile 9;
- PE extraction found exactly one matching key descriptor;
- the key blob, IV, and fixed tables passed their fingerprints;
- all 22 decrypted the first 0x20-byte metadata span to the same validated
  structured result.

Unit tests committed to the crate use only synthetic headers and malformed PE
inputs. They do not include game-derived fixtures or cryptographic material.

## CLI and library use

Header-only inspection:

```text
openrage rpf9 common.rpf
```

Static material validation and first-span decryption using the user's PE:

```text
openrage rpf9 common.rpf --exe game_scarlett_master.exe
```

The library entry points are `rpf9::read_header`,
`rpf9_exe::extract_scarlett_1_0_2_1`, and
`rpf9::decrypt_first_metadata_span`.

## Known limits

The following are deliberately not exposed as working features:

- full TOC record boundaries and the mapping between metadata views;
- name/hash resolution;
- payload offsets, compression flags, resource flags, and per-entry crypto;
- trailer plaintext structure and authentication/integrity semantics;
- safe file extraction;
- nested archive traversal through the VFS;
- editing, repacking, or writing RPF9;
- other game builds or platforms.

Until those pieces are established from code and cross-archive evidence,
RPF9 must not be parsed as RPF7 or RPF8 merely because individual values look
plausible.
