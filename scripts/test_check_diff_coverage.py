#!/usr/bin/env python3
"""Mutation tests for scripts/check_diff_coverage.py (D-242).

Every behavior the diff-coverage gate claims is proven in both directions:
the accepting input passes and the corresponding defect is rejected with the
documented exit status. The module functions are imported directly for the
parser and evaluation cases; the last test drives the public CLI through a
subprocess, because the checker is a gate and its exit status is the contract.
"""

from __future__ import annotations

import importlib.util
import os
import subprocess
import sys
import tempfile
import unittest
from io import StringIO
from pathlib import Path
from unittest import mock

CHECKER_PATH = Path(__file__).with_name("check_diff_coverage.py")
CHECKER_SPEC = importlib.util.spec_from_file_location("diff_coverage_checker", CHECKER_PATH)
if CHECKER_SPEC is None or CHECKER_SPEC.loader is None:
    raise RuntimeError("could not load scripts/check_diff_coverage.py")
CHECKER = importlib.util.module_from_spec(CHECKER_SPEC)
CHECKER_SPEC.loader.exec_module(CHECKER)


def lcov(*records: "tuple[str, dict]") -> str:
    """Build an LCOV text from ``(path, {line: hits})`` records."""
    out = []
    for path, lines in records:
        out.append(f"SF:{path}")
        for line, hits in lines.items():
            out.append(f"DA:{line},{hits}")
        out.append(f"LF:{len(lines)}")
        out.append(f"LH:{sum(1 for hits in lines.values() if hits > 0)}")
        out.append("end_of_record")
    return "\n".join(out) + "\n"


def added(path: str, start: int, lines: "list[str]") -> str:
    """Build a ``-U0`` diff adding ``lines`` at ``start`` in ``path``."""
    count = "" if len(lines) == 1 else f",{len(lines)}"
    body = "\n".join("+" + line for line in lines)
    return (
        f"diff --git a/{path} b/{path}\n--- a/{path}\n+++ b/{path}\n"
        f"@@ -{start - 1},0 +{start}{count} @@\n{body}\n"
    )


SRC = "crates/pycc_hir/src/lower.rs"


class ChangedSourcePredicate(unittest.TestCase):
    def test_matches_cargo_llvm_cov_default_exclusion(self) -> None:
        ignored = [
            "docs/TESTING.md",
            "scripts/check_diff_coverage.py",
            "crates/pycc_runtime/build.rs",
            "crates/pycc_hir/src/tests.rs",
            "crates/pycc_hir/src/expr/tests.rs",
            "crates/pycc_hir/src/class/enum_call_tests.rs",
            "crates/pycc_hir/src/class/enum-call-tests.rs",
            "crates/pycc_hir/src/tests/foo.rs",
            "tests/x.rs",
            "crates/pycc_cli/tests/x.rs",
            "benches/x.rs",
            "crates/pycc_cli/examples/x.rs",
        ]
        counted = [
            SRC,
            "crates/pycc_hir/src/foo_test.rs",
            "crates/pycc_hir/src/testsuite.rs",
            "crates/pycc_hir/src/testing.rs",
            "crates/pycc_hir/src/build.rs.rs",
        ]
        for path in ignored:
            self.assertFalse(CHECKER.is_changed_source(path), path)
        for path in counted:
            self.assertTrue(CHECKER.is_changed_source(path), path)


class LcovParsing(unittest.TestCase):
    def test_duplicate_da_records_take_the_maximum(self) -> None:
        text = "SF:a.rs\nDA:3,0\nDA:3,7\nDA:3,2\nLF:1\nLH:1\nend_of_record\n"
        files, found, hit = CHECKER.parse_lcov(text, "/repo")
        self.assertEqual(files, {"a.rs": {3: 7}})
        self.assertEqual((found, hit), (1, 1))

    def test_root_prefix_is_stripped_from_absolute_sf_paths(self) -> None:
        text = f"SF:/Users/runner/work/pycc/pycc/{SRC}\nDA:1,1\nend_of_record\n"
        files, _, _ = CHECKER.parse_lcov(text, "/Users/runner/work/pycc/pycc/")
        self.assertEqual(list(files), [SRC])
        files, _, _ = CHECKER.parse_lcov(text, "/elsewhere")
        self.assertEqual(list(files), [f"/Users/runner/work/pycc/pycc/{SRC}"])

    def test_totals_sum_across_records(self) -> None:
        _, found, hit = CHECKER.parse_lcov(lcov(("a.rs", {1: 1, 2: 0}), ("b.rs", {1: 1})), "/r")
        self.assertEqual((found, hit), (3, 2))

    def test_da_before_sf_and_malformed_records_are_parse_errors(self) -> None:
        for text in ("DA:1,1\n", "SF:a.rs\nDA:x,1\n", "SF:a.rs\nDA:1\n", "SF:a.rs\nLF:many\n"):
            with self.assertRaises(CHECKER.DiffCoverageError, msg=text):
                CHECKER.parse_lcov(text, "/r")


