#!/usr/bin/env python3
"""Reject new raw `std::env::temp_dir().join(...)` call sites (issue #781).

#779 found ~384 ad hoc `std::env::temp_dir().join(...)` call sites across 36
tracked `.rs` files, almost all of them test code building their own scratch
directory by hand: no shared cleanup, so a panic before a manual
`remove_dir_all` left the directory behind, and no collision-safe naming, so
two call sites picking the same name could clash. Part 1 (#781) added
`pycc_scratch::ScratchDir` (`crates/pycc_scratch/src/lib.rs`) as the one
correct way to get a scratch directory: `Drop`-based cleanup that survives a
panic unwind, plus a `pid`/`nanos`/`seq` naming scheme that cannot collide.

This script is the enforcement mechanism requirement 7 of #779 asks for: it
scans every tracked `*.rs` file for the literal pattern `temp_dir().join(`
(tolerant of whitespace/line breaks between the two calls, since some
existing call sites wrap the expression across lines) plus a narrower
`use ... temp_dir as <name>` import-alias pattern (see `ALIAS_PATTERN`
below), and fails if a file contains *more* such occurrences than
`ALLOWLIST` records for it.

Two allowances, both narrow and deliberate:

* `crates/pycc_scratch/src/lib.rs` is exempt unconditionally -- it is the one
  legitimate implementation site `ScratchDir::new` itself lives in.
* `ALLOWLIST` was a one-time snapshot, generated mechanically at Part 1's
  own merge commit, mapping every other file that already contained the
  banned pattern to its exact occurrence count at that commit (**not** a
  bare filename list: a per-file count kept an already-listed file from
  accumulating brand-new raw call sites while the pre-existing backlog was
  tolerated; the count was only ever allowed to stay the same or go down,
  a review convention backstopped by the D-068 pinned reviewer pass, since
  this script only ever sees the current tree's `ALLOWLIST`). The backlog
  has since been fully burned down: Part 2 (#782) migrated every test-file
  entry off the banned pattern (`tests/quick_start.rs` last, via the
  evidence-hero re-attestation its byte pin required), and Part 3 (#783)
  migrated `src/main.rs`'s two production call sites -- the last entry --
  onto caller-owned `ScratchDir`s. `ALLOWLIST` is now empty, which is the
  literal completeness signal for #779's requirements 4/5 (Parts 2/3), and
  its terminal state: every tracked `.rs` file outside `EXEMPT_FILES` is
  held to the new rule, so any occurrence at all is a violation.

**Known scope limitation, accepted deliberately** (recorded in the D-201
decision entry, not a silent gap): this is a textual pattern match, not a
data-flow analysis. A caller that splits the expression across a binding --
`let dir = std::env::temp_dir(); ... dir.join(...)` -- evades the pattern
even though it has the same effect as the banned call. Similarly, an
import-alias re-export -- `use std::env::temp_dir as get_scratch_root;`
followed by `get_scratch_root().join(...)` -- evades the primary
`temp_dir().join(` pattern entirely, and is ordinary, idiomatic Rust rather
than a deliberately obfuscated call shape. `ALIAS_PATTERN` below narrows that
specific gap by flagging the `use ... temp_dir as <name>` import line itself
as a violation signal (it cannot see whether the aliased name is later
`.join(...)`-ed, since that is exactly the data-flow question this script
does not attempt to answer), but does not attempt to catch every possible
re-export shape (e.g. a re-exported wrapper function, or aliasing through a
`mod` boundary). A textual check on the literal call shape is what
requirement 7 asks for; a variable-indirected or wrapper-function rewrite is
a deliberate, conspicuous evasion, not something ordinary refactoring
produces by accident, and the D-068 pinned reviewer pass on every PR is the
backstop for a deliberately obfuscated call site.
"""

from __future__ import annotations

import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent

# The one legitimate implementation site. Exempt unconditionally, not counted
# in ALLOWLIST.
EXEMPT_FILES = frozenset({"crates/pycc_scratch/src/lib.rs"})

