#!/usr/bin/env python3
"""Gate line coverage of the Rust lines a change adds or modifies (D-242).

`docs/decisions/D-242-product-mode-the-delivery-process-informs-rather-than-blocks.md`
narrows the merge invariant from "100% line and region coverage of the whole
workspace" to "100% line coverage of the lines this pull request adds or
modifies", with total coverage reported rather than enforced. This script is
the gate. It joins two inputs that the coverage job already produces:

* an LCOV export from ``cargo llvm-cov --workspace --lcov --output-path``;
* a zero-context unified diff (``git diff -U0 <base> HEAD``) of the change.

What counts
-----------

A diff path is a *changed source file* when it is a Rust file that
cargo-llvm-cov instruments by default. That is cargo-llvm-cov 0.8.7's own
default exclusion (``src/report.rs:919-923``): the path ends in ``.rs``, is
not ``build.rs``, has no ``tests``, ``examples``, or ``benches`` directory
component, and its basename does not match ``^(tests\\.rs|[0-9a-zA-Z_-]+[_-]tests\\.rs)$``.
Every other changed path (docs, YAML, scripts, integration tests) is ignored.

For a changed source file, the lines the diff adds or modifies are looked up
in the file's LCOV ``DA:`` records. A line with no ``DA:`` record (a comment,
a brace, a signature, an attribute) is not executable and is ignored. A line
whose record has zero hits is uncovered. A changed source file that has no
LCOV record at all fails the gate closed: D-014's exemption policy is a
documented whole-file ``--ignore-filename-regex`` entry, so an instrumentable
file without coverage data is a gate that would otherwise silently pass.

The threshold ``--require-changed-lines N`` (0-100, default 100) is compared
against ``covered / changed`` in percent; a change with no counted lines
passes. The LCOV ``LF``/``LH`` totals are printed as the workspace line
coverage report and never gate.

Exit status: 0 when the threshold is met, 1 when it is not or a changed
source file has no coverage data, 2 on a usage or parse error.
"""

from __future__ import annotations

import argparse
import os
import re
import sys
from pathlib import Path
from typing import Dict, List, Optional, Set, Tuple

HUNK = re.compile(r"^@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@")
# cargo-llvm-cov 0.8.7 src/report.rs:919-923 default exclusion.
EXCLUDED_BASENAME = re.compile(r"^(tests\.rs|[0-9a-zA-Z_-]+[_-]tests\.rs)$")
EXCLUDED_DIRS = ("tests", "examples", "benches")


class DiffCoverageError(Exception):
    """Malformed LCOV or diff input."""


def is_changed_source(path: str) -> bool:
    """Whether cargo-llvm-cov instruments ``path`` by default."""
    if not path.endswith(".rs"):
        return False
    parts = path.split("/")
    basename = parts[-1]
    if basename == "build.rs":
        return False
    if any(part in EXCLUDED_DIRS for part in parts[:-1]):
        return False
    if EXCLUDED_BASENAME.match(basename):
        return False
    return True


def _root_prefixes(root: str) -> List[str]:
    """The root as given plus its resolved form (macOS ``/var`` vs ``/private/var``)."""
    prefixes = [os.path.abspath(root)]
    resolved = str(Path(root).resolve())
    if resolved not in prefixes:
        prefixes.append(resolved)
    return [prefix.replace(os.sep, "/").rstrip("/") + "/" for prefix in prefixes]


def _relative(path: str, prefixes: List[str]) -> str:
    """Strip a root prefix from an absolute ``SF:`` path; leave relative paths alone."""
    normalized = path.replace(os.sep, "/")
    if not normalized.startswith("/"):
        return normalized
    for prefix in prefixes:
        if normalized.startswith(prefix):
            return normalized[len(prefix):]
    return normalized


def parse_lcov(text: str, root: str) -> Tuple[Dict[str, Dict[int, int]], int, int]:
    """Return ``{repo-relative path: {line: max hits}}`` and total (LF, LH)."""
    files: Dict[str, Dict[int, int]] = {}
    current: Optional[Dict[int, int]] = None
    found = hit = 0
    prefixes = _root_prefixes(root)
    for number, raw in enumerate(text.splitlines(), start=1):
        line = raw.strip()
        if not line:
            continue
        if line.startswith("SF:"):
            current = files.setdefault(_relative(line[3:], prefixes), {})
        elif line.startswith("DA:"):
            if current is None:
                raise DiffCoverageError(f"line {number}: DA record before any SF record")
            fields = line[3:].split(",")
            try:
                line_number, hits = int(fields[0]), int(fields[1])
            except (IndexError, ValueError):
                raise DiffCoverageError(f"line {number}: malformed DA record {raw!r}") from None
            current[line_number] = max(current.get(line_number, 0), hits)
        elif line.startswith("LF:") or line.startswith("LH:"):
            try:
                value = int(line[3:])
            except ValueError:
                raise DiffCoverageError(f"line {number}: malformed {line[:2]} record {raw!r}") from None
            if line.startswith("LF:"):
                found += value
            else:
                hit += value
        elif line == "end_of_record":
            current = None
    return files, found, hit


