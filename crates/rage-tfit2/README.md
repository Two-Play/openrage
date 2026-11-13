# rage-tfit2

The TFIT2 white-box cipher (CBC mode) and the strided file encryption of Red
Dead Redemption 2 PC archives (RPF8), part of
[OpenRAGE](https://github.com/Two-Play/openrage). Used by `rage-rpf` with its
`rdr2` feature.

No key material is included: the cipher tables, IV and keys come from the
user's own game at runtime.

## Licence

**AGPL-3.0-only** (see `LICENSE`). This crate is a Rust port of
[RDR2-RPF-Tool](https://github.com/amrshaheen61/RDR2-RPF-Tool) by amrshaheen61
(`Core/Tfit2CbcCipher.cs`, `Core/StridedCipher.cs`), which is distributed
under the GNU Affero General Public License v3.0. Any program that links this
crate is subject to the AGPL.
