# Scarlett native name lookup

The target is the executable identified in [rpf9-scarlett.md](rpf9-scarlett.md).
These observations use offline GNU objdump disassembly, original-file byte
reads, and isolated calls to three pure hash leaves in private mapped memory.
They are not observations of the running game. No original game file was
written, and no executable entry point or initialization code was called.

## Lookup observations

The function at `0x145305880` accepts a device and path string. It checks a
normalized prefix hash against device offset `0x70`, using a prefix length
from `0x78`, and then hashes the relative suffix. The suffix's last dot
selects an extension through `0x14056ee50`. A recognized extension is omitted
from the string hash and retained as a separate type. An unrecognized
extension remains part of the hashed string.

The path lookup eventually compares typed 32-bit or 64-bit hashes, through
`0x14056ec70` and `0x14056f130`. The getters reveal the index descriptor IDs:

| Getter | Descriptor type hash | Value |
| --- | --- | --- |
| `0x145305ee0` | `0xb7076b6245d35fb5` | u32 name hash |
| `0x145520ac0` | `0x73bcb72dce74f11f` | u64 name hash |

These lookup paths do not compare original filename strings. Decrypting the
index therefore does not by itself reverse these hashes into all original
paths. Candidate names can be checked against the hash and type; original
spelling and case are separate unknowns. This bounded lookup inspection does
not establish that no other archive structure or metadata contains names.

## Hash leaves and comparison

The native normalization table at `0x1465b23b0` changes only ASCII `A`–`Z`
to `a`–`z` and backslash to slash. All other byte values are unchanged.

| Leaf address | Algorithm |
| --- | --- |
| `0x144fc21d0` | Jenkins one-at-a-time over normalized bytes; seed zero at this caller |
| `0x144f66de0` | Normalized MurmurHash3 x64-128, seed zero, returning h1 |
| `0x144f67220` | Raw MurmurHash3 x64-128, seed zero, returning h1 |

The two Murmur leaves XOR only the low byte of the string length into the
final states. This differs from ordinary MurmurHash3 for lengths >= 256.
The Jenkins leaf has a C-string quote rule: if the first byte is a double
quote it skips that byte and stops at the next quote or NUL. Murmur has no
such quote rule. Every leaf stops at the first NUL.

The native 64-bit wrapper `0x144f66800` chooses a registered function pointer
at `registry + mode * 0x58 + 0x28`. Static initialization at `0x145c35461`
loads the registry pointer, puts the raw leaf at offset `0x28`, and puts the
normalized leaf at offset `0x80` (`0x145c35491`). Thus the inspected
initialization configures mode 0 as raw and mode 1 as normalized. The native
path lookup selects mode 1. This is static initialization evidence, not a
recording of the running game's registration state.

A small Linux C harness mapped PE sections privately at the preferred image
base, replaced the imported `strlen` trampoline in that private copy with
a local Microsoft-ABI `strlen` stub, and called only the three leaf addresses.
A separately written Python algorithm matched all 354 results for 118
synthetic inputs: empty/quoted/path strings and deterministic strings with
lengths 1–99, 127–129, 254–257, 511–513 and 1023–1024. An additional six
synthetic repeated-character vectors covered 15/16/17 and 255/256/257 bytes.
The Rust implementation also matched all 354 native outputs for the same
118 synthetic strings. The Rust tests retain selected synthetic inputs and numeric outputs, without
executable code, game data, normalization-table bytes, or keys.

## Static extension types

The built-in dispatcher compares `rpf` with type 0 and has three alias
families, with prefixes `z`, `i`, and `#`:

| Type | Suffix after the family prefix |
| ---: | --- |
| 1 | mf |
| 2 | dr |
| 3 | ft |
| 4 | dd |
| 5 | td |
| 6 | bn |
| 7 | bd |
| 8 | pd |
| 9 | bs |
| 10 | sd |
| 11 | mt |
| 12 | sc |
| 13 | cs |
| 14 | datb |
| 15 | bdr |
| 16 | bdd |
| 17 | bft |
| 18 | vert |

The dispatcher uses the imported C `strcmp` and can consult additional
registered callbacks. The registered types are numbered from 64 in the order
of three more pointer tables of 52 strings each, before VA `0x14618dec0` (the
`#` family; the `z` and `i` families precede it): `mrf`, `cut`, `gfx`, `#cd`,
`#ld`, … `#cov`. The numbers agree with the content of the archives (66
Scaleform, 67 clips, 72 particles, 79 navigation meshes, 89 scripts). `y`-prefixed GTA V extensions are not assumed to be
aliases. The Rust candidate helper lowercases extensions before matching as
an explicit candidate-generation convention, rather than claiming that the
native `strcmp` itself ignores case.

## Candidate collection

The executable scanner accepts NUL-terminated ASCII strings of 3–1024 bytes
containing a slash or a recognized static resource/media extension. It
normalizes slash direction and removes literal `mount:/` prefixes, consistent
with the inspected device suffix lookup. It synthesizes no directory or
extension guesses. Absolute paths, traversal, invalid filename characters
and format placeholders are rejected. The result is candidate text, never a
verified original name until its hashes and type match a catalog entry.
