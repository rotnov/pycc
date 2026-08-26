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
* `ALLOWLIST` is a one-time snapshot, generated mechanically at Part 1's own
  merge commit, mapping every other file that already contained the banned
  pattern to its exact occurrence count at that commit (**not** a bare
  filename list -- see below). A file not in `ALLOWLIST` is held to the new
  rule from the moment this check merges: any occurrence at all is a
  violation. A file that *is* in `ALLOWLIST` may keep its existing
  occurrences; Part 2 (#782) has since migrated every test-file entry off
  the banned pattern except `tests/quick_start.rs`, whose exact bytes are
  pinned by the site's versioned evidence-hero contract (see the entry's
  own comment below), so the remaining entries are that pinned file and
  `src/main.rs`'s two production call sites, Part 3's job, tracked by
  https://github.com/rotnov/pycc/issues/783. An entry's count is
  intended to only stay the same or go down on any later pull request --
  never up. That intent is a review convention, not something this script
  mechanically enforces across commits: this check only compares the
  current tree's occurrence count against `ALLOWLIST`'s recorded value for
  the same commit, with no visibility into a prior commit's `ALLOWLIST`
  entry -- so a pull request that both adds new raw call sites to an
  already-listed file and raises that file's `ALLOWLIST` count to match
  would still pass. The D-068 pinned reviewer pass on every pull request is
  the intended backstop for this gap, same as for the textual-pattern-match
  limitation below; a merge-base-comparison check may be added later if
  this proves insufficient in practice. A bare filename allowlist would let
  an already-listed file accumulate brand-new raw `temp_dir().join(...)`
  calls undetected without even that backstop being triggered, which
  defeats the actual goal of requirement 7 (stop the leak from getting
  worse); the per-file count closes that gap while still tolerating the
  pre-existing backlog. `ALLOWLIST` reaching empty is the literal
  completeness signal for closing out #779's requirements 4/5 (Parts 2/3).

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

# One-time snapshot generated mechanically at Part 1's (#781's) own merge
# commit via `git ls-files '*.rs'` piped through a count of this script's own
# pattern match per file (see the docstring above). Part 2 (#782) migrated
# every test-file entry except `tests/quick_start.rs` (evidence-pinned, see
# below), so the remaining entries are that file and `src/main.rs`, owned by
# Part 3 (#783).
#
# `src/main.rs`'s count of 2 (down from the snapshot's 7 after #782's
# Batch B migrated all five of its test-only sites and removed
# `src/project_config.rs`'s entry outright, its 26 sites all migrated) is
# exactly its two production call sites, owned by Part 3 (#783): the
# `try_build_obj_path` helper carrying `try_build`'s temp object path --
# which the release-isolation test also calls, so reading back the exact
# object `try_build` wrote needs no second raw call site -- and `run`'s
# output path.
ALLOWLIST: dict[str, int] = {
    "src/main.rs": 2,
    # The one test-file site #782 could not migrate: the public site's
    # versioned evidence-hero contract (docs/WEBSITE.md, enforced by
    # `scripts/check-site.sh` via `site/evidence-heroes.json`) pins this
    # file's exact canonical bytes -- both by SHA-256 and byte-for-byte
    # against the reviewed evidence commit `8ccc05b5` -- so any edit,
    # including this migration, fails the Pages `build` check until the
    # landing hero is re-attested (new evidence commit, accepted CI run,
    # updated allowlist and site projections). Migrate this site only as
    # part of that re-attestation ceremony.
    "tests/quick_start.rs": 1,
}

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
