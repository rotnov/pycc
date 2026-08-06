#!/usr/bin/env python3
"""Rewrite every 'DECISIONS.md#d-xxx-<slug>' reference -- any relative prefix
depth, with or without a literal 'docs/' segment -- to point at the real
per-decision file under docs/decisions/.
"""

from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path

# Real inbound references use three prefix shapes, all confirmed present in
# this repository's own docs/ tree: a literal "docs/" segment (optionally
# preceded by one or more "../", from files outside docs/, e.g.
# ".claude/skills/*/SKILL.md" three levels deep); a same-directory "./" with
# no "docs/" segment at all (from files already inside docs/ itself, e.g.
# docs/PYTHON_STANDARDS.md); and a bare "../" or "../../" with no "docs/"
# segment (from files inside a docs/ subdirectory, e.g. docs/sessions/ or
# docs/superpowers/specs/). Whatever prefix precedes "DECISIONS.md" is kept
# verbatim in the rewritten reference -- "decisions/" is always a sibling of
# wherever "DECISIONS.md" used to live, regardless of how that location was
# spelled. The slug's character class includes '_': GitHub's slugifier keeps
# underscores verbatim (e.g. D-102's own anchor mentions `pycc_testkit`), and
# a class missing '_' truncates the match mid-anchor, leaving a dangling
# fragment in the rewritten text instead of consuming the full reference.
REFERENCE_RE = re.compile(
    r"((?:\.\./)*(?:\./)?(?:docs/)?)DECISIONS\.md#(d-\d+)[a-z0-9_-]*"
)


def build_slug_map(decisions_dir):
    mapping = {}
    for path in decisions_dir.glob("D-*.md"):
        m = re.match(r"(D-\d+)-", path.name)
        if m:
            mapping[m.group(1).lower()] = path.name
    return mapping


def rewrite_text(text, slug_map):
    """Return (new_text, unresolved_ids). unresolved_ids lists any D-xxx
    referenced that has no entry in slug_map -- left untouched in new_text
    rather than silently mangled, so the caller can fail loudly."""
    unresolved = []

    def replace(m):
        prefix, id_lower = m.group(1), m.group(2)
        filename = slug_map.get(id_lower)
        if filename is None:
            unresolved.append(id_lower)
            return m.group(0)
        return f"{prefix}decisions/{filename}"

    new_text = REFERENCE_RE.sub(replace, text)
    return new_text, unresolved


def main(argv=None):
    parser = argparse.ArgumentParser()
    parser.add_argument("decisions_dir", type=Path)
    parser.add_argument("files", nargs="+", type=Path)
    args = parser.parse_args(argv)

    slug_map = build_slug_map(args.decisions_dir)
    any_unresolved = False
    for path in args.files:
        text = path.read_text(encoding="utf-8")
        new_text, unresolved = rewrite_text(text, slug_map)
        if unresolved:
            any_unresolved = True
            print(
                f"{path}: unresolved reference(s): {sorted(set(unresolved))}",
                file=sys.stderr,
            )
        if new_text != text:
            path.write_text(new_text, encoding="utf-8")
            print(f"rewrote {path}")
    return 1 if any_unresolved else 0


if __name__ == "__main__":
    sys.exit(main())
