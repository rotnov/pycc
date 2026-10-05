#!/usr/bin/env python3
"""Build the GitHub Pages site from `site/` plus repository data.

The site is deliberately small (D-259): a hand-written landing page, a status
page generated at build time from `docs/ROADMAP.md`, `llms.txt`, `sitemap.xml`,
`robots.txt` and a 404 page. Nothing in the published output carries a
hand-maintained date; the status page states the commit and commit date it was
generated from.

Usage:
    python3 scripts/build_site.py [--repo-root DIR] [--out DIR]

`--out` defaults to `<repo-root>/_site`, which is gitignored. The commit SHA
and date come from `git` unless `PYCC_SITE_COMMIT` and `PYCC_SITE_DATE` are
set (the tests use these so they never need a git checkout).
"""

from __future__ import annotations

import argparse
import html
import os
import re
import shutil
import subprocess
import sys
from pathlib import Path

REPO_URL = "https://github.com/rotnov/pycc"
ROADMAP_URL = f"{REPO_URL}/blob/main/docs/ROADMAP.md"

# Pages the previous seven-page site published. Each gets a noindex redirect
# stub so inbound links and search results keep resolving.
RETIRED_ROUTES = (
    "ai-native",
    "architecture",
    "diagnostics",
    "language-support",
    "python-aot-compilers",
)

MILESTONE_RE = re.compile(r"^\*\*(Current milestone:[^*]+)\*\*", re.MULTILINE)
STATUS_HEADER = "| Area | Status in this commit |"
BUILD_MARKER = ".build_site"


class BuildError(Exception):
    """Raised when the repository data cannot produce the site."""


def inline_markdown(text: str) -> str:
    """Render the small inline-Markdown subset the ROADMAP table cells use.

    Links keep only their text (stripped first, so a link around a code span
    keeps the span), `code` spans become <code>, bold/italic markers are
    dropped, and everything else is HTML-escaped.
    """
    text = re.sub(r"\[([^\]]*)\]\([^)]*\)", r"\1", text)
    parts = re.split(r"(`[^`]*`)", text)
    out = []
    for part in parts:
        if len(part) >= 2 and part.startswith("`") and part.endswith("`"):
            out.append(f"<code>{html.escape(part[1:-1])}</code>")
            continue
        part = part.replace("**", "").replace("__", "")
        out.append(html.escape(part))
    return "".join(out)


def parse_roadmap(text: str) -> tuple[str, list[tuple[str, str]]]:
    """Return the current-milestone sentence and the (area, status) rows."""
    match = MILESTONE_RE.search(text)
    if not match:
        raise BuildError("docs/ROADMAP.md has no '**Current milestone: ...**' line")
    milestone = match.group(1).strip()

    lines = text.splitlines()
    try:
        start = next(i for i, line in enumerate(lines) if line.startswith(STATUS_HEADER))
    except StopIteration as error:
        raise BuildError(
            f"docs/ROADMAP.md has no status table starting with {STATUS_HEADER!r}"
        ) from error

    rows: list[tuple[str, str]] = []
    for line in lines[start + 2 :]:
        if not line.startswith("|"):
            break
        cells = [
            cell.strip().replace("\\|", "|")
            for cell in re.split(r"(?<!\\)\|", line.strip().strip("|"))
        ]
        if len(cells) < 2 or not cells[0]:
            raise BuildError(f"malformed status table row: {line[:80]!r}")
        rows.append((cells[0], cells[1]))
    if not rows:
        raise BuildError("docs/ROADMAP.md status table has no rows")
    return milestone, rows


def commit_info(repo_root: Path) -> tuple[str, str]:
    """Return (full SHA, ISO date) for HEAD, or the PYCC_SITE_* overrides."""
    sha = os.environ.get("PYCC_SITE_COMMIT")
    date = os.environ.get("PYCC_SITE_DATE")
    if sha and date:
        return sha, date
    try:
        out = subprocess.run(
            ["git", "-C", str(repo_root), "log", "-1", "--format=%H %cs"],
            check=True,
            capture_output=True,
            text=True,
        ).stdout.split()
    except (OSError, subprocess.CalledProcessError) as error:
        raise BuildError(
            "cannot read the commit from git; set PYCC_SITE_COMMIT and PYCC_SITE_DATE"
        ) from error
    if len(out) != 2:
        raise BuildError("unexpected `git log` output")
    return out[0], out[1]


