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
  occurrences (migrating them off `temp_dir().join(...)` is Part 2's job,
  tracked by https://github.com/rotnov/pycc/issues/779); its count is
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
# pattern match per file (see the docstring above). Every entry here is a
# file Part 2 (#782) or Part 3 (#783) still needs to migrate; `src/main.rs`'s
# count includes both its two production call sites (Part 3's own scope,
# `try_build`/`run`) and its five test-only ones (Part 2's scope, inside its
# `#[cfg(test)] mod tests`).
ALLOWLIST: dict[str, int] = {
    # Added post-snapshot: `main` merged issue #150's fix (a `tests/`
    # integration test using the raw pattern) after Part 1's original
    # snapshot commit but before this PR's own merge onto `main` during a
    # rebase. The snapshot's definition is "every file containing the
    # pattern at the commit where the gate takes effect" -- that commit is
    # this rebased merge, not the pre-rebase branch tip -- so recording this
    # file here fulfills the snapshot rather than breaching its "one-time"
    # property (see D-201). Migrating it is out of scope for this PR: it
    # would require adding `pycc_scratch` back as a root `[dev-dependencies]`
    # entry, which f79bb2b5 already tried and reverted because it trips
    # D-091's bench-manifest fingerprint gate in `frontend-perf-measure` --
    # the same blocker tracked against #782 Batch B (PR #793). It stays
    # tracked under #782's Part 2 migration scope alongside every other
    # entry below.
    "tests/issue_150_zero_step_range.rs": 1,
    "src/main.rs": 7,
    "src/project_config.rs": 26,
    "tests/conformance.rs": 1,
    "tests/container_methods1_codegen_depth.rs": 1,
    "tests/issue_146_bigint_release.rs": 2,
    "tests/issue_147_bigint_range.rs": 1,
    "tests/issue_148_oversized_int_literal.rs": 3,
    "tests/issue_167_none_carrier_abi.rs": 1,
    "tests/issue_22_execution_order.rs": 15,
    "tests/issue_377_property.rs": 8,
    "tests/issue_378_dataclasses.rs": 25,
    "tests/issue_379_enum.rs": 21,
    "tests/issue_380_protocols.rs": 22,
    "tests/issue_381_match.rs": 20,
    "tests/issue_382_exceptions.rs": 56,
    "tests/issue_386_class_method_redefinition.rs": 8,
    "tests/issue_432_inheritance.rs": 11,
    "tests/issue_433_super.rs": 15,
    "tests/issue_435_isinstance_issubclass.rs": 33,
    "tests/issue_436_classmethod_staticmethod.rs": 18,
    "tests/issue_542_except_star.rs": 41,
    "tests/issue_575_str_repetition.rs": 2,
    "tests/issue_603_unary_general_operand.rs": 1,
    "tests/issue_630_pycc_rt_build_dependency.rs": 1,
    "tests/issue_702_user_exceptions.rs": 1,
    "tests/issue_739_oserror_hierarchy.rs": 1,
    "tests/issue_740_multi_type_except.rs": 1,
    "tests/issue_762_typing_final_annotated.rs": 3,
    "tests/issue_767_typing_cast.rs": 6,
    "tests/issue_770_optional_reassignment.rs": 2,
    "tests/nbody_bench.rs": 1,
    "tests/pycc_toml_release_default.rs": 1,
    "tests/quick_start.rs": 1,
    "tests/slice0.rs": 62,
    "tests/slice1_codegen_depth.rs": 4,
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
