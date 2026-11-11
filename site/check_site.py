#!/usr/bin/env python3
"""Checks the built website (zola build output) without network access.

Every link and image that points into the site must exist, every #anchor
must exist on its page, every image needs an alt attribute and every page
needs a title and exactly one <h1>.

Usage: check_site.py <public dir> <base url>
"""

import sys
from html.parser import HTMLParser
from pathlib import Path
from urllib.parse import urldefrag, unquote, urlsplit


class Page(HTMLParser):
    def __init__(self):
        super().__init__(convert_charrefs=True)
        self.links = []   # (attribute, url)
        self.ids = set()
        self.problems = []
        self.h1 = 0
        self.title = ""
        self._in_title = False

    def handle_starttag(self, tag, attrs):
        a = dict(attrs)
        if "id" in a:
            self.ids.add(a["id"])
        if tag == "a" and "name" in a:
            self.ids.add(a["name"])
        for attr in ("href", "src"):
            if a.get(attr) and tag in ("a", "link", "img", "script", "source"):
                self.links.append((attr, a[attr]))
        if tag == "img" and "alt" not in a:
            self.problems.append(f"<img src={a.get('src')!r}> has no alt attribute")
        if tag == "h1":
            self.h1 += 1
        if tag == "title":
            self._in_title = True

    def handle_endtag(self, tag):
        if tag == "title":
            self._in_title = False

    def handle_data(self, data):
        if self._in_title:
            self.title += data


def target_file(public: Path, path: str) -> Path:
    p = public / unquote(path).lstrip("/")
    if path == "" or path.endswith("/") or p.is_dir():
        p = p / "index.html"
    return p


def main() -> int:
    if len(sys.argv) != 3:
        print(__doc__)
        return 2
    public = Path(sys.argv[1])
    base = sys.argv[2].rstrip("/")
    base_path = urlsplit(base).path

    pages = {}
    for f in sorted(public.rglob("*.html")):
        parser = Page()
        parser.feed(f.read_text(encoding="utf-8"))
        pages[f.resolve()] = parser

    errors = []
    for f, page in pages.items():
        rel = f.relative_to(public.resolve())
        for problem in page.problems:
            errors.append(f"{rel}: {problem}")
        if not page.title.strip():
            errors.append(f"{rel}: empty <title>")
        if page.h1 != 1 and rel.name != "404.html":
            errors.append(f"{rel}: {page.h1} <h1> elements, expected 1")

        for attr, url in page.links:
            if url.startswith(base + "/") or url == base:
                path = urlsplit(url).path[len(base_path):]
            elif url.startswith(("http://", "https://", "mailto:", "data:", "//")):
                continue  # external
            elif url.startswith("#"):
                path = None
            else:
                errors.append(f"{rel}: relative URL {url!r}; use get_url() so it works under the base path")
                continue

            url_nofrag, frag = urldefrag(url)
            if path is None:
                target = f
            else:
                target = target_file(public, urlsplit(url_nofrag).path[len(base_path):]).resolve()
                if not target.exists():
                    errors.append(f"{rel}: {attr}={url!r} points to a missing file")
                    continue
            if frag and target in pages and frag not in pages[target].ids:
                errors.append(f"{rel}: {attr}={url!r}: no element with id {frag!r}")

    for e in errors:
        print(f"error: {e}")
    print(f"checked {len(pages)} pages, {sum(len(p.links) for p in pages.values())} links: "
          f"{'OK' if not errors else str(len(errors)) + ' problem(s)'}")
    return 1 if errors else 0


if __name__ == "__main__":
    sys.exit(main())
