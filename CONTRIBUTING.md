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
- Update the documentation in [`site/`](site/) when the change affects users.

Building from source, the build environment and the project layout are described on the [development page](https://two-play.github.io/openrage/development/).

## What we cannot accept

OpenRAGE contains no game files and no encryption keys, and it has to stay that way. Please never commit or attach:

- game files, executables or encryption keys,
- assets extracted from a game (models, textures, sounds, …),
- leaked or otherwise unlawfully obtained material.

Large generated files, such as name dictionaries, should be discussed first; they may be better shipped separately.

## Licence

By contributing you agree that your contribution is licensed under the licence of the part of the project you change (see [LICENSE](LICENSE) and the licence section of the [README](README.md#license)).
