# OpenRAGE website

Static source of <https://two-play.github.io/openrage/>: plain HTML and CSS, with a small
script for the theme toggle and the latest-release links (the site works without it).

The screenshots live in `docs/images/` and are copied to `images/` by the
[Pages workflow](../.github/workflows/pages.yml) on every push to `main` that changes the site.
To preview locally:

```sh
rm -rf /tmp/openrage-site && cp -r site /tmp/openrage-site && mkdir -p /tmp/openrage-site/images \
  && cp docs/images/*.png /tmp/openrage-site/images/ && python3 -m http.server -d /tmp/openrage-site 8000
```
