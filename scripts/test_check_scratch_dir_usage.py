#!/usr/bin/env python3
"""Mutation tests for scripts/check_scratch_dir_usage.py (issue #781).

Every failure mode the checker claims to detect is proven here against a
constructed fixture tree, not the real repository, so this suite still means
something once `ALLOWLIST` empties out as Parts 2/3 migrate real files. The
last test runs the real repository files through the real `ALLOWLIST`, so
this suite also fails if the checked-in allowlist stops describing the
checked-in tree.
"""

from __future__ import annotations

import importlib.util
import tempfile
import unittest
from pathlib import Path

CHECKER_PATH = Path(__file__).with_name("check_scratch_dir_usage.py")
CHECKER_SPEC = importlib.util.spec_from_file_location(
    "scratch_dir_usage_checker", CHECKER_PATH
)
if CHECKER_SPEC is None or CHECKER_SPEC.loader is None:
    raise RuntimeError("could not load scripts/check_scratch_dir_usage.py")
CHECKER = importlib.util.module_from_spec(CHECKER_SPEC)
CHECKER_SPEC.loader.exec_module(CHECKER)

find_violations = CHECKER.find_violations
validate = CHECKER.validate
occurrence_count = CHECKER.occurrence_count
tracked_rust_files = CHECKER.tracked_rust_files
ScratchDirUsageError = CHECKER.ScratchDirUsageError
EXEMPT_FILES = CHECKER.EXEMPT_FILES
ALLOWLIST = CHECKER.ALLOWLIST
REPOSITORY_ROOT = CHECKER_PATH.resolve().parent.parent

CLEAN_FILE = """
fn helper() -> std::path::PathBuf {
    // no banned scratch-directory call pattern here
    std::path::PathBuf::from("/tmp/example")
}
"""

UNLISTED_VIOLATION_FILE = """
fn scratch() -> std::path::PathBuf {
    std::env::temp_dir().join("pycc_example_1")
}
"""

AT_PARITY_FILE = """
fn scratch_one() -> std::path::PathBuf {
    std::env::temp_dir().join("pycc_example_1")
}

fn scratch_two() -> std::path::PathBuf {
    std::env::temp_dir()
        .join("pycc_example_2")
}
"""

GREW_PAST_ALLOWANCE_FILE = AT_PARITY_FILE + """

fn scratch_three() -> std::path::PathBuf {
    std::env::temp_dir().join("pycc_example_3")
}
"""

ALIAS_EVASION_FILE = """
use std::env::temp_dir as get_scratch_root;

fn scratch() -> std::path::PathBuf {
    get_scratch_root().join("pycc_example_alias")
}
"""

SHORT_ALIAS_EVASION_FILE = """
use env::temp_dir as get_scratch_root;

fn scratch() -> std::path::PathBuf {
    get_scratch_root().join("pycc_example_alias")
}
"""