class DiffParsing(unittest.TestCase):
    def test_omitted_count_means_one_line(self) -> None:
        diff = f"diff --git a/{SRC} b/{SRC}\n--- a/{SRC}\n+++ b/{SRC}\n@@ -4 +5 @@\n-old\n+new\n"
        self.assertEqual(CHECKER.parse_added_lines(diff), {SRC: {5}})

    def test_zero_count_hunk_adds_nothing(self) -> None:
        diff = f"diff --git a/{SRC} b/{SRC}\n--- a/{SRC}\n+++ b/{SRC}\n@@ -4,2 +3,0 @@\n-a\n-b\n"
        self.assertEqual(CHECKER.parse_added_lines(diff), {})

    def test_deleted_file_is_ignored(self) -> None:
        diff = f"diff --git a/{SRC} b/{SRC}\n--- a/{SRC}\n+++ /dev/null\n@@ -1,2 +0,0 @@\n-a\n-b\n"
        self.assertEqual(CHECKER.parse_added_lines(diff), {})

    def test_no_newline_marker_is_skipped(self) -> None:
        diff = added(SRC, 10, ["x"]) + "\\ No newline at end of file\n"
        self.assertEqual(CHECKER.parse_added_lines(diff), {SRC: {10}})

    def test_added_line_starting_with_plus_plus_is_content_not_a_header(self) -> None:
        diff = added(SRC, 10, ["++ not a header", "-- neither"]) + added("other.rs", 1, ["y"])
        self.assertEqual(CHECKER.parse_added_lines(diff), {SRC: {10, 11}, "other.rs": {1}})

    def test_removed_line_starting_with_minus_minus_is_content_not_a_header(self) -> None:
        # "-" + "-- x" renders as "--- x" inside a hunk whose old-side count is
        # still owed; the pending counter marks it as content, not a header
        # (the removal counterpart of the "++" case above).
        diff = (
            f"diff --git a/{SRC} b/{SRC}\n--- a/{SRC}\n+++ b/{SRC}\n@@ -4,2 +4 @@\n"
            "--- not a header\n-plain\n+kept\n"
        ) + added("other.rs", 1, ["y"])
        self.assertEqual(CHECKER.parse_added_lines(diff), {SRC: {4}, "other.rs": {1}})

    def test_path_is_the_whole_remainder_including_spaces(self) -> None:
        diff = "diff --git a/src/a b.rs b/src/a b.rs\n--- a/src/a b.rs\n+++ b/src/a b.rs\n@@ -0,0 +1 @@\n+x\n"
        self.assertEqual(CHECKER.parse_added_lines(diff), {"src/a b.rs": {1}})

    def test_c_quoted_path_is_a_parse_error(self) -> None:
        diff = 'diff --git "a/we\\303\\257rd.rs" "b/we\\303\\257rd.rs"\n--- "a/we\\303\\257rd.rs"\n+++ "b/we\\303\\257rd.rs"\n@@ -0,0 +1 @@\n+x\n'
        with self.assertRaises(CHECKER.DiffCoverageError):
            CHECKER.parse_added_lines(diff)

    def test_malformed_hunk_header_and_overflowing_hunk_are_parse_errors(self) -> None:
        bad_header = f"diff --git a/{SRC} b/{SRC}\n--- a/{SRC}\n+++ b/{SRC}\n@@ garbage @@\n+x\n"
        overflow = added(SRC, 10, ["x"]) + "+extra\n"
        stray_header = "+++ b/x.rs\n@@ -0,0 +1 @@\n+x\n"
        for diff in (bad_header, overflow, stray_header):
            with self.assertRaises(CHECKER.DiffCoverageError, msg=diff):
                CHECKER.parse_added_lines(diff)

    def test_empty_diff_adds_nothing(self) -> None:
        self.assertEqual(CHECKER.parse_added_lines(""), {})