def page(title: str, description: str, body: str, *, root: str, extra_head: str = "") -> str:
    return f"""<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>{html.escape(title)}</title>
<meta name="description" content="{html.escape(description)}">
<link rel="icon" href="{root}favicon.svg" type="image/svg+xml">
<link rel="stylesheet" href="{root}styles.css">
{extra_head}</head>
<body>
<header class="top"><a class="brand" href="{root}">pycc</a>
<nav><a href="{root}status/">Status</a> <a href="{REPO_URL}">GitHub</a></nav></header>
<main>
{body}
</main>
<footer><p>MIT licensed. Source: <a href="{REPO_URL}">github.com/rotnov/pycc</a>.</p></footer>
</body>
</html>
"""


def render_status(milestone: str, rows: list[tuple[str, str]], sha: str, date: str) -> str:
    table_rows = "\n".join(
        f"<tr><th scope=\"row\">{inline_markdown(area)}</th><td>{inline_markdown(status)}</td></tr>"
        for area, status in rows
    )
    short = html.escape(sha[:12])
    body = f"""<h1>Implementation status</h1>
<p class="lede">{inline_markdown(milestone)}</p>
<p>Generated from <a href="{ROADMAP_URL}"><code>docs/ROADMAP.md</code></a> at commit
<a href="{REPO_URL}/commit/{html.escape(sha)}"><code>{short}</code></a> ({html.escape(date)}).
The roadmap carries the evidence and remaining gap for every row.</p>
<table>
<thead><tr><th scope="col">Area</th><th scope="col">Status</th></tr></thead>
<tbody>
{table_rows}
</tbody>
</table>"""
    return page(
        "pycc status",
        "Current implementation status of pycc, generated from the repository roadmap.",
        body,
        root="../",
    )


def render_redirect(route: str) -> str:
    return f"""<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<title>pycc</title>
<meta name="robots" content="noindex">
<meta http-equiv="refresh" content="0; url=../">
<link rel="canonical" href="https://rotnov.github.io/pycc/">
</head>
<body>
<p>The {html.escape(route)} page was retired. Continue to <a href="../">pycc</a>.</p>
</body>
</html>
"""


def build(repo_root: Path, out: Path) -> None:
    source = repo_root / "site"
    if not source.is_dir():
        raise BuildError(f"missing site source directory: {source}")
    milestone, rows = parse_roadmap((repo_root / "docs" / "ROADMAP.md").read_text(encoding="utf-8"))
    sha, date = commit_info(repo_root)

    if out.exists():
        # Only ever delete a directory an earlier build produced, so a mistyped
        # --out (the repository root, docs/, site/) can never remove sources.
        if not (out / BUILD_MARKER).is_file():
            raise BuildError(
                f"refusing to replace {out}: it exists and was not produced by build_site.py"
            )
        shutil.rmtree(out)
    if source.resolve() in out.resolve().parents:
        raise BuildError(f"refusing to build into {out}: it is inside site/")
    shutil.copytree(source, out)
    (out / BUILD_MARKER).write_text("generated by scripts/build_site.py\n", encoding="utf-8")

    for generated in ["status", *RETIRED_ROUTES]:
        if (out / generated).exists():
            raise BuildError(f"site/{generated}/ is generated at build time; remove it from site/")

    status_dir = out / "status"
    status_dir.mkdir()
    (status_dir / "index.html").write_text(render_status(milestone, rows, sha, date), encoding="utf-8")
    for route in RETIRED_ROUTES:
        (out / route).mkdir()
        (out / route / "index.html").write_text(render_redirect(route), encoding="utf-8")


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--repo-root", type=Path, default=Path(__file__).resolve().parent.parent)
    parser.add_argument("--out", type=Path)
    args = parser.parse_args(argv)
    out = args.out or args.repo_root / "_site"
    try:
        build(args.repo_root, out)
    except BuildError as error:
        print(f"build_site: {error}", file=sys.stderr)
        return 1
    print(f"build_site: wrote {out}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
