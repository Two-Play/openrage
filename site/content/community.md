+++
title = "Community and contributing"
description = "Talk to the OpenRAGE community on Matrix, report bugs, and contribute code, documentation and tests."

[extra]
docs = true
lead = "Questions, ideas, bug reports and pull requests are all welcome."
toc = true
+++

## Chat on Matrix

The OpenRAGE community meets on [Matrix](https://matrix.org), an open, decentralised chat network:
**[#openRAGE:nope.chat](https://matrix.to/#/#openRAGE:nope.chat)**.

Ask for help with the setup, discuss ideas, show what you made, or talk about file formats and development.
New to Matrix? Open the link and pick a client, for example [Element](https://element.io/download) (web, desktop and
phone). Creating an account is free, and you can use any Matrix server.

> [!NOTE]
> Please never share game files, executables or encryption keys in the chat. OpenRAGE takes them from your own copy
> of the game.

## Report a bug or suggest a feature

Bugs that can be reproduced and feature requests go to
[GitHub Issues](https://github.com/Two-Play/openrage/issues/new/choose). The templates ask for what helps most: the
game and its edition, your OpenRAGE version (**Help → About OpenRAGE**), your system, and the mods in your game
folder. Mods such as ASI loaders or Script Hook can change what OpenRAGE detects.

## Contribute

Code, documentation, notes on file formats and testing on your own game all help. How to build OpenRAGE is
described on the [development page](@/development.md); the rules for pull requests are in
[CONTRIBUTING.md](https://github.com/Two-Play/openrage/blob/develop/CONTRIBUTING.md). In short:

- Pull requests go into `develop`. Keep them focused, and discuss large changes on Matrix or in an issue first.
- Run `cargo fmt --all --check` and `cargo test` before opening a pull request.
- No game files, keys, extracted assets or leaked material, and no code decompiled from a game or a closed-source tool.

## Licences of contributions

Your contribution is licensed under the licence of the files you change: GPL-3.0-only for most of the project,
AGPL-3.0-only for `crates/rage-tfit2`, and MIT OR Apache-2.0 for the plugin interface and SDK (see
[Licenses](@/licenses.md)). You keep your copyright; there is no contributor licence agreement.

Every commit is signed off under the [Developer Certificate of Origin](https://developercertificate.org/), confirming
that you have the right to submit it:

```
git commit -s -m "Your message"
```
