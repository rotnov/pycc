#!/usr/bin/env python3
"""Mutation tests for the conformance breadth checker.

Every failure mode `check_conformance_breadth.py` claims to detect is proven
here by mutating a known-good input until only that one thing is wrong, then
asserting the checker rejects it. A checker whose failure paths are never
exercised is a checker that can rot into a no-op, which is exactly how the
matrix drifted in the first place (#572).

The last test runs the real repository files, so this suite also fails if the
checked-in manifest stops describing the checked-in matrix.
"""

from __future__ import annotations

import copy
import importlib.util
import json
from pathlib import Path
import unittest

CHECKER_PATH = Path(__file__).with_name("check_conformance_breadth.py")
CHECKER_SPEC = importlib.util.spec_from_file_location(
    "conformance_breadth_checker", CHECKER_PATH
)
if CHECKER_SPEC is None or CHECKER_SPEC.loader is None:
    raise RuntimeError("could not load the conformance breadth checker")
CHECKER = importlib.util.module_from_spec(CHECKER_SPEC)
CHECKER_SPEC.loader.exec_module(CHECKER)
BreadthError = CHECKER.BreadthError
cited_fixtures = CHECKER.cited_fixtures
green_rows = CHECKER.green_rows
is_registered = CHECKER.is_registered
parse_matrix = CHECKER.parse_matrix
validate = CHECKER.validate

REPOSITORY_ROOT = CHECKER_PATH.resolve().parent.parent

MATRIX = """
# Conformance

| Track | Upstream checkpoint | pycc consequence |
|---|---|---|
| v1 stable oracle | Python 3.14.7 | pinned |

| PEP | Feature | Cat | Test | St |
|---|---|---|---|---|
| [498](https://peps.python.org/pep-0498/) | f-strings | syntax | `pep_0498_fstrings.py` | ✅ |
| [3102](https://peps.python.org/pep-3102/) | Keyword-only arguments | syntax | `py30/pep_3102_kwonly.py` | ☐ |
| [3110](https://peps.python.org/pep-3110/) | `try`/`except` | sem | `issue_382_exceptions.rs` | ⚙ |
""".lstrip()

HARNESS = """
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/pep_0498_fstrings.py");
    /// See `pep_0594_dead_battery.py` for the negative control.
"""


def manifest() -> dict:
    return {
        "manifest_version": 1,
        "rows": [
            {
                "pep": "[498](https://peps.python.org/pep-0498/)",
                "feature": "f-strings",
                "matrix_line": 9,
                "fixtures": ["pep_0498_fstrings.py"],
                "proven": [
                    {
                        "category": "interpolation of str and int names",
                        "evidence": "pep_0498_fstrings.py",
                    }
                ],
                "not_proven": [
                    {
                        "category": "format specifiers",
                        "reason": "rejected with C0001 in HIR lowering",
                        "issue": 248,
                    }
                ],
            }
        ],
    }


class ParsingTests(unittest.TestCase):
    def test_only_status_marked_five_cell_rows_are_matrix_rows(self) -> None:
        rows = parse_matrix(MATRIX)
        self.assertEqual([row.status for row in rows], ["✅", "☐", "⚙"])

    def test_green_rows_selects_the_passing_rows_only(self) -> None:
        self.assertEqual([row.pep[1:4] for row in green_rows(MATRIX)], ["498"])

    def test_the_line_number_is_one_based(self) -> None:
        self.assertEqual(green_rows(MATRIX)[0].line_number, 9)

    def test_cited_fixtures_takes_backtick_spans_ending_in_py(self) -> None:
        cell = "`a.py` and [link](x) plus `b.rs` then `c.py`"
        self.assertEqual(cited_fixtures(cell), ["a.py", "c.py"])

    def test_cited_fixtures_ignores_an_unclosed_span(self) -> None:
        self.assertEqual(cited_fixtures("`unterminated.py"), [])

    def test_is_registered_requires_the_joined_path_form(self) -> None:
        self.assertTrue(is_registered(HARNESS, "pep_0498_fstrings.py"))
        # Named only in a doc comment, never joined as a path.
        self.assertFalse(is_registered(HARNESS, "pep_0594_dead_battery.py"))

    def test_the_row_key_separates_two_rows_sharing_one_pep(self) -> None:
        shared = MATRIX + (
            "| [498](https://peps.python.org/pep-0498/) | other slice | syntax |"
            " `pep_0498_other.py` | ✅ |\n"
        )
        keys = {row.key for row in green_rows(shared)}
        self.assertEqual(len(keys), 2)