class CheckScratchDirUsageTest(unittest.TestCase):
    def setUp(self) -> None:
        self._tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self._tmp.cleanup)
        self.root = Path(self._tmp.name)

    def write(self, relative_path: str, content: str) -> None:
        full_path = self.root / relative_path
        full_path.parent.mkdir(parents=True, exist_ok=True)
        full_path.write_text(content, encoding="utf-8")

    def test_occurrence_count_matches_expected_pattern_hits(self) -> None:
        self.write("clean.rs", CLEAN_FILE)
        self.write("violation.rs", UNLISTED_VIOLATION_FILE)
        self.write("parity.rs", AT_PARITY_FILE)
        self.assertEqual(occurrence_count(self.root / "clean.rs"), 0)
        self.assertEqual(occurrence_count(self.root / "violation.rs"), 1)
        # AT_PARITY_FILE has two occurrences: one on a single line, one split
        # across two lines -- proving the whitespace-tolerant regex matches
        # both shapes.
        self.assertEqual(occurrence_count(self.root / "parity.rs"), 2)

    def test_clean_file_with_no_occurrences_is_accepted(self) -> None:
        self.write("clean.rs", CLEAN_FILE)
        violations = find_violations(["clean.rs"], allowlist={}, root=self.root)
        self.assertEqual(violations, {})
        validate(["clean.rs"], allowlist={}, root=self.root)  # must not raise

    def test_unlisted_file_with_a_new_violation_is_rejected(self) -> None:
        self.write("violation.rs", UNLISTED_VIOLATION_FILE)
        violations = find_violations(["violation.rs"], allowlist={}, root=self.root)
        self.assertEqual(violations, {"violation.rs": (1, 0)})
        with self.assertRaises(ScratchDirUsageError):
            validate(["violation.rs"], allowlist={}, root=self.root)

    def test_file_whose_count_matches_its_allowlist_entry_exactly_is_accepted(
        self,
    ) -> None:
        self.write("parity.rs", AT_PARITY_FILE)
        violations = find_violations(
            ["parity.rs"], allowlist={"parity.rs": 2}, root=self.root
        )
        self.assertEqual(violations, {})
        validate(
            ["parity.rs"], allowlist={"parity.rs": 2}, root=self.root
        )  # must not raise

    def test_file_whose_count_is_below_its_allowlist_entry_is_accepted(self) -> None:
        # A file that migrated some (not all) of its occurrences off the
        # banned pattern -- fewer than its recorded allowance -- must still
        # pass, since the allowlist is a ceiling, not an exact target.
        self.write("parity.rs", AT_PARITY_FILE)
        violations = find_violations(
            ["parity.rs"], allowlist={"parity.rs": 5}, root=self.root
        )
        self.assertEqual(violations, {})

    def test_already_listed_file_that_grew_past_its_allowance_is_rejected(
        self,
    ) -> None:
        self.write("grew.rs", GREW_PAST_ALLOWANCE_FILE)
        violations = find_violations(
            ["grew.rs"], allowlist={"grew.rs": 2}, root=self.root
        )
        self.assertEqual(violations, {"grew.rs": (3, 2)})
        with self.assertRaises(ScratchDirUsageError):
            validate(["grew.rs"], allowlist={"grew.rs": 2}, root=self.root)

    def test_the_pycc_scratch_implementation_file_is_exempt_unconditionally(
        self,
    ) -> None:
        exempt_relative_path = next(iter(EXEMPT_FILES))
        self.write(exempt_relative_path, UNLISTED_VIOLATION_FILE)
        violations = find_violations(
            [exempt_relative_path], allowlist={}, root=self.root
        )
        self.assertEqual(violations, {})

    def test_import_alias_evasion_is_detected(self) -> None:
        # `use std::env::temp_dir as get_scratch_root;` followed by
        # `get_scratch_root().join(...)` evades PATTERN entirely (the call
        # site no longer contains the literal `temp_dir().join(` text), but
        # is ordinary, idiomatic Rust rather than a deliberate evasion.
        # ALIAS_PATTERN flags the `use ... as` import line itself.
        self.write("alias.rs", ALIAS_EVASION_FILE)
        self.assertEqual(occurrence_count(self.root / "alias.rs"), 1)
        violations = find_violations(["alias.rs"], allowlist={}, root=self.root)
        self.assertEqual(violations, {"alias.rs": (1, 0)})
        with self.assertRaises(ScratchDirUsageError):
            validate(["alias.rs"], allowlist={}, root=self.root)

    def test_import_alias_evasion_via_unqualified_env_path_is_detected(self) -> None:
        # A caller with `env` already in scope (`use std::env;`) can alias
        # via the shorter `use env::temp_dir as ...` path -- still an evasion
        # of PATTERN, still caught by ALIAS_PATTERN.
        self.write("alias_short.rs", SHORT_ALIAS_EVASION_FILE)
        self.assertEqual(occurrence_count(self.root / "alias_short.rs"), 1)

    def test_allowlist_values_never_exceed_the_real_tracked_trees_current_count(
        self,
    ) -> None:
        # The allowlist ratchet (module docstring) requires that a listed
        # file's recorded count only ever go down over time, never up --
        # migrating occurrences off the banned pattern lowers the real
        # count, and the checked-in ALLOWLIST value must follow it down.
        # `find_violations`/`validate` alone cannot catch a future PR that
        # pads a file's ALLOWLIST number upward without adding any matching
        # real occurrences, because `found <= allowed` still holds in that
        # case. This test instead compares each ALLOWLIST value directly
        # against the real tree's current occurrence count for that file,
        # so a pad-without-justification shows up as `allowed > found`.
        offenders = {}
        for relative_path, allowed in ALLOWLIST.items():
            found = occurrence_count(REPOSITORY_ROOT / relative_path)
            if allowed > found:
                offenders[relative_path] = (allowed, found)
        self.assertEqual(
            offenders,
            {},
            "ALLOWLIST records more occurrences than the file actually "
            "contains -- lower the recorded count(s) to match reality: "
            f"{offenders}",
        )

    def test_every_allowlisted_file_still_exists_in_the_tracked_tree(self) -> None:
        # A file that is deleted or renamed without also removing/updating
        # its ALLOWLIST entry leaves a stale key behind that
        # `tracked_rust_files()` (which only returns files that still exist)
        # never re-validates again.
        tracked = set(tracked_rust_files(REPOSITORY_ROOT))
        stale = sorted(set(ALLOWLIST) - tracked)
        self.assertEqual(
            stale,
            [],
            f"ALLOWLIST references file(s) no longer tracked: {stale}",
        )

    def test_the_real_repository_tree_passes_its_own_checked_in_allowlist(
        self,
    ) -> None:
        files = tracked_rust_files(REPOSITORY_ROOT)
        self.assertIn("crates/pycc_scratch/src/lib.rs", files)
        validate(files, ALLOWLIST, root=REPOSITORY_ROOT)  # must not raise


if __name__ == "__main__":
    unittest.main()
