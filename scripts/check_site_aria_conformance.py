#!/usr/bin/env python3
"""W3C Nu ARIA conformance evaluator for issue #199.

Parses each served HTML body and rejects any ``<div>`` element carrying
``aria-label`` or ``aria-labelledby`` without an explicit non-``generic``
``role`` attribute.  This catches the W3C Nu validator's
``aria-label``/``aria-labelledby`` prohibition on ``generic``-role
elements without requiring a network request to validator.w3.org.

The check is deterministic: it uses Python's ``html.parser`` (the same
parser family ``check-site.sh`` already uses) to parse the HTML and
inspect each ``<div>`` element's attributes.

A ``<div>`` with ``aria-label`` or ``aria-labelledby`` is allowed only if
it also has a ``role`` attribute whose value is a WAI-ARIA 1.2 role that
permits naming via ``aria-label``/``aria-labelledby``.  Per the WAI-ARIA
1.2 spec (section 5.2.8.4 "Roles Supporting Name from Author"), the
following concrete roles support name from author and thus permit
``aria-label``/``aria-labelledby``: ``alert``, ``alertdialog``,
``application``, ``article``, ``banner``, ``blockquote``, ``button``,
``cell``, ``checkbox``, ``columnheader``, ``combobox``,
``complementary``, ``contentinfo``, ``definition``, ``dialog``,
``directory``, ``document``, ``feed``, ``figure``, ``form``, ``grid``,
``gridcell``, ``group``, ``heading``, ``img``, ``link``, ``list``,
``listbox``, ``listitem``, ``log``, ``main``, ``marquee``, ``math``,
``meter``, ``menu``, ``menubar``, ``menuitem``, ``menuitemcheckbox``,
``menuitemradio``, ``navigation``, ``note``, ``option``,
``progressbar``, ``radio``, ``radiogroup``, ``region``, ``row``,
``rowgroup``, ``rowheader``, ``scrollbar``, ``search``, ``searchbox``,
``separator``, ``slider``, ``spinbutton``, ``status``, ``switch``,
``tab``, ``table``, ``tablist``, ``tabpanel``, ``term``, ``textbox``,
``time``, ``timer``, ``toolbar``, ``tooltip``, ``tree``, ``treegrid``,
``treeitem``.

Roles that **prohibit** naming (``nameFrom: prohibited`` per section
5.2.8.6) — such as ``caption``, ``code``, ``deletion``, ``emphasis``,
``insertion``, ``paragraph``, ``strong``, ``subscript``,
``superscript``, ``generic``, ``none``, and ``presentation`` — are
excluded.  Abstract roles (``command``, ``composite``, ``input``,
``landmark``, ``range``, ``roletype``, ``section``, ``sectionhead``,
``select``, ``structure``, ``widget``, ``window``) are also excluded
because authors must not use them directly.

Usage::

    python3 scripts/check_site_aria_conformance.py --site-dir site

Exit codes:
  0 — all pages pass the ARIA conformance check
  1 — one or more pages have ARIA conformance violations
  2 — configuration or structural error
"""

from __future__ import annotations

import argparse
import json
import sys
from html.parser import HTMLParser
from pathlib import Path
from typing import Any

# Roles that permit naming via aria-label/aria-labelledby per WAI-ARIA 1.2
# section 5.2.8.4 ("Roles Supporting Name from Author").  This excludes:
#   - Roles with nameFrom: prohibited (section 5.2.8.6): caption, code,
#     deletion, emphasis, generic, insertion, paragraph, presentation,
#     strong, subscript, superscript.
#   - "none" (synonym for presentation, also prohibits naming).
#   - Abstract roles (section 5.3.1): command, composite, input, landmark,
#     range, roletype, section, sectionhead, select, structure, widget,
#     window — authors must not use these directly.
# See https://www.w3.org/TR/wai-aria-1.2/#namefromauthor
NAME_PERMITTED_ROLES = frozenset({
    "alert", "alertdialog", "application", "article", "banner",
    "blockquote", "button", "cell", "checkbox", "columnheader",
    "combobox", "complementary", "contentinfo", "definition", "dialog",
    "directory", "document", "feed", "figure", "form", "grid", "gridcell",
    "group", "heading", "img", "link", "list", "listbox", "listitem",
    "log", "main", "marquee", "math", "meter", "menu", "menubar",
    "menuitem", "menuitemcheckbox", "menuitemradio", "navigation", "note",
    "option", "progressbar", "radio", "radiogroup", "region", "row",
    "rowgroup", "rowheader", "scrollbar", "search", "searchbox",
    "separator", "slider", "spinbutton", "status", "switch", "tab",
    "table", "tablist", "tabpanel", "term", "textbox", "time", "timer",
    "toolbar", "tooltip", "tree", "treegrid", "treeitem",
})

MANIFEST_DEFAULT = "tests/fixtures/pages-performance-manifest.json"


class AriaConformanceChecker(HTMLParser):
    """Parse HTML and collect ARIA conformance violations on <div> elements."""

    def __init__(self) -> None:
        super().__init__()
        self.violations: list[str] = []

    def handle_starttag(self, tag: str, attrs: list[tuple[str, str | None]]) -> None:
        if tag.lower() != "div":
            return

        attr_dict = {k.lower(): (v or "") for k, v in attrs}
        has_aria_label = "aria-label" in attr_dict and attr_dict["aria-label"]
        has_aria_labelledby = "aria-labelledby" in attr_dict and attr_dict["aria-labelledby"]

        if not has_aria_label and not has_aria_labelledby:
            return

        role = attr_dict.get("role", "").strip().lower()
        if role and role in NAME_PERMITTED_ROLES:
            return

        # Violation: div with aria-label/aria-labelledby but no permitted role
        naming_attr = "aria-label" if has_aria_label else "aria-labelledby"
        attr_value = attr_dict.get(naming_attr, "")
        role_desc = f"role={role!r}" if role else "no explicit role"
        self.violations.append(
            f"<div> with {naming_attr}={attr_value!r} has {role_desc} "
            f"(generic roles do not support {naming_attr})"
        )


def check_html(html: str) -> list[str]:
    """Check a single HTML document for ARIA conformance violations.

    Returns a list of violation description strings (empty if clean).
    """
    checker = AriaConformanceChecker()
    checker.feed(html)
    checker.close()
    return checker.violations


def load_manifest(manifest_path: str) -> dict[str, Any]:
    path = Path(manifest_path)
    if not path.is_file():
        raise SystemExit(f"manifest not found: {manifest_path}")
    return json.loads(path.read_text())


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        description="W3C Nu ARIA conformance evaluator for the site accessibility gate."
    )
    parser.add_argument(
        "--manifest",
        default=MANIFEST_DEFAULT,
        help="Path to the page-identity manifest JSON.",
    )
    parser.add_argument(
        "--site-dir",
        default="site",
        help="Directory containing the site tree (unused; source paths are repo-relative).",
    )
    args = parser.parse_args(argv)

    manifest = load_manifest(args.manifest)
    all_failures: list[str] = []

    for page in manifest["canonical_pages"]:
        page_id = page["id"]
        source_path = Path(page["source_artifact"])
        if not source_path.is_file():
            all_failures.append(f"{page_id}: source artifact not found: {source_path}")
            continue

        html = source_path.read_text(encoding="utf-8")
        violations = check_html(html)
        for v in violations:
            all_failures.append(f"{page_id}: {v}")

    if all_failures:
        for f in all_failures:
            print(f"FAIL: {f}", file=sys.stderr)
        return 1

    print("OK: ARIA conformance check passed for all canonical pages")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
