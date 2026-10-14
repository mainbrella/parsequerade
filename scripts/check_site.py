#!/usr/bin/env python3
"""Check local documentation routes, assets, and in-page anchors."""

from html.parser import HTMLParser
from pathlib import Path
from urllib.parse import unquote, urlparse

ROOT = Path(__file__).resolve().parents[1]
ASSETS = ROOT / "dist"


class Page(HTMLParser):
    def __init__(self, path):
        super().__init__()
        self.path = path
        self.ids = set()
        self.links = []

    def handle_starttag(self, tag, attrs):
        attrs = dict(attrs)
        if attrs.get("id"):
            self.ids.add(attrs["id"])
        for key in ("href", "src"):
            value = attrs.get(key)
            if value:
                self.links.append(value)


def local_target(page_path, url):
    parsed = urlparse(url)
    if parsed.scheme or parsed.netloc or parsed.path.startswith("//"):
        return None
    if parsed.path.startswith("/"):
        target = ASSETS / unquote(parsed.path.lstrip("/"))
    elif parsed.path:
        target = (page_path.parent / unquote(parsed.path)).resolve()
    else:
        target = page_path
    if target.is_dir():
        target /= "index.html"
    return target, unquote(parsed.fragment)


def main():
    pages = sorted(ASSETS.rglob("*.html"))
    parsed_pages = {}
    for page_path in pages:
        page = Page(page_path)
        page.feed(page_path.read_text(encoding="utf-8"))
        parsed_pages[page_path.resolve()] = page

    for page_path, page in parsed_pages.items():
        for link in page.links:
            resolved = local_target(page_path, link)
            if resolved is None:
                continue
            target, fragment = resolved
            target = target.resolve()
            if not target.is_file():
                raise ValueError(f"{page_path.relative_to(ROOT)}: missing local target {link}")
            if fragment and target.suffix.lower() == ".html":
                target_page = parsed_pages.get(target)
                if target_page is None:
                    target_page = Page(target)
                    target_page.feed(target.read_text(encoding="utf-8"))
                if fragment not in target_page.ids:
                    raise ValueError(f"{page_path.relative_to(ROOT)}: missing anchor {link}")

    print(f"Static site checks passed ({len(pages)} pages).")


if __name__ == "__main__":
    main()
