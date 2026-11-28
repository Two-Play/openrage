# Contributing to OpenRAGE

Thanks for wanting to help! Bug reports, ideas, documentation, testing on your own game and code are all welcome.

## Talk to us

- **Matrix:** [#openRAGE:nope.chat](https://matrix.to/#/#openRAGE:nope.chat) for questions, help and development.
- **GitHub Issues:** for bugs that can be reproduced and for feature requests. Please use the templates.

For bigger changes (a new game, a new archive format, a large refactoring), open an issue or ask on Matrix first, so that we can agree on the approach before you put a lot of work in.

## Pull requests

- Work goes into **`develop`**; `main` only takes releases. Please target `develop`.
- Keep pull requests focused: one feature or fix each. Large changes are easier to review in several smaller steps.
- Run `cargo fmt --all --check` and `cargo test` before opening the pull request.
- Update the documentation in the [website repository](https://github.com/openRAGE-org/website) when the change affects users.

Building from source, the build environment and the project layout are described on the [development page](https://openrage.dev/development/).

## What we cannot accept

OpenRAGE contains no game files and no encryption keys, and it has to stay that way. Please never commit or attach:

- game files, executables or encryption keys,
- assets extracted from a game (models, textures, sounds, …),
- leaked or otherwise unlawfully obtained material.

Large generated files, such as name dictionaries, should be discussed first; they may be better shipped separately.

## Licences and sign-off

OpenRAGE has three licences, depending on the part of the project:

| Part | Licence |
|---|---|
| Everything not listed below | GPL-3.0-only |
| `crates/rage-tfit2` | AGPL-3.0-only |
| Plugin interface and SDK (`crates/openrage-plugins/wit`, `plugins/`) | MIT OR Apache-2.0 |

**Your contribution is licensed under the licence of the files you change** ("inbound = outbound"). You keep your copyright; there is no contributor licence agreement (CLA).

### Developer Certificate of Origin

Every commit has to be signed off under the [Developer Certificate of Origin 1.1](https://developercertificate.org/). With the sign-off you confirm that you wrote the change yourself, or otherwise have the right to submit it under the project's licence. Add it with `-s`:

```sh
git commit -s -m "Your message"
```

This adds a line with your name and e-mail address to the commit message:

```
Signed-off-by: Your Name <you@example.com>
```

A check on every pull request makes sure each commit carries a sign-off with its author's e-mail address. Use a name you are known by; a pseudonym is fine. Forgot it? `git commit --amend -s` fixes the last commit, `git rebase --signoff develop` all commits of your branch.

### Code from elsewhere

- Code from other projects is welcome if its licence is compatible with the part you add it to. Keep its copyright notice, name the source in the commit message and add it to [NOTICE.md](NOTICE.md).
- Do not copy code whose licence is unknown or incompatible, and no code decompiled or copied from a game or from closed-source tools. Findings from reverse engineering (how a format is laid out) are fine; describe them in your own code.
- New dependencies must pass the licence check (`cargo deny check licenses`, also run in CI).
