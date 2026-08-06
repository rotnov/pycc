#!/usr/bin/env python3
"""Split docs/DECISIONS.md's long-form entries and index-only rows into one
file per decision, verifiable by round-trip before anything is written.
"""

from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path

ENTRY_HEADING_RE = re.compile(r"^## (D-\d+): (.+)$")
# Group 3 is deliberately just the bare status word: real index rows carry
# trailing detail after *any* status word, not only "superseded" (e.g. D-022:
# "accepted (privacy clause superseded by D-087)"; D-046: "superseded by
# D-048"). Anchoring the alternation right after "| " and capturing only the
# matched word -- with any trailing detail absorbed by the uncaptured
# `[^|]*` that follows -- normalizes to the first word directly at parse
# time, which is all the generated frontmatter needs.
INDEX_ROW_RE = re.compile(
    r"^\| (D-\d+) \| (.+) \| (accepted|proposed|superseded|rejected|deprecated)[^|]* \|$"
)
FENCE_RE = re.compile(r"^(`{3,}|~{3,})")


def split_entries(text):
    """Return [(id, title, body)] for every '## D-XXX: ...' heading found
    outside a fenced code block, in original file order. body includes the
    heading line itself through (not including) the next top-level
    heading."""
    lines = text.splitlines(keepends=True)
    in_fence = False
    entries = []
    current = None
    for line in lines:
        if FENCE_RE.match(line.strip()):
            in_fence = not in_fence
            if current is not None:
                current["body"].append(line)
            continue
        if not in_fence:
            m = ENTRY_HEADING_RE.match(line)
            if m:
                if current is not None:
                    entries.append(current)
                current = {"id": m.group(1), "title": m.group(2), "body": [line]}
                continue
        if current is not None:
            current["body"].append(line)
    if current is not None:
        entries.append(current)
    return [(e["id"], e["title"], "".join(e["body"])) for e in entries]


def parse_index_table(text):
    """Return [(id, decision_text, status)] for every index-table row, in
    table order."""
    rows = []
    for line in text.splitlines():
        m = INDEX_ROW_RE.match(line.strip())
        if m:
            rows.append((m.group(1), m.group(2).strip(), m.group(3).strip()))
    return rows


def slugify(text, max_len=50):
    slug = re.sub(r"[^a-z0-9]+", "-", text.lower()).strip("-")
    if len(slug) <= max_len:
        return slug
    truncated = slug[:max_len]
    if "-" in truncated:
        truncated = truncated.rsplit("-", 1)[0]
    return truncated


def yaml_quote(text):
    return '"' + text.replace("\\", "\\\\").replace('"', '\\"') + '"'


def render_entry_file(id_, title, status, body):
    frontmatter = f"---\nid: {id_}\ntitle: {yaml_quote(title)}\nstatus: {status}\n---\n\n"
    return frontmatter + body


def render_stub_file(id_, decision_text, status):
    frontmatter = f"---\nid: {id_}\ntitle: {yaml_quote(decision_text)}\nstatus: {status}\n---\n\n"
    body = (
        f"# {id_}\n\nIndex-only: no long-form entry recorded yet.\n\n{decision_text}\n"
    )
    return frontmatter + body


def build_files(text):
    """Return {filename: content} -- one file per long-form entry, plus a
    stub for every index row with no matching heading. A long-form entry's
    frontmatter `status` is sourced from its index-table row, never from its
    own prose '- Status: ...' line: the design is explicit that the index
    table is authoritative and the prose is only "occasionally more
    nuanced" -- real data shows they can disagree outright (D-046/D-047/
    D-106 all read "accepted" in the body's own Status line after a later
    decision superseded them, with only the index row updated)."""
    entries = split_entries(text)
    index_rows = parse_index_table(text)
    entry_ids = {id_ for id_, _, _ in entries}
    index_status = {id_: status for id_, _, status in index_rows}

    missing = sorted(id_ for id_, _, _ in entries if id_ not in index_status)
    if missing:
        raise ValueError(
            f"long-form entries with no matching index-table row: {missing} "
            "-- add their rows before calling build_files"
        )

    files = {}
    for id_, title, body in entries:
        status = index_status[id_]
        slug = slugify(title)
        files[f"{id_}-{slug}.md"] = render_entry_file(id_, title, status, body)

    for id_, decision_text, status in index_rows:
        if id_ in entry_ids:
            continue
        slug = slugify(decision_text)
        files[f"{id_}-{slug}.md"] = render_stub_file(id_, decision_text, status)

    return files


def verify_round_trip(text):
    """Two checks, both must pass: (1) concatenating every found entry's
    body, in original order, exactly reproduces the original file's tail
    from the first entry onward -- confirms no content was lost or
    duplicated while splitting. (2) every entry heading found has a
    matching index-table row -- confirms the index table (and this
    migration's own stub-generation, which reads it) is not silently
    missing something real. Returns (ok, message)."""
    entries = split_entries(text)
    if not entries:
        return False, "no entries found"

    reassembled = "".join(body for _, _, body in entries)
    first_body = entries[0][2]
    start = text.index(first_body)
    original_tail = text[start:]
    if reassembled != original_tail:
        return False, "MISMATCH: reassembled entries do not match the original tail"

    entry_ids = {id_ for id_, _, _ in entries}
    index_ids = {id_ for id_, _, _ in parse_index_table(text)}
    orphan_headings = entry_ids - index_ids
    if orphan_headings:
        return False, f"headings with no index-table row: {sorted(orphan_headings)}"

    return (
        True,
        f"OK: {len(entries)} entries round-tripped byte-for-byte, "
        f"{len(index_ids - entry_ids)} index-only rows",
    )


def main():
    parser = argparse.ArgumentParser()
    sub = parser.add_subparsers(dest="command", required=True)

    verify_p = sub.add_parser("verify")
    verify_p.add_argument("source", type=Path)

    write_p = sub.add_parser("write")
    write_p.add_argument("source", type=Path)
    write_p.add_argument("output_dir", type=Path)

    args = parser.parse_args()
    text = args.source.read_text(encoding="utf-8")

    if args.command == "verify":
        ok, message = verify_round_trip(text)
        print(message)
        return 0 if ok else 1

    ok, message = verify_round_trip(text)
    if not ok:
        print(f"refusing to write: {message}", file=sys.stderr)
        return 1
    files = build_files(text)
    args.output_dir.mkdir(parents=True, exist_ok=True)
    for filename, content in files.items():
        (args.output_dir / filename).write_text(content, encoding="utf-8")
    print(f"wrote {len(files)} files to {args.output_dir}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
