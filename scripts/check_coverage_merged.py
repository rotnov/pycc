#!/usr/bin/env python3
"""Merged-coverage gate.

Replaces `cargo llvm-cov --fail-under-lines 100 --fail-under-regions 100`
with a check that merges coverage across all instantiations before
counting missed lines and regions.

Why this exists (D-171):
  `llvm-cov`'s summary statistics count regions and lines *per
  instantiation*.  When a generic Rust function is monomorphized into
  multiple instantiations, a source region that is covered by at least
  one instantiation but not by all of them appears as "missed" in the
  summary even though the merged (HTML / LCOV) view shows it as fully
  covered.  The per-instantiation summary therefore overcounts misses
  and can report < 100% coverage for code that is, in fact, fully
  covered.

  This script reads the `llvm-cov export` JSON (produced by
  `cargo llvm-cov --json --output-path`) and computes coverage the same
  way the HTML report does: by merging across all instantiations.

  * Line coverage:  a line is missed when *every* segment on that line
    (across all instantiations) has count 0.
  * Region coverage: a region is missed when *every* instantiation of
    that source region has count 0.

Exit status:
  0  — 0 missed lines and 0 missed regions (100% merged coverage).
  1  — at least one missed line or region, or a usage / parse error.
"""

from __future__ import annotations

import json
import sys
from collections import defaultdict


def _line_coverage(files: list[dict]) -> list[tuple[str, int]]:
    """Return [(filename, line)] for every missed line, merged across instantiations."""
    missed: list[tuple[str, int]] = []
    for f in files:
        fn = f["filename"]
        segs = f.get("segments", [])
        if not segs:
            continue
        # Segments are sorted by (line, col) by llvm-cov.
        # A segment is [line, col, count, hasCount, kind].
        # hasCount=True  → this segment starts a new region with `count`.
        # hasCount=False → continuation; inherits the previous count.
        #
        # A *line* is covered when any segment on that line has a
        # positive count (directly or inherited).  We walk the segments
        # in order, tracking the "current count" (the count of the
        # region we are currently inside), and for each line record
        # whether we saw a positive count.
        current_count = 0
        line_has_positive: dict[int, bool] = defaultdict(bool)
        for seg in segs:
            line, _col, count, has_count = seg[0], seg[1], seg[2], seg[3]
            if has_count:
                current_count = count
            if current_count > 0:
                line_has_positive[line] = True
        # A line is "missed" when it has at least one segment (i.e. it
        # is executable) but no segment ever gave it a positive count.
        lines_with_segs: set[int] = set()
        for seg in segs:
            lines_with_segs.add(seg[0])
        for line in sorted(lines_with_segs):
            if not line_has_positive.get(line, False):
                missed.append((fn, line + 1))  # llvm-cov lines are 0-based
    return missed


def _is_test_file(filename: str) -> bool:
    """Check if a file is a test file (excluded from the coverage denominator."""
    return (
        "/tests/" in filename
        or filename.endswith("_tests.rs")
        or filename.endswith("/tests.rs")
    )


def _region_coverage(
    functions: list[dict], product_files: set[str]
) -> list[tuple[str, int, int, int, int]]:
    """Return [(filename, line, col, end_line, end_col)] for every missed region."""
    # (filename, line, col, end_line, end_col) → list of counts across instantiations
    region_counts: dict[tuple[str, int, int, int, int], list[int]] = defaultdict(list)
    for func in functions:
        filenames = func.get("filenames", [])
        regions = func.get("regions", [])
        if not regions:
            continue
        # A function can span multiple files (rare).  Map region index
        # to file by position — but in practice llvm-cov puts all
        # regions for the function's primary file first.  We use the
        # first filename for all regions (the common case for Rust).
        if not filenames:
            continue
        fn = filenames[0]
        # Skip test files — they are excluded from the coverage denominator
        # by cargo-llvm-cov by default (D-014).
        if fn not in product_files:
            continue
        for r in regions:
            # r = [line, col, end_line, end_col, count, kind, kind2, kind3]
            key = (fn, r[0], r[1], r[2], r[3])
            region_counts[key].append(r[4])
    missed: list[tuple[str, int, int, int, int]] = []
    for key, counts in region_counts.items():
        if all(c == 0 for c in counts):
            fn, line, col, end_line, end_col = key
            missed.append((fn, line + 1, col + 1, end_line + 1, end_col + 1))
    return missed


def main(argv: list[str]) -> int:
    if len(argv) != 2:
        print(f"usage: {argv[0]} <coverage.json>", file=sys.stderr)
        return 1
    json_path = argv[1]
    try:
        with open(json_path) as f:
            data = json.load(f)
    except (OSError, json.JSONDecodeError) as e:
        print(f"error: cannot read {json_path}: {e}", file=sys.stderr)
        return 1

    # The cargo-llvm-cov JSON wraps everything in data[0].
    if "data" in data and isinstance(data["data"], list) and data["data"]:
        inner = data["data"][0]
    else:
        inner = data

    files = inner.get("files", [])
    functions = inner.get("functions", [])
    product_files = {f["filename"] for f in files}

    # Fail closed: if llvm-cov produced no file or function data, the
    # report is structurally empty (e.g. instrumentation or target-selection
    # failure).  A 100% gate must not pass on zero measured product code.
    if not product_files:
        print(
            "Coverage gate: empty report — no product files found",
            file=sys.stderr,
        )
        return 1
    if not functions:
        print(
            "Coverage gate: empty report — no functions found",
            file=sys.stderr,
        )
        return 1

    missed_lines = _line_coverage(files)
    missed_regions = _region_coverage(functions, product_files)

    if not missed_lines and not missed_regions:
        print("Merged coverage: 100% (0 missed lines, 0 missed regions)")
        return 0

    if missed_lines:
        print(f"Missed lines ({len(missed_lines)}):", file=sys.stderr)
        for fn, line in missed_lines[:50]:
            # Shorten the filename for readability.
            short = fn.split("/crates/")[-1] if "/crates/" in fn else fn.split("/")[-1]
            print(f"  {short}:{line}", file=sys.stderr)
        if len(missed_lines) > 50:
            print(f"  ... and {len(missed_lines) - 50} more", file=sys.stderr)

    if missed_regions:
        print(f"Missed regions ({len(missed_regions)}):", file=sys.stderr)
        for fn, line, col, end_line, end_col in missed_regions[:50]:
            short = fn.split("/crates/")[-1] if "/crates/" in fn else fn.split("/")[-1]
            print(f"  {short}:{line}:{col} -> {end_line}:{end_col}", file=sys.stderr)
        if len(missed_regions) > 50:
            print(f"  ... and {len(missed_regions) - 50} more", file=sys.stderr)

    return 1


if __name__ == "__main__":
    sys.exit(main(sys.argv))
