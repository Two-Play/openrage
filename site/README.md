# OpenRAGE website

Source of <https://two-play.github.io/openrage/>, built with [Zola](https://www.getzola.org) (one binary, no other tools).

| What | Where |
|---|---|
| Landing page text, highlights, screenshots, supported games | `content/_index.md` (front matter + Markdown) |
| Getting started | `content/getting-started.md` |
| New page | add `content/<name>.md` with `title` and `description` in the front matter |
| Layout | `templates/` |
| CSS, theme toggle, release links | `static/` |
| Screenshots | `../docs/images/` (copied to `static/images/` at build time, not committed here) |

Markdown supports GitHub-style alerts (`> [!NOTE]`, `> [!WARNING]`) and `{{ <downloads /> }}` for the
download buttons of the latest release.

## Preview locally

Install Zola (the version is pinned in [`.github/workflows/pages.yml`](../.github/workflows/pages.yml)), e.g. the
release binary from <https://github.com/getzola/zola/releases>, then:

```sh
mkdir -p site/static/images && cp docs/images/*.png site/static/images/
cd site && zola serve          # http://127.0.0.1:1111, reloads on save
```

## Checks and deployment

The *Website* workflow runs on every pull request that touches `site/`: `zola check` (content, internal links;
external links only warn), `zola build`, and `check_site.py` (links, anchors, alt texts, titles in the built HTML).
Pushes to `main` also deploy the site to GitHub Pages. Site-only changes do not run the Rust CI.
