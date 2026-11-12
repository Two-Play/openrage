# Third-party notices

## CodeWalker

Format knowledge for RPF7 and the NG cipher, the audio container layout and
Rockstar's XXTEA variant, the metadata formats (RSC meta, PSO, RBF) and their XML
layout, the layout of expression dictionaries (`.yed`), the SHA-1 fingerprints in `crates/rage-crypto/src/key_hashes.rs`, and the
list of metadata names in `crates/rage-formats/src/meta_names.txt` (from
CodeWalker's `MetaNames`) are derived from CodeWalker
(https://github.com/dexyfex/CodeWalker), which is distributed under the MIT license:

```
Copyright (c) 2015 Neodymium
Copyright (c) 2017-2019 dexyfex

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in
all copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN
THE SOFTWARE.
```

The fingerprints are SHA-1 hashes used to locate key material inside the
user's own game executable. They do not contain or allow reconstruction of any key.

## Trademarks

Grand Theft Auto, Red Dead Redemption, RAGE and Rockstar Games are trademarks of
Take-Two Interactive Software, Inc. OpenRAGE is not affiliated with them.

## Symphonia

MPEG audio is decoded with Symphonia (https://github.com/pdeljanov/Symphonia),
licensed under the Mozilla Public License 2.0; its source is available there.

## lewton

Vorbis audio (Red Dead Redemption 2) is decoded with lewton
(https://github.com/RustAudio/lewton), licensed under the MIT or Apache-2.0
license.

## RDR2-RPF-Tool

The TFIT2-CBC cipher and the strided file encryption of Red Dead Redemption 2
PC archives in `crates/rage-tfit2` are ported from RDR2-RPF-Tool by amrshaheen61
(https://github.com/amrshaheen61/RDR2-RPF-Tool, `Core/Tfit2CbcCipher.cs` and
`Core/StridedCipher.cs`), which is distributed under the GNU Affero General
Public License v3.0. That crate stays under the AGPL-3.0 (see
`crates/rage-tfit2/LICENSE`); GPL-3.0 section 13 permits combining it with the
rest of OpenRAGE. No key material from that project is used: OpenRAGE takes the
cipher tables, IV and keys from the user's own game.
