+++
title = "Licenses"
description = "OpenRAGE's license, the projects it builds on, and the licenses of all third-party crates in the programs."
template = "licenses.html"

[extra]
lead = "OpenRAGE is free software under the GPL-3.0-only. This page lists the work it builds on and the license of every third-party crate in the programs."
+++

## OpenRAGE

OpenRAGE is licensed under the [GNU General Public License, version 3](https://github.com/Two-Play/openrage/blob/main/LICENSE)
(GPL-3.0-only). The TFIT2 cipher in `crates/rage-tfit2`, ported from RDR2-RPF-Tool, stays under the
[GNU Affero General Public License, version 3](https://github.com/Two-Play/openrage/blob/main/crates/rage-tfit2/LICENSE).
It ships no game files and no encryption keys. Format knowledge and ported code from other projects are credited in
[NOTICE.md](https://github.com/Two-Play/openrage/blob/main/NOTICE.md).

The same information is in the app (**Help → About OpenRAGE**), on the command line (`openrage licenses`) and in
`THIRD_PARTY_LICENSES.md`, which comes with every download.

## How licenses are checked

Every dependency must be under a license that can be combined with the GPL-3.0: CI runs
[cargo-deny](https://github.com/EmbarkStudios/cargo-deny) on every change and fails if a new dependency brings in
another license. The list below is generated from `Cargo.lock` with
[cargo-about](https://github.com/EmbarkStudios/cargo-about), from the license files the crates ship; CI also checks
that it is current.