class Evaluation(unittest.TestCase):
    def test_all_added_lines_covered_passes(self) -> None:
        coverage = {SRC: {10: 1, 11: 3}}
        passed, uncovered, missing, changed, covered = CHECKER.evaluate(coverage, {SRC: {10, 11}}, 100)
        self.assertTrue(passed)
        self.assertEqual((uncovered, missing, changed, covered), ({}, [], 2, 2))

    def test_one_uncovered_added_line_fails_and_is_listed(self) -> None:
        coverage = {SRC: {10: 1, 11: 0}}
        passed, uncovered, missing, changed, covered = CHECKER.evaluate(coverage, {SRC: {10, 11}}, 100)
        self.assertFalse(passed)
        self.assertEqual(uncovered, {SRC: [11]})
        self.assertEqual((missing, changed, covered), ([], 2, 1))

    def test_lines_without_da_records_are_ignored(self) -> None:
        passed, _, _, changed, covered = CHECKER.evaluate({SRC: {10: 1}}, {SRC: {9, 10, 11}}, 100)
        self.assertTrue(passed)
        self.assertEqual((changed, covered), (1, 1))

    def test_changed_source_file_absent_from_lcov_fails(self) -> None:
        passed, uncovered, missing, changed, _ = CHECKER.evaluate({"other.rs": {1: 1}}, {SRC: {1}}, 100)
        self.assertFalse(passed)
        self.assertEqual((uncovered, missing, changed), ({}, [SRC], 0))

    def test_non_source_changes_are_ignored_even_without_coverage(self) -> None:
        added_lines = {
            "docs/TESTING.md": {1},
            "crates/pycc_runtime/build.rs": {1},
            "crates/pycc_hir/src/expr/tests.rs": {1},
            "tests/conformance.rs": {1},
        }
        passed, uncovered, missing, changed, _ = CHECKER.evaluate({}, added_lines, 100)
        self.assertTrue(passed)
        self.assertEqual((uncovered, missing, changed), ({}, [], 0))

    def test_threshold_below_100_passes_a_partial_result(self) -> None:
        coverage = {SRC: {1: 1, 2: 1, 3: 1, 4: 0}}
        passed, _, _, changed, covered = CHECKER.evaluate(coverage, {SRC: {1, 2, 3, 4}}, 75)
        self.assertTrue(passed)
        self.assertEqual((changed, covered), (4, 3))
        passed, _, _, _, _ = CHECKER.evaluate(coverage, {SRC: {1, 2, 3, 4}}, 76)
        self.assertFalse(passed)

    def test_no_changed_lines_passes(self) -> None:
        passed, _, _, changed, covered = CHECKER.evaluate({}, {}, 100)
        self.assertTrue(passed)
        self.assertEqual((changed, covered), (0, 0))


