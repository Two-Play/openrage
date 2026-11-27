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

## Icons on this website

The icons for bugs, code and pull requests come from [Lucide](https://lucide.dev) (ISC license, copyright Lucide
Contributors, portions copyright Cole Bemis as part of Feather). The Linux, Apple and Matrix logos come from
[Simple Icons](https://simpleicons.org) (CC0 1.0); they remain trademarks of their owners and only show which systems
and services OpenRAGE works with.

<details>
<summary>Lucide license (ISC)</summary>

```
Copyright (c) for portions of Lucide are held by Cole Bemis 2013-2022 as part of Feather (MIT).
All other copyright (c) for Lucide are held by Lucide Contributors 2022.

Permission to use, copy, modify, and/or distribute this software for any
purpose with or without fee is hereby granted, provided that the above
copyright notice and this permission notice appear in all copies.

THE SOFTWARE IS PROVIDED "AS IS" AND THE AUTHOR DISCLAIMS ALL WARRANTIES
WITH REGARD TO THIS SOFTWARE INCLUDING ALL IMPLIED WARRANTIES OF
MERCHANTABILITY AND FITNESS. IN NO EVENT SHALL THE AUTHOR BE LIABLE FOR
ANY SPECIAL, DIRECT, INDIRECT, OR CONSEQUENTIAL DAMAGES OR ANY DAMAGES
WHATSOEVER RESULTING FROM LOSS OF USE, DATA OR PROFITS, WHETHER IN AN
ACTION OF CONTRACT, NEGLIGENCE OR OTHER TORTIOUS ACTION, ARISING OUT OF
OR IN CONNECTION WITH THE USE OR PERFORMANCE OF THIS SOFTWARE.
```

</details>

## How licenses are checked

Every dependency must be under a license that can be combined with the GPL-3.0: CI runs
[cargo-deny](https://github.com/EmbarkStudios/cargo-deny) on every change and fails if a new dependency brings in
another license. The list below is generated from `Cargo.lock` with
[cargo-about](https://github.com/EmbarkStudios/cargo-about), from the license files the crates ship; CI also checks
that it is current.
