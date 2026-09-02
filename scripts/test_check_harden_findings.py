#!/usr/bin/env python3
"""Mutation tests for scripts/check_harden_findings.py.

Every defect the checker claims to detect is proven against a constructed
git checkout in a temporary directory, in both directions: the violation
is rejected, and the clean pile is accepted. The last test runs the real
repository's own piles through the checker so the suite also fails if a
checked-in pile stops conforming.
"""

from __future__ import annotations

import importlib.util
import json
import subprocess
import tempfile
import unittest
from pathlib import Path

CHECKER_PATH = Path(__file__).with_name("check_harden_findings.py")
CHECKER_SPEC = importlib.util.spec_from_file_location("harden_findings_checker", CHECKER_PATH)
if CHECKER_SPEC is None or CHECKER_SPEC.loader is None:
    raise RuntimeError("could not load scripts/check_harden_findings.py")
CHECKER = importlib.util.module_from_spec(CHECKER_SPEC)
CHECKER_SPEC.loader.exec_module(CHECKER)

GOOD = {
    "round": 1,
    "file": "src/x.rs",
    "line": 4,
    "category": "doc-drift",
    "summary": "comment overstates",
    "disposition": "fixed",
    "note": "",
}
REFUTED = dict(GOOD, disposition="refuted", note="the guard is exercised by test y")


def git(root: Path, *args: str) -> None:
    subprocess.run(["git", "-C", str(root), *args], check=True, capture_output=True)


class FindingsCheckerTests(unittest.TestCase):
    def setUp(self) -> None:
        self.tmp = tempfile.TemporaryDirectory()
        self.root = Path(self.tmp.name)
        git(self.root, "init", "-q")
        self.pile = self.root / ".harden" / "findings" / "issue-1.jsonl"
        self.pile.parent.mkdir(parents=True)

    def tearDown(self) -> None:
        self.tmp.cleanup()

    def write(self, rows: list[object], track: bool = True) -> None:
        self.pile.write_text(
            "".join(json.dumps(r) + "\n" if not isinstance(r, str) else r + "\n" for r in rows),
            encoding="utf-8",
        )
        if track:
            git(self.root, "add", "-f", "--", str(self.pile))

    def test_clean_tracked_pile_passes(self) -> None:
        self.write([GOOD, REFUTED])
        self.assertEqual(CHECKER.validate([self.pile], self.root), [])
        self.assertEqual(CHECKER.main([str(self.pile), "--repo-root", str(self.root)]), 0)

    def test_untracked_pile_is_rejected_even_when_well_formed(self) -> None:
        self.write([GOOD], track=False)
        problems = CHECKER.validate([self.pile], self.root)
        self.assertEqual(len(problems), 1)
        self.assertIn("not tracked by git", problems[0])

    def test_excluded_pile_is_reported_as_untracked(self) -> None:
        (self.root / ".git" / "info").mkdir(exist_ok=True)
        (self.root / ".git" / "info" / "exclude").write_text(".harden/\n", encoding="utf-8")
        self.write([GOOD], track=False)
        git(self.root, "add", "-A")  # honours the exclude, exactly the observed failure
        problems = CHECKER.validate([self.pile], self.root)
        self.assertTrue(any("not tracked by git" in p for p in problems), problems)

    def test_missing_file_is_rejected(self) -> None:
        problems = CHECKER.validate([self.pile], self.root)
        self.assertEqual(problems, [f"{self.pile}: no such file"])

    def test_empty_pile_is_rejected(self) -> None:
        self.write([])
        self.assertIn(f"{self.pile}: pile is empty", CHECKER.validate([self.pile], self.root))

    def test_invalid_json_line_is_rejected(self) -> None:
        self.write([GOOD, "{not json"])
        problems = CHECKER.validate([self.pile], self.root)
        self.assertEqual(len(problems), 1)
        self.assertIn(":2: not valid JSON", problems[0])

    def test_non_object_line_is_rejected(self) -> None:
        self.write([GOOD, [1, 2]])
        self.assertIn(f"{self.pile}:2: line is not a JSON object", CHECKER.validate([self.pile], self.root))

    def test_missing_keys_are_named(self) -> None:
        row = {k: v for k, v in GOOD.items() if k not in ("category", "note")}
        self.write([row])
        problems = CHECKER.validate([self.pile], self.root)
        self.assertEqual(problems, [f"{self.pile}:1: missing key(s) category, note"])

    def test_unknown_disposition_is_rejected(self) -> None:
        self.write([dict(GOOD, disposition="deferred")])
        problems = CHECKER.validate([self.pile], self.root)
        self.assertEqual(len(problems), 1)
        self.assertIn("disposition 'deferred' is not one of fixed, refuted", problems[0])

    def test_refuted_without_note_is_rejected_but_fixed_without_note_passes(self) -> None:
        self.write([dict(REFUTED, note="  ")])
        self.assertEqual(
            CHECKER.validate([self.pile], self.root),
            [f"{self.pile}:1: refuted finding has an empty note"],
        )
        self.write([GOOD])
        self.assertEqual(CHECKER.validate([self.pile], self.root), [])

    def test_clean_round_marker_is_accepted_only_with_its_category(self) -> None:
        marker = dict(GOOD, disposition="clean", category="clean-round", note="round 3 clean")
        self.write([GOOD, marker])
        self.assertEqual(CHECKER.validate([self.pile], self.root), [])
        self.write([dict(GOOD, disposition="clean")])
        problems = CHECKER.validate([self.pile], self.root)
        self.assertEqual(len(problems), 1)
        self.assertIn("disposition 'clean' is not one of fixed, refuted", problems[0])

    def test_main_reports_each_problem_and_exits_one(self) -> None:
        self.write([dict(GOOD, disposition="maybe")], track=False)
        self.assertEqual(CHECKER.main([str(self.pile), "--repo-root", str(self.root)]), 1)

    def test_main_rejects_a_non_checkout_root(self) -> None:
        with tempfile.TemporaryDirectory() as plain:
            self.assertEqual(CHECKER.main([str(self.pile), "--repo-root", plain]), 2)

    def test_legacy_pile_skips_the_schema_check_but_not_the_tracking_check(self) -> None:
        legacy = self.pile.with_name("issue-197.jsonl")
        legacy.write_text(json.dumps({"round": 1, "status": "fixed"}) + "\n", encoding="utf-8")
        self.assertEqual(CHECKER.validate([legacy], self.root), [f"{legacy}: not tracked by git -- stage it with `git add -f` (a machine-local exclude can hide a tracked directory from `git add -A`)"])
        git(self.root, "add", "-f", "--", str(legacy))
        self.assertEqual(CHECKER.validate([legacy], self.root), [])
        self.assertIn(legacy.name, CHECKER.LEGACY_SCHEMA_PILES)

    def test_legacy_snapshot_names_only_piles_that_exist(self) -> None:
        repo = Path(__file__).resolve().parent.parent
        for name in CHECKER.LEGACY_SCHEMA_PILES:
            self.assertTrue((repo / ".harden" / "findings" / name).is_file(), name)

    def test_real_repository_piles_conform(self) -> None:
        repo = Path(__file__).resolve().parent.parent
        piles = sorted((repo / ".harden" / "findings").glob("*.jsonl"))
        self.assertTrue(piles, "expected at least one checked-in findings pile")
        self.assertEqual(CHECKER.validate(piles, repo), [])


if __name__ == "__main__":
    unittest.main()
