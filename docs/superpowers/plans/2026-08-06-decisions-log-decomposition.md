# docs/DECISIONS.md decomposition Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the single, ever-growing `docs/DECISIONS.md` with one file per
decision under `docs/decisions/`, plus a generated (never hand-edited) index, so a
missing index row becomes structurally impossible and reading/diffing one decision no
longer touches the other 124.

**Architecture:** Three small, independently-tested Python scripts (split-and-verify,
index-generate-and-check, reference-rewrite), then one migration pass that runs them
for real against the live repository and rewrites every inbound cross-reference.

**Tech Stack:** Python 3 standard library only (`re`, `argparse`, `difflib`, `pathlib`)
— matching this repository's existing `scripts/validate_agent_assets.py` /
`scripts/run_alpha_skill_evals.py` convention, no new dependency.

## Global Constraints

- Design source of truth: [`docs/superpowers/specs/2026-08-06-decisions-log-decomposition-design.md`](../specs/2026-08-06-decisions-log-decomposition-design.md).
- Precedent: [D-130](../../DECISIONS.md#d-130-decompose-the-session-handoff-log-into-per-session-files)
  (`SESSION_LOG.md` -> `docs/sessions/`) — mechanical split on real boundaries,
  byte-for-byte round-trip verification, sibling links rebased not left broken,
  historical *narrative* left untouched, no stub left at the retired path.
- **Link repair vs. narrative update, per D-130's own distinction:** every concrete
  `docs/DECISIONS.md#d-xxx-slug` anchor is rewritten everywhere it appears, including
  inside historical/dated documents — a dangling anchor is a mechanical defect
  regardless of the surrounding file's mutability. A *bare* `docs/DECISIONS.md`
  mention with no anchor is prose, not a link — it only gets updated in currently-live
  governance documents (the enumerated 15-file list in Task 4), never inside
  `docs/sessions/`, `docs/superpowers/plans/`, `docs/superpowers/specs/`, or
  `docs/AGENT_RETROSPECTIVE.md`, matching D-130's own explicit carve-out for those.
- Every normative documentation claim is enforceable by a test or CI check
  (`docs/SPEC.md`'s own doc-lifecycle rule) — `generate_decisions_index.py --check`
  is that enforcement for the new index.
- `docs/DECISIONS.md`'s own rule, unchanged: "Changing an accepted decision requires a
  new entry, not an edit." Moving an entry's storage location is not an edit to its
  content — every long-form entry's body must survive the split byte-for-byte.
- `python3 -B -m unittest discover -s scripts -p 'test_*.py'` already runs in CI
  (`.github/workflows/ci.yml`'s "Check agent policy" step) and auto-discovers any new
  `scripts/test_*.py` file — the three new test files below need no separate CI wiring
  for themselves; only the new index-freshness check itself needs a new CI step
  (Task 5).

---

### Task 1: `scripts/migrate_decisions_log.py` — split, extract, verify

**Files:**
- Create: `scripts/migrate_decisions_log.py`
- Create: `scripts/test_migrate_decisions_log.py`

**Interfaces:**
- Produces: `split_entries(text) -> list[tuple[str, str, str]]` (id, title, body),
  `parse_index_table(text) -> list[tuple[str, str, str]]` (id, decision_text, status —
  status is normalized to its first word, e.g. `"superseded"` from a cell reading
  `"superseded by D-048"`), `slugify(text, max_len=50) -> str`,
  `yaml_quote(text) -> str`, `render_entry_file(id_, title, status, body) -> str`,
  `render_stub_file(id_, decision_text, status) -> str`,
  `build_files(text) -> dict[str, str]` (filename -> content),
  `verify_round_trip(text) -> tuple[bool, str]`. Task 4 (the real migration) calls
  `build_files` and `verify_round_trip` via this script's own CLI (`verify` and
  `write` subcommands, defined in Step 9 below). Task 2 and Task 3 do not import this
  module — they operate on `build_files`'s *output directory*, not on
  `docs/DECISIONS.md` directly.

- [ ] **Step 1: Write the failing tests**

Create `scripts/test_migrate_decisions_log.py`:

`````python
from __future__ import annotations

import unittest

import migrate_decisions_log as migrate


FIXTURE = '''# Test Decisions

Format: one entry per irreversible-ish call.

| ID | Decision | Status |
|---|---|---|
| D-001 | First decision, index-only | proposed |
| D-002 | Second decision | accepted |
| D-003 | Third decision, has a nested code block | accepted |

## Template

```
## D-999: Should not be split -- this heading is inside a fence
- Status: proposed
```

Entries D-001 gets its long-form section once it graduates.

## D-002: Second decision

- Status: accepted
- Context: something happened.
- Decision: we did X.
- Alternatives: Y (rejected).
- Consequences: Z.

## D-003: Third decision, has a nested code block

- Status: accepted (closes #7)
- Context: needs an example.
- Decision: use this snippet:

```python
def f():
    return 1
```

- Alternatives: none.
- Consequences: none.
'''


class SplitEntriesTests(unittest.TestCase):
    def test_finds_only_real_headings_outside_fences(self):
        entries = migrate.split_entries(FIXTURE)
        ids = [id_ for id_, _, _ in entries]
        self.assertEqual(ids, ["D-002", "D-003"])

    def test_entry_body_includes_nested_fence_content(self):
        entries = migrate.split_entries(FIXTURE)
        _, _, body = entries[1]
        self.assertIn("def f():", body)
        self.assertIn("Alternatives: none.", body)


class ParseIndexTableTests(unittest.TestCase):
    def test_parses_all_rows_in_order(self):
        rows = migrate.parse_index_table(FIXTURE)
        self.assertEqual(
            rows,
            [
                ("D-001", "First decision, index-only", "proposed"),
                ("D-002", "Second decision", "accepted"),
                ("D-003", "Third decision, has a nested code block", "accepted"),
            ],
        )

    def test_captures_trailing_detail_after_any_status_word(self):
        # Real docs/DECISIONS.md rows carry parenthetical or narrowing detail
        # after every status word, not just "superseded" (e.g. D-022:
        # "accepted (privacy clause superseded by D-087)"; D-046: "superseded
        # by D-048"). The row must still parse, and the returned status must
        # normalize to the first word only.
        text = FIXTURE.replace(
            "| D-002 | Second decision | accepted |",
            "| D-002 | Second decision | accepted (narrowed by D-999) |",
        )
        rows = migrate.parse_index_table(text)
        self.assertIn(("D-002", "Second decision", "accepted"), rows)

    def test_normalizes_superseded_by_detail_to_first_word(self):
        text = FIXTURE.replace(
            "| D-002 | Second decision | accepted |",
            "| D-002 | Second decision | superseded by D-999 |",
        )
        rows = migrate.parse_index_table(text)
        self.assertIn(("D-002", "Second decision", "superseded"), rows)


class SlugifyTests(unittest.TestCase):
    def test_basic(self):
        self.assertEqual(migrate.slugify("Second decision"), "second-decision")

    def test_truncates_at_word_boundary(self):
        long_title = "a very long title that keeps going and going and going and going"
        slug = migrate.slugify(long_title, max_len=20)
        self.assertLessEqual(len(slug), 20)
        self.assertFalse(slug.endswith("-"))
        words = slug.split("-")
        title_words = long_title.lower().split()
        self.assertEqual(words, title_words[: len(words)])


class BuildFilesTests(unittest.TestCase):
    def test_builds_one_file_per_long_form_entry_and_one_stub(self):
        files = migrate.build_files(FIXTURE)
        self.assertEqual(
            set(files.keys()),
            {
                "D-001-first-decision-index-only.md",
                "D-002-second-decision.md",
                "D-003-third-decision-has-a-nested-code-block.md",
            },
        )

    def test_long_form_file_has_frontmatter_and_body(self):
        files = migrate.build_files(FIXTURE)
        content = files["D-002-second-decision.md"]
        self.assertTrue(content.startswith('---\nid: D-002\n'))
        self.assertIn('status: accepted', content)
        self.assertIn("## D-002: Second decision", content)
        self.assertIn("- Context: something happened.", content)

    def test_index_only_stub_has_the_fixed_note(self):
        files = migrate.build_files(FIXTURE)
        content = files["D-001-first-decision-index-only.md"]
        self.assertIn("status: proposed", content)
        self.assertIn("Index-only: no long-form entry recorded yet.", content)

    def test_status_comes_from_index_row_not_body_when_they_disagree(self):
        # Real docs/DECISIONS.md shape (D-046/D-047/D-106): the long-form
        # body's own "- Status: accepted ..." line is left narrating the
        # decision as it stood when written, while the index table row was
        # updated to "superseded by D-xxx" once a later decision superseded
        # it. The index table is the authoritative source for the generated
        # frontmatter -- the design doc is explicit that status is "sourced
        # from the current index table row ... not re-derived from the
        # prose".
        drifted = FIXTURE.replace(
            "| D-002 | Second decision | accepted |",
            "| D-002 | Second decision | superseded by D-999 |",
        )
        files = migrate.build_files(drifted)
        content = files["D-002-second-decision.md"]
        self.assertIn("status: superseded", content)
        self.assertNotIn("status: accepted", content)

    def test_raises_when_a_long_form_entry_has_no_index_row(self):
        # build_files is a lower-level building block than the CLI's `write`
        # subcommand (which always calls verify_round_trip first and refuses
        # to proceed on the same condition) -- calling it directly on text
        # with an orphan heading must still fail loudly, naming the missing
        # ID, rather than raising an opaque KeyError or silently defaulting.
        orphan = FIXTURE.replace(
            "## D-003: Third decision, has a nested code block",
            "## D-999: Orphan heading with no index row\n\n"
            "- Status: accepted\n\n"
            "## D-003: Third decision, has a nested code block",
        )
        with self.assertRaises(ValueError) as cm:
            migrate.build_files(orphan)
        self.assertIn("D-999", str(cm.exception))


class VerifyRoundTripTests(unittest.TestCase):
    def test_ok_on_well_formed_fixture(self):
        ok, message = migrate.verify_round_trip(FIXTURE)
        self.assertTrue(ok, message)
        self.assertIn("2 entries", message)

    def test_fails_when_a_heading_has_no_index_row(self):
        orphan = FIXTURE.replace(
            "## D-003: Third decision, has a nested code block",
            "## D-999: Orphan heading with no index row\n\n## D-003: Third decision, has a nested code block",
        )
        ok, message = migrate.verify_round_trip(orphan)
        self.assertFalse(ok)
        self.assertIn("D-999", message)

    def test_fails_on_empty_input(self):
        ok, message = migrate.verify_round_trip("# Nothing here\n")
        self.assertFalse(ok)
        self.assertEqual(message, "no entries found")


if __name__ == "__main__":
    unittest.main()
`````

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cd scripts && python3 -m unittest test_migrate_decisions_log -v`
Expected: `ModuleNotFoundError: No module named 'migrate_decisions_log'`

- [ ] **Step 3: Write `scripts/migrate_decisions_log.py`**

```python
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
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cd scripts && python3 -m unittest test_migrate_decisions_log -v`
Expected: all tests `ok`.

- [ ] **Step 5: Commit**

```bash
git add scripts/migrate_decisions_log.py scripts/test_migrate_decisions_log.py
git commit -m "Add migrate_decisions_log.py: split, extract, and verify DECISIONS.md entries"
```

---

### Task 2: `scripts/generate_decisions_index.py` — generated (never hand-edited) index

**Files:**
- Create: `scripts/generate_decisions_index.py`
- Create: `scripts/test_generate_decisions_index.py`

**Interfaces:**
- Consumes: the frontmatter shape Task 1's `render_entry_file`/`render_stub_file`
  produce (`---\nid: D-XXX\ntitle: "..."\nstatus: ...\n---\n\n`).
- Produces: `read_frontmatter(path) -> tuple[str, str, str]`,
  `generate_index(decisions_dir) -> str`. Task 4 calls this script's CLI
  (`generate_decisions_index.py <dir> <readme> [--check]`) directly; Task 5 wires the
  `--check` invocation into CI.

- [ ] **Step 1: Write the failing tests**

Create `scripts/test_generate_decisions_index.py`:

```python
from __future__ import annotations

import tempfile
import unittest
from pathlib import Path

import generate_decisions_index as gen


def write_decision(directory, filename, id_, title, status):
    content = f'---\nid: {id_}\ntitle: "{title}"\nstatus: {status}\n---\n\n# {id_}: {title}\n'
    (directory / filename).write_text(content, encoding="utf-8")


class ReadFrontmatterTests(unittest.TestCase):
    def test_reads_id_title_status(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "D-002-second.md"
            write_decision(Path(directory), "D-002-second.md", "D-002", "Second decision", "accepted")
            self.assertEqual(gen.read_frontmatter(path), ("D-002", "Second decision", "accepted"))

    def test_unescapes_quotes_in_title(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "D-003.md"
            content = '---\nid: D-003\ntitle: "Uses \\"quotes\\" and a backslash \\\\"\nstatus: accepted\n---\n\nbody\n'
            path.write_text(content, encoding="utf-8")
            _, title, _ = gen.read_frontmatter(path)
            self.assertEqual(title, 'Uses "quotes" and a backslash \\')

    def test_missing_frontmatter_raises(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "D-004.md"
            path.write_text("# no frontmatter here\n", encoding="utf-8")
            with self.assertRaises(ValueError):
                gen.read_frontmatter(path)


class GenerateIndexTests(unittest.TestCase):
    def test_sorts_numerically_not_lexically(self):
        with tempfile.TemporaryDirectory() as directory:
            d = Path(directory)
            write_decision(d, "D-002-second.md", "D-002", "Second", "accepted")
            write_decision(d, "D-010-tenth.md", "D-010", "Tenth", "accepted")
            write_decision(d, "D-001-first.md", "D-001", "First", "proposed")
            table = gen.generate_index(d)
            first_pos = table.index("D-001")
            second_pos = table.index("D-002")
            tenth_pos = table.index("D-010")
            self.assertLess(first_pos, second_pos)
            self.assertLess(second_pos, tenth_pos)

    def test_links_point_at_the_real_filename(self):
        with tempfile.TemporaryDirectory() as directory:
            d = Path(directory)
            write_decision(d, "D-002-second-decision.md", "D-002", "Second decision", "accepted")
            table = gen.generate_index(d)
            self.assertIn("[D-002](./D-002-second-decision.md)", table)

    def test_includes_status_column(self):
        with tempfile.TemporaryDirectory() as directory:
            d = Path(directory)
            write_decision(d, "D-001-first.md", "D-001", "First", "proposed")
            table = gen.generate_index(d)
            self.assertIn("| proposed |", table)


class MainCheckModeTests(unittest.TestCase):
    def test_check_passes_when_readme_matches_generated(self):
        with tempfile.TemporaryDirectory() as directory:
            d = Path(directory)
            write_decision(d, "D-001-first.md", "D-001", "First", "accepted")
            readme = d / "README.md"
            readme.write_text(gen.generate_index(d), encoding="utf-8")
            exit_code = gen.main([str(d), str(readme), "--check"])
            self.assertEqual(exit_code, 0)

    def test_check_fails_when_readme_is_stale(self):
        with tempfile.TemporaryDirectory() as directory:
            d = Path(directory)
            write_decision(d, "D-001-first.md", "D-001", "First", "accepted")
            readme = d / "README.md"
            readme.write_text("stale content\n", encoding="utf-8")
            exit_code = gen.main([str(d), str(readme), "--check"])
            self.assertEqual(exit_code, 1)

    def test_writes_readme_without_check(self):
        with tempfile.TemporaryDirectory() as directory:
            d = Path(directory)
            write_decision(d, "D-001-first.md", "D-001", "First", "accepted")
            readme = d / "README.md"
            exit_code = gen.main([str(d), str(readme)])
            self.assertEqual(exit_code, 0)
            self.assertIn("D-001", readme.read_text(encoding="utf-8"))


if __name__ == "__main__":
    unittest.main()
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cd scripts && python3 -m unittest test_generate_decisions_index -v`
Expected: `ModuleNotFoundError: No module named 'generate_decisions_index'`

- [ ] **Step 3: Write `scripts/generate_decisions_index.py`**

```python
#!/usr/bin/env python3
"""Generate docs/decisions/README.md's index table from every decision
file's own frontmatter. The index is a build artifact -- never hand-edit
it; run this script after adding or changing a decision file, and its
--check mode verifies the committed file is current.
"""

from __future__ import annotations

import argparse
import difflib
import re
import sys
from pathlib import Path

FRONTMATTER_RE = re.compile(
    r'\A---\nid: (D-\d+)\ntitle: "((?:[^"\\]|\\.)*)"\nstatus: (\S+)\n---\n'
)

PREAMBLE = """# pycc Design Decisions (ADR log)

Format: one file per irreversible-ish call, under this directory. Statuses:
`proposed` -> `accepted` -> (`superseded by D-xxx`). Changing an accepted
decision requires a new entry, not an edit -- see `TEMPLATE.md`.

This table is generated by `scripts/generate_decisions_index.py` from every
`D-*.md` file's own frontmatter. Never hand-edit it -- run the generator
after adding or changing a decision file; `--check` verifies it is
current."""


def unescape_yaml(value):
    return value.replace('\\"', '"').replace("\\\\", "\\")


def read_frontmatter(path):
    text = path.read_text(encoding="utf-8")
    m = FRONTMATTER_RE.match(text)
    if not m:
        raise ValueError(f"{path}: missing or malformed frontmatter")
    id_, title_escaped, status = m.groups()
    return id_, unescape_yaml(title_escaped), status


def generate_index(decisions_dir):
    entries = []
    for path in sorted(decisions_dir.glob("D-*.md")):
        id_, title, status = read_frontmatter(path)
        entries.append((id_, title, status, path.name))
    entries.sort(key=lambda e: int(e[0].split("-")[1]))

    lines = [PREAMBLE, "", "| ID | Decision | Status |", "|---|---|---|"]
    for id_, title, status, filename in entries:
        lines.append(f"| [{id_}](./{filename}) | {title} | {status} |")
    return "\n".join(lines) + "\n"


def main(argv=None):
    parser = argparse.ArgumentParser()
    parser.add_argument("decisions_dir", type=Path)
    parser.add_argument("readme_path", type=Path)
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args(argv)

    generated = generate_index(args.decisions_dir)

    if args.check:
        current = (
            args.readme_path.read_text(encoding="utf-8")
            if args.readme_path.exists()
            else ""
        )
        if generated != current:
            diff = "".join(
                difflib.unified_diff(
                    current.splitlines(keepends=True),
                    generated.splitlines(keepends=True),
                    fromfile=str(args.readme_path),
                    tofile="generated",
                )
            )
            print(
                f"{args.readme_path} is out of date with its source files:\n{diff}",
                file=sys.stderr,
            )
            return 1
        print(f"{args.readme_path} is up to date.")
        return 0

    args.readme_path.write_text(generated, encoding="utf-8")
    print(f"wrote {args.readme_path} ({generated.count(chr(10))} lines)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cd scripts && python3 -m unittest test_generate_decisions_index -v`
Expected: all tests `ok`.

- [ ] **Step 5: Commit**

```bash
git add scripts/generate_decisions_index.py scripts/test_generate_decisions_index.py
git commit -m "Add generate_decisions_index.py: generated, checkable docs/decisions/README.md"
```

---

### Task 3: `scripts/rewrite_decisions_references.py` — repair inbound anchor links

**Files:**
- Create: `scripts/rewrite_decisions_references.py`
- Create: `scripts/test_rewrite_decisions_references.py`

**Interfaces:**
- Consumes: the `D-XXX-<slug>.md` filename shape Task 1's `build_files` produces.
- Produces: `build_slug_map(decisions_dir) -> dict[str, str]`,
  `rewrite_text(text, slug_map) -> tuple[str, list[str]]`. Task 4 calls this script's
  CLI directly against the 12 files with real anchor references.

- [ ] **Step 1: Write the failing tests**

Create `scripts/test_rewrite_decisions_references.py`:

```python
from __future__ import annotations

import tempfile
import unittest
from pathlib import Path

import rewrite_decisions_references as rewrite


class BuildSlugMapTests(unittest.TestCase):
    def test_maps_lowercase_id_to_filename(self):
        with tempfile.TemporaryDirectory() as directory:
            d = Path(directory)
            (d / "D-021-agent-task-preflight.md").write_text("x", encoding="utf-8")
            (d / "D-142-dispatched-agent.md").write_text("x", encoding="utf-8")
            mapping = rewrite.build_slug_map(d)
            self.assertEqual(
                mapping,
                {
                    "d-021": "D-021-agent-task-preflight.md",
                    "d-142": "D-142-dispatched-agent.md",
                },
            )


class RewriteTextTests(unittest.TestCase):
    def setUp(self):
        self.slug_map = {"d-021": "D-021-agent-task-preflight.md"}

    def test_rewrites_plain_reference(self):
        text = "See [D-021](docs/DECISIONS.md#d-021-agent-task-preflight-and-documentation-refresh) for details."
        new_text, unresolved = rewrite.rewrite_text(text, self.slug_map)
        self.assertEqual(unresolved, [])
        self.assertEqual(
            new_text,
            "See [D-021](docs/decisions/D-021-agent-task-preflight.md) for details.",
        )

    def test_rewrites_relative_reference_preserving_prefix_depth(self):
        text = "[D-021](../../../docs/DECISIONS.md#d-021-agent-task-preflight-and-documentation-refresh)"
        new_text, unresolved = rewrite.rewrite_text(text, self.slug_map)
        self.assertEqual(unresolved, [])
        self.assertEqual(
            new_text, "[D-021](../../../docs/decisions/D-021-agent-task-preflight.md)"
        )

    def test_rewrites_same_directory_reference_with_no_docs_prefix(self):
        # Real shape from docs/PYTHON_STANDARDS.md and docs/DELIVERY_PLAN.md,
        # both already inside docs/ themselves: a same-directory reference
        # has no literal "docs/" segment at all, just "./DECISIONS.md".
        text = "[D-021](./DECISIONS.md#d-021-agent-task-preflight-and-documentation-refresh)"
        new_text, unresolved = rewrite.rewrite_text(text, self.slug_map)
        self.assertEqual(unresolved, [])
        self.assertEqual(new_text, "[D-021](./decisions/D-021-agent-task-preflight.md)")

    def test_rewrites_one_level_up_reference_with_no_docs_prefix(self):
        # Real shape from docs/sessions/README.md and other files one level
        # below docs/: "../DECISIONS.md" with no "docs/" segment, since the
        # referring file is already inside the docs/ tree.
        text = "[D-021](../DECISIONS.md#d-021-agent-task-preflight-and-documentation-refresh)"
        new_text, unresolved = rewrite.rewrite_text(text, self.slug_map)
        self.assertEqual(unresolved, [])
        self.assertEqual(new_text, "[D-021](../decisions/D-021-agent-task-preflight.md)")

    def test_rewrites_two_levels_up_reference_with_no_docs_prefix(self):
        # Real shape from docs/superpowers/specs/*.md: "../../DECISIONS.md"
        # with no "docs/" segment.
        text = "[D-021](../../DECISIONS.md#d-021-agent-task-preflight-and-documentation-refresh)"
        new_text, unresolved = rewrite.rewrite_text(text, self.slug_map)
        self.assertEqual(unresolved, [])
        self.assertEqual(new_text, "[D-021](../../decisions/D-021-agent-task-preflight.md)")

    def test_rewrites_reference_with_underscore_in_slug(self):
        # Real shape from D-102's own anchor (title mentions `pycc_testkit`):
        # GitHub's slugifier keeps underscores verbatim -- the character
        # class must include '_' or the match truncates mid-anchor, leaving
        # a dangling "_testkit-crate)" fragment in the rewritten text.
        slug_map = {"d-102": "D-102-extend-tests-conformance.md"}
        text = "[D-102](./DECISIONS.md#d-102-extend-testsconformancers-for-pr-9s-9-new-pep-fixtures-no-pycc_testkit-crate)"
        new_text, unresolved = rewrite.rewrite_text(text, slug_map)
        self.assertEqual(unresolved, [])
        self.assertEqual(new_text, "[D-102](./decisions/D-102-extend-tests-conformance.md)")

    def test_rewrites_multiple_occurrences(self):
        text = (
            "First: docs/DECISIONS.md#d-021-agent-task-preflight-and-documentation-refresh\n"
            "Second: docs/DECISIONS.md#d-021-agent-task-preflight-and-documentation-refresh\n"
        )
        new_text, unresolved = rewrite.rewrite_text(text, self.slug_map)
        self.assertEqual(unresolved, [])
        self.assertEqual(new_text.count("docs/decisions/D-021-agent-task-preflight.md"), 2)

    def test_leaves_unresolved_id_untouched_and_reports_it(self):
        text = "docs/DECISIONS.md#d-999-does-not-exist"
        new_text, unresolved = rewrite.rewrite_text(text, self.slug_map)
        self.assertEqual(new_text, text)
        self.assertEqual(unresolved, ["d-999"])

    def test_text_with_no_references_is_unchanged(self):
        text = "Nothing to rewrite here.\n"
        new_text, unresolved = rewrite.rewrite_text(text, self.slug_map)
        self.assertEqual(new_text, text)
        self.assertEqual(unresolved, [])


if __name__ == "__main__":
    unittest.main()
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cd scripts && python3 -m unittest test_rewrite_decisions_references -v`
Expected: `ModuleNotFoundError: No module named 'rewrite_decisions_references'`

- [ ] **Step 3: Write `scripts/rewrite_decisions_references.py`**

```python
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
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cd scripts && python3 -m unittest test_rewrite_decisions_references -v`
Expected: all tests `ok`.

- [ ] **Step 5: Commit**

```bash
git add scripts/rewrite_decisions_references.py scripts/test_rewrite_decisions_references.py
git commit -m "Add rewrite_decisions_references.py: repair inbound DECISIONS.md anchor links"
```

---

### Task 4: Run the real migration

**Files:**
- Modify: `docs/DECISIONS.md` (one-time prerequisite fix, then deleted)
- Create: `docs/decisions/D-*.md` (~139 files, generated)
- Create: `docs/decisions/README.md` (generated)
- Create: `docs/decisions/TEMPLATE.md`
- Create: `docs/decisions/D-1NN-decompose-the-decisions-log-into-per-decision-files.md`
  (`NN` resolved at execution time — see Step 8)
- Modify: the 12 files with anchor references (Step 4 list below)
- Modify: the 15 files with bare mentions (Step 5 list below)
- Modify: `docs/SPEC.md` (the giant-parenthetical fix, beyond the mechanical
  substitution)
- Delete: `docs/DECISIONS.md`

**Interfaces:** none new — this task only *runs* the CLIs Tasks 1-3 built.

- [ ] **Step 1: Fix 5 pre-existing missing index rows (a prerequisite the
      migration's own verification discovered)**

`migrate_decisions_log.py verify` will refuse to proceed while any long-form heading
lacks a matching index-table row. The current file already has exactly this defect —
independently of this migration — for D-136, D-137, D-138, D-139, and D-148 (the last
one is the exact class of bug `rotnov/pycc#363` found live). Add their rows now, before
splitting, so the new structure does not inherit the drift it exists to prevent.

Insert after the `D-135` row (between the current `D-135` and `D-140` rows):

```
| D-136 | `pycc_std` is a plain data crate; `math`/`sys` symbols are hand-recognized inside `pycc_types::infer_expr_in`, reusing the `print`/`len` pattern | accepted |
| D-137 | stdlib imports bind module-qualified names via a per-module HIR import table; every other import form stays a clean `C0001` | accepted |
| D-138 | the PEP-594 conformance fixture imports `cgi` and asserts its unchanged `C0001` rejection, contrasted with a passing `math`/`sys` fixture | accepted |
| D-139 | the container/generics differential corpus is one hand-authored, multi-feature fixture file, oracle-diffed like every other conformance fixture | accepted |
```

Insert after the `D-146` row (and after `D-147`'s row too, if that indicative entry has
merged by the time this task runs — check `docs/DECISIONS.md`'s live tip first):

```
| D-148 | Reuse `L0001` for context-invalid `break`/`continue`/`async for`, not a new code (#141) | accepted |
```

Verify:

```bash
python3 scripts/migrate_decisions_log.py verify docs/DECISIONS.md
```

Expected: `OK: <N> entries round-tripped byte-for-byte, <K> index-only rows` — no
`headings with no index-table row` failure.

Commit this prerequisite fix on its own:

```bash
git add docs/DECISIONS.md
git commit -m "Add 5 pre-existing missing index-table rows (D-136..D-139, D-148) before migration"
```

- [ ] **Step 2: Run the split, generate the index, add the static template**

```bash
python3 scripts/migrate_decisions_log.py write docs/DECISIONS.md docs/decisions
python3 scripts/generate_decisions_index.py docs/decisions docs/decisions/README.md
```

Create `docs/decisions/TEMPLATE.md`:

```markdown
# Decision entry template

```
## D-0XX: Title
- Status: proposed
- Context: what forces the choice
- Decision: what we do
- Alternatives: what we rejected and why
- Consequences: what gets easier / harder / irreversible
```

Entries graduate from `proposed` to `accepted` (first PR that depends on the decision
must include the long-form section).
```

- [ ] **Step 3: Confirm the file count and spot-check a few entries**

```bash
ls docs/decisions/D-*.md | wc -l
head -6 docs/decisions/D-014-100-test-coverage-requirement.md
grep -c "^| \[D-" docs/decisions/README.md
```

Expected: both counts equal **143** (124 long-form entries + 19 index-only stub rows,
after Step 1's 5-row addition — precisely measured by running
`migrate_decisions_log.py`'s own `split_entries`/`parse_index_table` against the
5-row-patched file before this step; re-verify against the live tree at execution time
rather than trusting this number blindly, the same way Step 1's own 5-row list was
re-checked against the live tip). The `D-*.md` glob deliberately excludes
`README.md`/`TEMPLATE.md` (added later in this step) so the two counts are actually
comparable — `ls docs/decisions/*.md` would NOT equal `README.md`'s row count, since the
former also counts files the latter's generator never lists a row for. Both counts
become 144 after Step 8 adds this migration's own entry. `D-014`'s file starts with its
frontmatter block followed by its unchanged `## D-014: 100% test coverage requirement`
heading.

- [ ] **Step 4: Rewrite anchor references (link repair — every file, including
      historical ones)**

The file list below was accurate when this plan was written, but this repository has a
documented concurrent actor and this session's own task branches have drifted from it
before — **re-derive the live list immediately before running this step**, rather than
trusting the hardcoded one blindly (the same discipline Step 1 already applies to the
5 missing index rows):

```bash
git ls-files '*.md' | xargs grep -l "DECISIONS\.md#d-" 2>/dev/null \
  | grep -v "^docs/decisions/" \
  | grep -v "^docs/superpowers/plans/2026-08-06-decisions-log-decomposition\.md" \
  | grep -v "^docs/superpowers/specs/2026-08-06-decisions-log-decomposition-design\.md"
```

Compare this live output against the list this plan was written against — `AGENTS.md`,
`docs/superpowers/plans/2026-08-05-ultra-review-skill.md`,
`.claude/skills/next-milestone/SKILL.md`,
`docs/superpowers/specs/2026-07-28-v0-2-collections-generics-design.md`,
`docs/sessions/README.md`, `docs/PYTHON_STANDARDS.md`, `docs/DELIVERY_PLAN.md`,
`.claude/skills/ultra-review/SKILL.md`, `.claude/skills/issue-select/SKILL.md`,
`.claude/skills/issue-implement/SKILL.md`,
`docs/sessions/2026-07-31-01-v0-2-pr-10-confirmed-self-inflicted-frontend-perf-gate-regression-d-10.md`,
`docs/ROADMAP.md` — as of this plan's last verification, two of those twelve
(`docs/superpowers/plans/2026-08-05-ultra-review-skill.md` and
`.claude/skills/ultra-review/SKILL.md`) do not exist on this task branch yet; they live
on unmerged PR #357. Running the rewriter against a file path that does not exist
raises `FileNotFoundError` and aborts the whole batch, so pass the **live-derived** list,
not the hardcoded one, to the command below:

```bash
python3 scripts/rewrite_decisions_references.py docs/decisions $(git ls-files '*.md' | xargs grep -l "DECISIONS\.md#d-" 2>/dev/null | grep -v "^docs/decisions/" | grep -v "^docs/superpowers/plans/2026-08-06-decisions-log-decomposition\.md" | grep -v "^docs/superpowers/specs/2026-08-06-decisions-log-decomposition-design\.md")
```

Expected exit 0, with one `rewrote <path>` line per file that actually changed, and no
`unresolved reference(s)` line — if one appears, the referenced `D-xxx` genuinely has
no file in `docs/decisions/` (stop and investigate before proceeding; it means either
Step 1-3 missed an entry or a reference points at a number that never existed). Record
the exact live-derived file list actually used, for the commit message / report.

- [ ] **Step 5: Update bare mentions in live governance documents only**

Per the Global Constraints' link-repair-vs-narrative distinction: these 15 files get a
mechanical replacement of every bare (non-anchored) `DECISIONS.md` mention;
`docs/sessions/`, `docs/superpowers/plans/`, `docs/superpowers/specs/`, and
`docs/AGENT_RETROSPECTIVE.md` are deliberately excluded, matching D-130's own
precedent for historical narrative.

A plain substring replacement of the literal text `"docs/DECISIONS.md"` is not enough:
real bare mentions in this file set use three shapes, only one of which contains that
literal substring — confirmed by grepping this repository's own live files, not assumed
from the design doc's original (incomplete) enumeration. `docs/ARCHITECTURE.md:170` and
`docs/DELIVERY_PLAN.md:138` both read `[DECISIONS.md](./DECISIONS.md)` (same-directory,
no `docs/` segment, since both files already live inside `docs/`); `docs/DELIVERY_PLAN.md:61`
and `docs/SPEC.md:48` mention the bare word `DECISIONS.md` in running prose with no path
prefix at all. A substitution that only matches `"docs/DECISIONS.md"` verbatim would
silently skip all four — leaving `docs/ARCHITECTURE.md` and `docs/DELIVERY_PLAN.md` with
genuinely broken links once Step 7 deletes the file, and two prose sentences describing
a filename that no longer exists.

```bash
python3 - <<'PYEOF'
import re
from pathlib import Path

# Handles all three real shapes: a literal "docs/" segment (optionally preceded
# by "../"), a same-directory "./" with no "docs/" segment, and a bare mention
# with no path prefix at all -- see Step 4's comment in
# scripts/rewrite_decisions_references.py for the same three-shape rationale.
# (?!#) skips any residual anchored mention -- Step 4 already rewrote every
# "DECISIONS.md#d-xxx" occurrence in files common to both lists, so nothing
# anchored should remain by the time this step runs; the guard is defense in
# depth, not load-bearing.
BARE_MENTION_RE = re.compile(r"((?:\.\./)*(?:\./)?(?:docs/)?)DECISIONS\.md\b(?!#)")

def replace(m):
    prefix = m.group(1)
    return "docs/decisions/README.md" if prefix == "" else f"{prefix}decisions/README.md"

files = [
    ".agents/skills/process-error-postmortem/SKILL.md",
    ".claude/skills/ci-temporary-bypass/SKILL.md",
    ".claude/skills/grill-with-docs/SKILL.md",
    ".claude/skills/issue-implement/SKILL.md",
    ".claude/skills/issue-select/SKILL.md",
    ".claude/skills/issue-to-plan/SKILL.md",
    ".claude/skills/process-error-postmortem/SKILL.md",
    "AGENTS.md",
    "docs/ARCHITECTURE.md",
    "docs/DELIVERY_PLAN.md",
    "docs/REPOSITORY_GOVERNANCE.md",
    "docs/ROADMAP.md",
    "docs/SPEC.md",
    "docs/STDLIB_PLAN.md",
    "docs/TESTING.md",
]
for name in files:
    path = Path(name)
    text = path.read_text(encoding="utf-8")
    new_text = BARE_MENTION_RE.sub(replace, text)
    if new_text != text:
        path.write_text(new_text, encoding="utf-8")
        print(f"updated {name}")
PYEOF
```

Expect at least `docs/ARCHITECTURE.md`, `docs/DELIVERY_PLAN.md`, and `docs/SPEC.md`
among the `updated` lines (all three carry a shape the original substring-only version
would have missed). Read the diff for each changed file afterward (`git diff -- <path>`)
and confirm every substitution still reads grammatically correct in context — this is a
uniform mechanical rule, not a guarantee of perfect prose, so a rare awkward phrasing
gets a manual touch-up here rather than being left in.

- [ ] **Step 6: Fix `docs/SPEC.md`'s row beyond the mechanical substitution**

Step 5 already repointed the row's own link target (to `[docs/decisions/README.md]
(./decisions/README.md)`, per Step 5's corrected regex, replacing both the link text
and the href since the mention appears in both). Now replace the giant enumerated
parenthetical itself — the direct "noise over info" artifact this whole migration
exists to fix. Locate the row by its stable **content**, not by its link text (which
Step 5 just changed and which depends on execution order) — it is the only row in
`docs/SPEC.md`'s table whose middle cell starts with `ADR log D-001…`:

```bash
grep -n "ADR log D-001" docs/SPEC.md
```

Replace that entire row (from the line's leading `|` through its trailing `|`) with:

```
| [decisions/](./decisions/README.md) | ADR log, one file per decision — see the generated index for the full list | irreversible calls |
```

- [ ] **Step 7: Delete the retired file**

```bash
git rm docs/DECISIONS.md
```

No stub — matching D-130's own precedent for `SESSION_LOG.md`.

- [ ] **Step 8: Add this migration's own decision entry**

Resolve the next free `D-` number against the live tip immediately before this step
(`grep -n "^## D-[0-9]" docs/decisions/*.md` no longer applies once the source is
per-file — instead: `ls docs/decisions/D-*.md | sed -E 's#.*/D-([0-9]+)-.*#\1#' | sort -n | tail -1`
gives the highest ID currently in the new directory; the next free one is one past
that, unless a concurrent PR has claimed it — re-check immediately before committing,
per this project's standing convention for number collisions). Create
`docs/decisions/D-1NN-decompose-the-decisions-log-into-per-decision-files.md` in the
new frontmatter format:

```markdown
---
id: D-1NN
title: "Decompose docs/DECISIONS.md into docs/decisions/, one file per decision, generated index"
status: accepted
---

# D-1NN: Decompose docs/DECISIONS.md into docs/decisions/, one file per decision, generated index

- Status: accepted
- Context: `docs/DECISIONS.md` had grown to 1498 lines and 139 index rows, becoming the
  exact kind of shared, ever-growing, must-read-then-append tail
  [D-130](./D-130-decompose-the-session-handoff-log-into-per-session-files.md) already
  moved `docs/SESSION_LOG.md` away from. Unlike that file, this one is read by ID/topic
  lookup — 40 measured inbound `#d-xxx` anchor references across 12 files — not
  chronologically, so D-130's own no-index conclusion did not transfer directly. The
  hand-maintained index had also already silently drifted before this migration even
  started: D-136, D-137, D-138, D-139, and D-148 each had a long-form entry with no
  matching index-table row, discovered only because this migration's own round-trip
  verification refused to proceed until they were fixed.
- Decision: one file per decision under `docs/decisions/D-XXX-<slug>.md`, split
  mechanically from the original file's real entry boundaries and verified
  byte-for-byte before anything was deleted (`scripts/migrate_decisions_log.py`).
  `docs/decisions/README.md`'s index table is a generated artifact
  (`scripts/generate_decisions_index.py`), never hand-edited — a missing index row for
  a new decision becomes structurally impossible rather than merely less likely, the
  direct fix for the exact class of gap this migration's own preflight found five
  instances of, and that `rotnov/pycc#363` found a sixth live instance of via
  `ultra-review`'s own first real dogfood run. Every inbound anchor reference across 12
  files (including historical/dated ones, since a dangling anchor is a mechanical
  defect regardless of the surrounding file's mutability) was rewritten to the new
  per-file target (`scripts/rewrite_decisions_references.py`); bare, non-anchored
  mentions were updated only in 15 currently-live governance documents, leaving
  `docs/sessions/`, `docs/superpowers/plans/`, `docs/superpowers/specs/`, and
  `docs/AGENT_RETROSPECTIVE.md` untouched, matching D-130's own explicit precedent that
  historical narrative is not rewritten. `docs/SPEC.md`'s own giant parenthetical
  enumerating every decision's topic — the most visible symptom of the problem this
  migration solves — is replaced with a short pointer to the generated index.
  `docs/DECISIONS.md` is deleted outright, no stub, matching D-130's treatment of
  `SESSION_LOG.md`.
- Alternatives: keep D-130's own no-index shape (`ls`-only discovery) — rejected, this
  file's real read pattern is ID/topic lookup, not chronological, and dropping the
  index would regress an actively-used workflow. Range-grouped files
  (`D001-050.md`, ...) — rejected, arbitrary boundaries tracking nothing real about the
  decisions, and each range bucket still grows without bound. Topic-grouped files —
  rejected, most decisions here span more than one area, forcing a subjective grouping
  call per entry. A hand-maintained index — rejected, reintroduces the exact
  shared-mutable-tail conflict this migration exists to remove.
- Consequences: `scripts/generate_decisions_index.py --check` is wired into CI as a
  new required-check step, so a decision file added without regenerating the index
  fails the build rather than silently drifting. Every skill, `AGENTS.md`, and spec
  document's own reference to a specific decision now resolves to that decision's own
  file directly, no anchor needed. A future decision is added by creating
  `docs/decisions/D-1NN-<slug>.md` directly in this format and regenerating the index —
  never by re-creating a single growing file. `.claude/skills/ultra-review/SKILL.md` and
  `docs/superpowers/plans/2026-08-05-ultra-review-skill.md` were not in this migration's
  own file list because they live only on unmerged PR #357 (already `CONFLICTING` against
  `main` for an unrelated reason) — that PR carries 5 of its own `docs/DECISIONS.md#d-xxx`
  anchor references that become dead links the moment this migration merges. PR #357's
  own rebase needs to apply the same anchor rewrite to its changed files before it can
  merge cleanly after this one.
```

- [ ] **Step 9: Regenerate the index one final time and verify it is current**

```bash
python3 scripts/generate_decisions_index.py docs/decisions docs/decisions/README.md
python3 scripts/generate_decisions_index.py docs/decisions docs/decisions/README.md --check
```

Expected: the second command exits 0 (`docs/decisions/README.md is up to date.`) —
confirms Step 8's newly-added file is reflected.

- [ ] **Step 10: Final verification sweep**

```bash
git status --short
grep -rln "DECISIONS\.md" --include="*.md" . 2>/dev/null \
  | grep -v "^\./docs/decisions/" \
  | grep -v "^\./docs/sessions/" \
  | grep -v "^\./docs/superpowers/plans/" \
  | grep -v "^\./docs/superpowers/specs/" \
  | grep -v "^\./docs/AGENT_RETROSPECTIVE\.md"
python3 -m unittest discover -s scripts -p "test_*.py" 2>&1 | tail -20
python3 scripts/validate_agent_assets.py
python3 scripts/validate_agent_policies.py
```

The pattern is deliberately the bare word `DECISIONS\.md`, not `docs/DECISIONS\.md` —
Step 5's own fix exists precisely because a `docs/`-anchored pattern misses the
`./DECISIONS.md` and bare-word forms that turned out to be real; a verification sweep
using the same too-narrow pattern would report false-clean even if Step 5 regressed.
`docs/decisions/` is excluded for a different, expected reason: several migrated entry
bodies legitimately self-reference `` `docs/DECISIONS.md` `` in their own historical
prose (e.g. D-066's and D-130's own entries describe the old filename as part of what
was true when they were written) — carried over byte-for-byte by Task 1's migration and
never rewritten, per this project's own "a new decision supersedes, it does not edit,
an accepted one" rule. Those are correct as-is, not a gap.

Expected: the `grep` command's output is empty (every live, non-historical reference
was updated — a non-empty result here is a real gap, go back to Step 4 or 5); the full
test suite passes; both validators exit 0.

- [ ] **Step 11: Commit**

```bash
git add -A
git commit -m "Migrate docs/DECISIONS.md to docs/decisions/ (one file per decision, generated index)"
```

---

### Task 5: Wire the index-freshness check into CI

**Descoped from this plan's PR — tracked as a separate follow-up issue.** This task's
own execution discovered a real constraint the plan never accounted for:
`scripts/check_roadmap_evidence.rb`'s `validate_evidence` hashes `.github/workflows/ci.yml`
**in its entirety** (`Digest::SHA256.hexdigest(workflow_text)`, the whole file's bytes) and
requires that digest to already appear in the reviewed `REVIEWED_PERF_CI_WORKFLOW_SHA256S`
allowlist — confirmed empirically: the live `ci.yml`'s current digest exactly matches
`D114_FRONTEND_PERF_THRESHOLD_CI_WORKFLOW_SHA256`, the currently-accepted entry. Any byte
change to `ci.yml`, including this task's own harmless new step, breaks that check
immediately — this is exactly the condition AGENTS.md's CI-privilege-boundary section and
`issue-implement`'s own staged-pattern section describe ("If the diff touches a workflow
file under `.github/workflows/` **and** requires registering a new digest... split the work
into two sequential pull requests"), and this repository has done exactly that nine times
already (`tests/fixtures/d51-...` through `d114-...`).

This plan's Task 1-4 PR does not touch `ci.yml` at all and delivers complete, independently
valuable, already-reviewed work on its own (the migration itself, with a generated index that
is already correct — CI enforcement of its freshness is a safety net against *future* drift,
not a correctness requirement for the migration that already happened). Bundling a staged
CI-workflow-digest change into that PR would mix an unrelated governance mechanism into a
documentation-structure migration, and the two-PR stage/activate pattern is itself designed
to be its own focused pair, not appended to unrelated work. Task 5 is therefore filed as its
own GitHub issue for a dedicated stage-then-activate PR pair after this plan's PR merges,
rather than executed here. The steps below are preserved as reference for whoever picks up
that follow-up issue — they describe the *activation* half; the *stage* half (checking in the
target byte-exact `ci.yml` as a `tests/fixtures/` entry and registering its digest in
`scripts/check_roadmap_evidence.rb`, touching no other file) is not detailed here and should
be derived fresh from `issue-implement`'s own staged-pattern section at execution time, since
the exact digest depends on `ci.yml`'s content at that later point in time.

**Files:**
- Modify: `.github/workflows/ci.yml`

**Interfaces:** none new.

- [ ] **Step 1: Add a CI step**

In `.github/workflows/ci.yml`, immediately after the existing "Check roadmap evidence"
step (which itself follows the exact `test_*.rb` then `check_*.rb` pattern this new
check mirrors), add:

```yaml
      - name: Check decisions index
        run: |
          python3 -B scripts/generate_decisions_index.py docs/decisions docs/decisions/README.md --check
```

- [ ] **Step 2: Verify locally**

```bash
python3 -B scripts/generate_decisions_index.py docs/decisions docs/decisions/README.md --check
```

Expected: exit 0.

- [ ] **Step 3: Run the full local gate set once more**

```bash
python3 -m unittest discover -s scripts -p "test_*.py"
python3 scripts/validate_agent_assets.py
python3 scripts/validate_agent_policies.py
ruby scripts/test_check_roadmap_evidence.rb && ruby scripts/check_roadmap_evidence.rb
ruby scripts/test_check_ci_permissions.rb && ruby scripts/check_ci_permissions.rb
```

Expected: all commands exit 0 — the new CI step must not be the only thing verifying
this; every gate this branch's diff could plausibly affect is confirmed green locally
before it depends on CI to catch a problem.

- [ ] **Step 4: Commit**

```bash
git add .github/workflows/ci.yml
git commit -m "Wire generate_decisions_index.py --check into CI"
```