def parse_added_lines(diff: str) -> Dict[str, Set[int]]:
    """Return ``{path: {added or modified line numbers}}`` from a ``-U0`` diff."""
    added: Dict[str, Set[int]] = {}
    path: Optional[str] = None
    in_file = False
    old_left = new_left = 0
    for number, raw in enumerate(diff.splitlines(), start=1):
        if raw.startswith("diff --git "):
            path = None
            in_file = True
            old_left = new_left = 0
            continue
        # A removed line whose content starts with "-- " (or an added line
        # starting with "++ ") is indistinguishable from a file header by
        # prefix alone; the hunk counters settle it: inside a hunk with
        # removals (additions) still owed, the line is content.
        if raw.startswith("--- ") and old_left <= 0:
            continue
        if raw.startswith("+++ ") and new_left <= 0:
            if not in_file:
                raise DiffCoverageError(f"line {number}: +++ header before any diff --git header")
            target = raw[4:]
            if target == "/dev/null":
                path = None
            elif target.startswith("b/"):
                if target[2:].startswith('"'):
                    raise DiffCoverageError(f"line {number}: C-quoted path is not supported {raw!r}")
                path = target[2:]
            elif target.startswith('"'):
                raise DiffCoverageError(f"line {number}: C-quoted path is not supported {raw!r}")
            else:
                path = target
            old_left = new_left = 0
            continue
        if raw.startswith("@@"):
            match = HUNK.match(raw)
            if not match:
                raise DiffCoverageError(f"line {number}: malformed hunk header {raw!r}")
            old_left = int(match.group(2)) if match.group(2) is not None else 1
            start = int(match.group(3))
            new_left = int(match.group(4)) if match.group(4) is not None else 1
            if path is not None and new_left:
                added.setdefault(path, set()).update(range(start, start + new_left))
            continue
        if raw.startswith("\\"):
            continue
        if raw.startswith("+"):
            if new_left <= 0:
                raise DiffCoverageError(f"line {number}: added line outside the hunk's declared range")
            new_left -= 1
        elif raw.startswith("-"):
            if old_left <= 0:
                raise DiffCoverageError(f"line {number}: removed line outside the hunk's declared range")
            old_left -= 1
        elif raw.startswith(" "):
            if old_left <= 0 or new_left <= 0:
                raise DiffCoverageError(f"line {number}: context line outside the hunk's declared range")
            old_left -= 1
            new_left -= 1
    return added


def evaluate(
    coverage: Dict[str, Dict[int, int]], added: Dict[str, Set[int]], threshold: float
) -> Tuple[bool, Dict[str, List[int]], List[str], int, int]:
    """Return (passed, uncovered lines per file, files without data, changed, covered)."""
    uncovered: Dict[str, List[int]] = {}
    missing: List[str] = []
    changed = covered = 0
    for path in sorted(added):
        if not is_changed_source(path):
            continue
        hits = coverage.get(path)
        if hits is None:
            missing.append(path)
            continue
        misses: List[int] = []
        for line in sorted(added[path]):
            if line not in hits:
                continue
            changed += 1
            if hits[line] > 0:
                covered += 1
            else:
                misses.append(line)
        if misses:
            uncovered[path] = misses
    percent = 100.0 if changed == 0 else 100.0 * covered / changed
    passed = not missing and percent >= threshold
    return passed, uncovered, missing, changed, covered


def format_ranges(lines: List[int]) -> str:
    out: List[str] = []
    start: Optional[int] = None
    prev = 0
    for line in lines:
        if start is None:
            start = prev = line
        elif line == prev + 1:
            prev = line
        else:
            out.append(str(start) if start == prev else f"{start}-{prev}")
            start = prev = line
    if start is not None:
        out.append(str(start) if start == prev else f"{start}-{prev}")
    return ", ".join(out)


def _threshold(value: str) -> float:
    try:
        number = float(value)
    except ValueError:
        raise argparse.ArgumentTypeError(f"not a number: {value!r}") from None
    if not 0 <= number <= 100:
        raise argparse.ArgumentTypeError(f"threshold must be between 0 and 100: {value!r}")
    return number


def main(argv: Optional[List[str]] = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--lcov", required=True, help="LCOV export from cargo llvm-cov --lcov")
    parser.add_argument("--diff", required=True, help="unified diff produced with -U0 (may be empty)")
    parser.add_argument(
        "--root",
        default=None,
        help="repository root stripped from absolute SF: paths (default: current directory)",
    )
    parser.add_argument(
        "--require-changed-lines",
        type=_threshold,
        default=100.0,
        metavar="N",
        help="minimum percent of changed executable lines that must be covered (default 100)",
    )
    try:
        args = parser.parse_args(argv)
    except SystemExit as error:
        return 2 if error.code else 0
    root = args.root if args.root else os.getcwd()
    try:
        coverage, found, hit = parse_lcov(Path(args.lcov).read_text(encoding="utf-8"), root)
        added = parse_added_lines(Path(args.diff).read_text(encoding="utf-8"))
    except (OSError, UnicodeDecodeError, DiffCoverageError) as error:
        print(f"check_diff_coverage: {error}", file=sys.stderr)
        return 2
    passed, uncovered, missing, changed, covered = evaluate(
        coverage, added, args.require_changed_lines
    )
    for path in missing:
        print(f"{path}: no coverage data for changed source file")
    for path, lines in uncovered.items():
        print(f"{path}: uncovered lines {format_ranges(lines)}")
    percent = 100.0 if changed == 0 else 100.0 * covered / changed
    print(f"changed lines: {changed}, covered: {covered} ({percent:.2f}%)")
    total = 100.0 * hit / found if found else 0.0
    print(f"workspace lines: {hit}/{found} ({total:.2f}%)")
    if passed:
        print(f"diff coverage gate passed (threshold {args.require_changed_lines:g}%)")
        return 0
    print(f"diff coverage gate FAILED (threshold {args.require_changed_lines:g}%)")
    return 1


if __name__ == "__main__":
    sys.exit(main())