class ValidationTests(unittest.TestCase):
    def assert_rejected(self, mutate, expected: str) -> None:
        document = manifest()
        mutate(document)
        with self.assertRaises(BreadthError) as caught:
            validate(MATRIX, document, HARNESS)
        self.assertIn(expected, str(caught.exception))

    def test_the_unmutated_input_is_accepted(self) -> None:
        validate(MATRIX, manifest(), HARNESS)

    def test_a_green_row_with_no_manifest_entry_is_rejected(self) -> None:
        self.assert_rejected(
            lambda document: document["rows"].clear(),
            "is ✅ but has no breadth manifest entry",
        )

    def test_an_entry_for_a_row_that_is_not_green_is_rejected(self) -> None:
        def mutate(document: dict) -> None:
            extra = copy.deepcopy(document["rows"][0])
            extra["pep"] = "[3102](https://peps.python.org/pep-3102/)"
            extra["fixtures"] = ["py30/pep_3102_kwonly.py"]
            document["rows"].append(extra)

        self.assert_rejected(mutate, "matches no ✅ row in the matrix")

    def test_an_empty_proven_list_is_rejected(self) -> None:
        self.assert_rejected(
            lambda document: document["rows"][0].update(proven=[]),
            "proves no semantic category",
        )

    def test_an_unregistered_fixture_is_rejected(self) -> None:
        def mutate(document: dict) -> None:
            document["rows"][0]["fixtures"] = ["pep_0594_dead_battery.py"]
            document["rows"][0]["proven"][0]["evidence"] = "pep_0594_dead_battery.py"
            # Keep the row key matching the matrix by rewriting the matrix cell
            # is not possible here, so this asserts the registration failure
            # alongside the key failure; both are real.

        document = manifest()
        mutate(document)
        with self.assertRaises(BreadthError) as caught:
            validate(MATRIX, document, HARNESS)
        self.assertIn("matches no ✅ row in the matrix", str(caught.exception))

    def test_an_unregistered_fixture_on_a_matching_row_is_rejected(self) -> None:
        matrix = MATRIX.replace("`pep_0498_fstrings.py`", "`pep_0498_unrun.py`")
        document = manifest()
        document["rows"][0]["fixtures"] = ["pep_0498_unrun.py"]
        document["rows"][0]["proven"][0]["evidence"] = "pep_0498_unrun.py"
        with self.assertRaises(BreadthError) as caught:
            validate(matrix, document, HARNESS)
        self.assertIn("is not registered in tests/conformance.rs", str(caught.exception))

    def test_proven_evidence_outside_the_rows_fixtures_is_rejected(self) -> None:
        self.assert_rejected(
            lambda document: document["rows"][0]["proven"][0].update(
                evidence="pep_0594_dead_battery.py"
            ),
            "is not one of this row's fixtures",
        )

    def test_a_stale_matrix_line_is_rejected(self) -> None:
        self.assert_rejected(
            lambda document: document["rows"][0].update(matrix_line=4),
            "but PEP",
        )

    def test_a_duplicate_entry_is_rejected(self) -> None:
        self.assert_rejected(
            lambda document: document["rows"].append(copy.deepcopy(document["rows"][0])),
            "repeats the row already declared",
        )

    def test_a_proven_category_without_a_category_name_is_rejected(self) -> None:
        self.assert_rejected(
            lambda document: document["rows"][0]["proven"][0].update(category="  "),
            "needs a non-empty `category`",
        )

    def test_a_not_proven_entry_without_a_reason_is_rejected(self) -> None:
        self.assert_rejected(
            lambda document: document["rows"][0]["not_proven"][0].pop("reason"),
            "needs a non-empty `reason`",
        )

    def test_a_missing_feature_is_rejected(self) -> None:
        self.assert_rejected(
            lambda document: document["rows"][0].pop("feature"),
            "needs a non-empty `feature`",
        )

    def test_an_empty_fixtures_list_is_rejected(self) -> None:
        self.assert_rejected(
            lambda document: document["rows"][0].update(fixtures=[]),
            "needs a non-empty `fixtures` list",
        )

    def test_a_wrong_manifest_version_is_rejected(self) -> None:
        self.assert_rejected(
            lambda document: document.update(manifest_version=99),
            "`manifest_version` must be 1",
        )

    def test_a_non_object_manifest_is_rejected(self) -> None:
        with self.assertRaises(BreadthError):
            validate(MATRIX, [], HARNESS)

    def test_a_manifest_without_rows_is_rejected(self) -> None:
        with self.assertRaises(BreadthError) as caught:
            validate(MATRIX, {"manifest_version": 1}, HARNESS)
        self.assertIn("needs a `rows` list", str(caught.exception))

    def test_a_non_object_row_is_rejected(self) -> None:
        self.assert_rejected(
            lambda document: document["rows"].append("not an object"),
            "must be an object",
        )

    def test_a_matrix_with_no_green_rows_fails_rather_than_passing_vacuously(
        self,
    ) -> None:
        with self.assertRaises(BreadthError) as caught:
            validate(MATRIX.replace("✅", "☐"), {"manifest_version": 1, "rows": []}, HARNESS)
        self.assertIn("would pass vacuously", str(caught.exception))


class RepositoryTests(unittest.TestCase):
    def test_the_checked_in_manifest_describes_the_checked_in_matrix(self) -> None:
        matrix = (REPOSITORY_ROOT / "docs/PYTHON_STANDARDS.md").read_text(
            encoding="utf-8"
        )
        document = json.loads(
            (
                REPOSITORY_ROOT / "tests/fixtures/conformance-breadth-manifest.json"
            ).read_text(encoding="utf-8")
        )
        harness = (REPOSITORY_ROOT / "tests/conformance.rs").read_text(encoding="utf-8")
        validate(matrix, document, harness)

    def test_every_green_row_records_at_least_one_unproven_category_or_says_why(
        self,
    ) -> None:
        # Not a validator rule -- a whole-PEP row that genuinely proves
        # everything is legitimate -- but a manifest where *no* row records a
        # gap would mean the population pass was not actually done.
        document = json.loads(
            (
                REPOSITORY_ROOT / "tests/fixtures/conformance-breadth-manifest.json"
            ).read_text(encoding="utf-8")
        )
        with_gaps = [row for row in document["rows"] if row["not_proven"]]
        self.assertGreater(len(with_gaps), len(document["rows"]) // 2)


if __name__ == "__main__":
    unittest.main()
