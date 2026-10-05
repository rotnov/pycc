#!/usr/bin/env python3
"""Check a built Pages site for well-formed HTML and valid links (D-259).

This is the only site gate: it checks structure, not wording, dates, sizes or
performance. Run it on the output of `scripts/build_site.py`:

    python3 scripts/build_site.py && python3 scripts/check_site.py _site

Checks:
  * every HTML page starts with a doctype and has balanced tags;
  * every internal href/src resolves to a built file, and every same-page
    `#fragment` names an element id on that page;
  * every sitemap `<loc>` is a unique URL under the site root that resolves;
  * robots.txt names the sitemap;
  * llms.txt starts with `# pycc` and a blockquote summary, its site links
    resolve, and its links into the repository's `main` branch name files
    that exist in the checkout;
  * 404.html exists.
"""

from __future__ import annotations

import re
import sys
import xml.etree.ElementTree as ET
from html.parser import HTMLParser
from pathlib import Path
from urllib.parse import unquote, urlsplit

SITE_URL = "https://rotnov.github.io/pycc/"
REPO_LINK_RE = re.compile(
    r"https://(?:raw\.githubusercontent\.com/rotnov/pycc/main|github\.com/rotnov/pycc/blob/main)/([^\s)>\]]+)"
)
MARKDOWN_LINK_RE = re.compile(r"\]\((https?://[^)\s]+)\)")
VOID_ELEMENTS = {
    "area", "base", "br", "col", "embed", "hr", "img", "input", "link",
    "meta", "param", "source", "track", "wbr",
}
SITEMAP_NS = "{http://www.sitemaps.org/schemas/sitemap/0.9}"


class PageParser(HTMLParser):
    def __init__(self) -> None:
        super().__init__(convert_charrefs=True)
        self.stack: list[tuple[str, int]] = []
        self.errors: list[str] = []
        self.links: list[str] = []
        self.ids: set[str] = set()

    def handle_starttag(self, tag, attrs):
        for name, value in attrs:
            if name == "id" and value:
                self.ids.add(value)
            if name in ("href", "src") and value is not None:
                self.links.append(value)
        if tag not in VOID_ELEMENTS:
            self.stack.append((tag, self.getpos()[0]))

    def handle_startendtag(self, tag, attrs):
        # `<br/>`-style syntax: record attributes without opening an element.
        self.handle_starttag(tag, attrs)
        if tag not in VOID_ELEMENTS:
            self.stack.pop()

    def handle_endtag(self, tag):
        if tag in VOID_ELEMENTS:
            self.errors.append(f"line {self.getpos()[0]}: end tag for void element <{tag}>")
            return
        if not self.stack:
            self.errors.append(f"line {self.getpos()[0]}: unexpected </{tag}>")
            return
        open_tag, line = self.stack.pop()
        if open_tag != tag:
            self.errors.append(
                f"line {self.getpos()[0]}: </{tag}> closes <{open_tag}> opened on line {line}"
            )


def resolve_target(site: Path, page: Path, path: str) -> Path | None:
    """Map a site-relative URL path to the file GitHub Pages would serve."""
    if path.startswith("/"):
        if not path.startswith("/pycc/"):
            return None
        target = site / path[len("/pycc/"):]
    else:
        target = page.parent / path
    target = Path(unquote(str(target)))
    if target.is_dir() or str(path).endswith("/"):
        target = target / "index.html"
    return target


def check_html(site: Path, page: Path, errors: list[str]) -> None:
    rel = page.relative_to(site)
    text = page.read_text(encoding="utf-8")
    if not text.lstrip().lower().startswith("<!doctype html>"):
        errors.append(f"{rel}: missing <!doctype html>")
    parser = PageParser()
    parser.feed(text)
    parser.close()
    errors.extend(f"{rel}: {e}" for e in parser.errors)
    for tag, line in parser.stack:
        errors.append(f"{rel}: <{tag}> opened on line {line} is never closed")

    for link in parser.links:
        parts = urlsplit(link)
        if parts.scheme in ("mailto", "data", "javascript"):
            continue
        if parts.scheme or parts.netloc:
            if link.startswith(SITE_URL):
                check_site_url(site, link, f"{rel}", errors)
            continue
        if not parts.path:
            if parts.fragment and parts.fragment not in parser.ids:
                errors.append(f"{rel}: fragment #{parts.fragment} has no matching id")
            continue
        target = resolve_target(site, page, parts.path)
        if target is None or not target.is_file():
            errors.append(f"{rel}: broken link {link!r}")