# Originally a one-time snapshot generated mechanically at Part 1's (#781's)
# own merge commit via `git ls-files '*.rs'` piped through a count of this
# script's own pattern match per file (see the docstring above). Part 2
# (#782) migrated every test-file entry, and Part 3 (#783) migrated the last
# entry -- `src/main.rs`'s two production call sites (`try_build`'s temp
# object, `run`'s output executable) -- onto caller-owned
# `pycc_scratch::ScratchDir`s. Empty is this dict's terminal state: any raw
# occurrence in any tracked `.rs` file outside `EXEMPT_FILES` is now a
# violation, and nothing should ever be added back here.
ALLOWLIST: dict[str, int] = {}

# Tolerant of whitespace/line breaks between `temp_dir()` and `.join(`, since
# some existing call sites wrap the expression across lines.
PATTERN = re.compile(r"temp_dir\s*\(\s*\)\s*\.\s*join\s*\(")

# Narrower second signal: an import that aliases `std::env::temp_dir` (or
# `env::temp_dir`, for a caller that already has `std::env` or `env` in
# scope) to a different local name -- e.g.
# `use std::env::temp_dir as get_scratch_root;`. This evades PATTERN
# entirely once the aliased name is called instead of the literal
# `temp_dir()` shape, and is ordinary idiomatic Rust rather than a
# deliberately obfuscated call site. See the "Known scope limitation"
# paragraph in the module docstring for what this does and does not catch.
ALIAS_PATTERN = re.compile(r"use\s+(?:::)?(?:std\s*::\s*)?env\s*::\s*temp_dir\s+as\s+\w+")


class ScratchDirUsageError(Exception):
    """A tracked `.rs` file has more banned occurrences than it is allowed."""


def tracked_rust_files(root: Path = ROOT) -> list[str]:
    result = subprocess.run(
        ["git", "ls-files", "-z", "*.rs"],
        cwd=root,
        check=True,
        capture_output=True,
    )
    return sorted(
        entry.decode("utf-8") for entry in result.stdout.split(b"\0") if entry
    )


def occurrence_count(path: Path) -> int:
    text = path.read_text(encoding="utf-8")
    return len(PATTERN.findall(text)) + len(ALIAS_PATTERN.findall(text))


def find_violations(
    files: list[str],
    allowlist: dict[str, int],
    root: Path = ROOT,
) -> dict[str, tuple[int, int]]:
    """Returns {file: (found, allowed)} for every file exceeding its allowance."""
    violations: dict[str, tuple[int, int]] = {}
    for relative_path in files:
        if relative_path in EXEMPT_FILES:
            continue
        found = occurrence_count(root / relative_path)
        if found == 0:
            continue
        allowed = allowlist.get(relative_path, 0)
        if found > allowed:
            violations[relative_path] = (found, allowed)
    return violations


def validate(files: list[str], allowlist: dict[str, int], root: Path = ROOT) -> None:
    violations = find_violations(files, allowlist, root)
    if not violations:
        return
    lines = [
        "scripts/check_scratch_dir_usage.py: found raw "
        "`std::env::temp_dir().join(...)` call sites that are new or grew "
        "past their recorded allowance:",
        "",
    ]
    for relative_path in sorted(violations):
        found, allowed = violations[relative_path]
        lines.append(f"  {relative_path}: found {found}, allowed {allowed}")
    lines += [
        "",
        "Use `pycc_scratch::ScratchDir` (crates/pycc_scratch/src/lib.rs) "
        "instead of a raw `std::env::temp_dir().join(...)` call -- see "
        "docs/TESTING.md's \"Scratch directories\" section. If this file is "
        "already in ALLOWLIST and you migrated some (not all) of its "
        "occurrences off the banned pattern, lower its recorded count in "
        "scripts/check_scratch_dir_usage.py to match.",
    ]
    raise ScratchDirUsageError("\n".join(lines))


def main() -> int:
    try:
        validate(tracked_rust_files(), ALLOWLIST)
    except ScratchDirUsageError as exc:
        print(str(exc), file=sys.stderr)
        return 1
    print("scripts/check_scratch_dir_usage.py: no new raw temp_dir().join(...) call sites")
    return 0


if __name__ == "__main__":
    sys.exit(main())