class MainEntry(unittest.TestCase):
    def setUp(self) -> None:
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)

    def write(self, name: str, text: str) -> str:
        path = self.root / name
        path.write_text(text, encoding="utf-8")
        return str(path)

    def run_main(self, *args: str) -> "tuple[int, str, str]":
        out, err = StringIO(), StringIO()
        with mock.patch.object(sys, "stdout", out), mock.patch.object(sys, "stderr", err):
            code = CHECKER.main(list(args))
        return code, out.getvalue(), err.getvalue()

    def test_success_prints_totals_and_exits_zero(self) -> None:
        lcov_path = self.write("cov.lcov", lcov((SRC, {10: 1, 11: 2, 12: 0})))
        diff_path = self.write("changes.diff", added(SRC, 10, ["a", "b"]))
        code, out, _ = self.run_main("--lcov", lcov_path, "--diff", diff_path)
        self.assertEqual(code, 0, out)
        self.assertIn("changed lines: 2, covered: 2 (100.00%)", out)
        self.assertIn("workspace lines: 2/3 (66.67%)", out)

    def test_uncovered_line_exits_one_with_file_and_line(self) -> None:
        lcov_path = self.write("cov.lcov", lcov((SRC, {10: 1, 11: 0})))
        diff_path = self.write("changes.diff", added(SRC, 10, ["a", "b"]))
        code, out, _ = self.run_main("--lcov", lcov_path, "--diff", diff_path)
        self.assertEqual(code, 1, out)
        self.assertIn(f"{SRC}: uncovered lines 11", out)
        self.assertIn("changed lines: 2, covered: 1 (50.00%)", out)

    def test_missing_coverage_for_changed_source_exits_one(self) -> None:
        lcov_path = self.write("cov.lcov", lcov(("crates/other/src/lib.rs", {1: 1})))
        diff_path = self.write("changes.diff", added(SRC, 1, ["a"]))
        code, out, _ = self.run_main("--lcov", lcov_path, "--diff", diff_path)
        self.assertEqual(code, 1, out)
        self.assertIn(f"{SRC}: no coverage data for changed source file", out)

    def test_empty_diff_prints_totals_and_exits_zero(self) -> None:
        lcov_path = self.write("cov.lcov", lcov((SRC, {1: 1, 2: 0})))
        diff_path = self.write("changes.diff", "")
        code, out, _ = self.run_main("--lcov", lcov_path, "--diff", diff_path)
        self.assertEqual(code, 0, out)
        self.assertIn("changed lines: 0, covered: 0 (100.00%)", out)
        self.assertIn("workspace lines: 1/2 (50.00%)", out)

    def test_root_maps_absolute_sf_paths(self) -> None:
        lcov_path = self.write("cov.lcov", lcov((f"{self.root}/{SRC}", {1: 1})))
        diff_path = self.write("changes.diff", added(SRC, 1, ["a"]))
        code, out, _ = self.run_main("--lcov", lcov_path, "--diff", diff_path, "--root", str(self.root))
        self.assertEqual(code, 0, out)
        code, out, _ = self.run_main("--lcov", lcov_path, "--diff", diff_path, "--root", str(self.root / "x"))
        self.assertEqual(code, 1, out)

    def test_threshold_flag_accepts_partial_result(self) -> None:
        lcov_path = self.write("cov.lcov", lcov((SRC, {1: 1, 2: 0})))
        diff_path = self.write("changes.diff", added(SRC, 1, ["a", "b"]))
        code, _, _ = self.run_main("--lcov", lcov_path, "--diff", diff_path, "--require-changed-lines", "50")
        self.assertEqual(code, 0)
        code, _, _ = self.run_main("--lcov", lcov_path, "--diff", diff_path, "--require-changed-lines", "51")
        self.assertEqual(code, 1)

    def test_usage_and_parse_errors_exit_two(self) -> None:
        lcov_path = self.write("cov.lcov", lcov((SRC, {1: 1})))
        good_diff = self.write("changes.diff", added(SRC, 1, ["a"]))
        bad_diff = self.write("bad.diff", f"diff --git a/{SRC} b/{SRC}\n--- a/{SRC}\n+++ b/{SRC}\n@@ nope @@\n+x\n")
        quoted = self.write("quoted.diff", '+++ "b/we\\303\\257rd.rs"\n')
        cases = [
            ("--lcov", lcov_path),
            ("--lcov", lcov_path, "--diff", str(self.root / "absent.diff")),
            ("--lcov", str(self.root / "absent.lcov"), "--diff", good_diff),
            ("--lcov", lcov_path, "--diff", bad_diff),
            ("--lcov", lcov_path, "--diff", quoted),
            ("--lcov", lcov_path, "--diff", good_diff, "--require-changed-lines", "101"),
            ("--lcov", lcov_path, "--diff", good_diff, "--require-changed-lines", "many"),
        ]
        for args in cases:
            code, _, _ = self.run_main(*args)
            self.assertEqual(code, 2, args)

    def test_public_cli_exit_status(self) -> None:
        lcov_path = self.write("cov.lcov", lcov((SRC, {10: 1, 11: 0})))
        passing = self.write("pass.diff", added(SRC, 10, ["a"]))
        failing = self.write("fail.diff", added(SRC, 10, ["a", "b"]))
        env = dict(os.environ, LC_ALL="en_US.UTF-8")
        for diff, expected in ((passing, 0), (failing, 1)):
            result = subprocess.run(
                [sys.executable, "-B", str(CHECKER_PATH), "--lcov", lcov_path, "--diff", diff],
                capture_output=True,
                text=True,
                env=env,
                check=False,
            )
            self.assertEqual(result.returncode, expected, result.stdout + result.stderr)
            self.assertIn("workspace lines: 1/2 (50.00%)", result.stdout)


if __name__ == "__main__":
    unittest.main()