def check_site_url(site: Path, url: str, where: str, errors: list[str]) -> None:
    path = urlsplit(url).path
    target = resolve_target(site, site / "index.html", path)
    if target is None or not target.is_file():
        errors.append(f"{where}: {url} does not resolve to a built file")


def check_sitemap(site: Path, errors: list[str]) -> None:
    sitemap = site / "sitemap.xml"
    if not sitemap.is_file():
        errors.append("sitemap.xml is missing")
        return
    try:
        root = ET.parse(sitemap).getroot()
    except ET.ParseError as error:
        errors.append(f"sitemap.xml is not well-formed XML: {error}")
        return
    locs = [loc.text.strip() if loc.text else "" for loc in root.iter(f"{SITEMAP_NS}loc")]
    if not locs:
        errors.append("sitemap.xml lists no <loc> URLs")
    if len(set(locs)) != len(locs):
        errors.append("sitemap.xml lists a URL more than once")
    for loc in locs:
        if not loc.startswith(SITE_URL):
            errors.append(f"sitemap.xml: {loc!r} is outside {SITE_URL}")
            continue
        check_site_url(site, loc, "sitemap.xml", errors)


def check_robots(site: Path, errors: list[str]) -> None:
    robots = site / "robots.txt"
    if not robots.is_file():
        errors.append("robots.txt is missing")
    elif f"Sitemap: {SITE_URL}sitemap.xml" not in robots.read_text(encoding="utf-8"):
        errors.append("robots.txt does not name the sitemap")


def check_llms(site: Path, repo_root: Path, errors: list[str]) -> None:
    llms = site / "llms.txt"
    if not llms.is_file():
        errors.append("llms.txt is missing")
        return
    text = llms.read_text(encoding="utf-8")
    lines = [line for line in text.splitlines() if line.strip()]
    if not lines or not lines[0].startswith("# pycc"):
        errors.append("llms.txt must start with '# pycc'")
    if len(lines) < 2 or not lines[1].startswith("> "):
        errors.append("llms.txt must follow its title with a '> ' summary")
    for url in MARKDOWN_LINK_RE.findall(text):
        if url.startswith(SITE_URL):
            check_site_url(site, url, "llms.txt", errors)
    for path in REPO_LINK_RE.findall(text):
        if not (repo_root / unquote(path)).exists():
            errors.append(f"llms.txt links to {path}, which does not exist in the repository")


def check(site: Path, repo_root: Path) -> list[str]:
    errors: list[str] = []
    if not site.is_dir():
        return [f"{site} is not a directory; run scripts/build_site.py first"]
    if not (site / "404.html").is_file():
        errors.append("404.html is missing")
    for page in sorted(site.rglob("*.html")):
        check_html(site, page, errors)
    check_sitemap(site, errors)
    check_robots(site, errors)
    check_llms(site, repo_root, errors)
    return errors


def main(argv: list[str] | None = None) -> int:
    argv = sys.argv[1:] if argv is None else argv
    repo_root = Path(__file__).resolve().parent.parent
    site = Path(argv[0]) if argv else repo_root / "_site"
    if len(argv) > 1:
        repo_root = Path(argv[1])
    errors = check(site, repo_root)
    for error in errors:
        print(f"check_site: {error}", file=sys.stderr)
    if errors:
        return 1
    print(f"check_site: {site} OK")
    return 0


if __name__ == "__main__":
    sys.exit(main())
