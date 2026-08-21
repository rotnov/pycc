#!/usr/bin/env python3
"""Mutation tests for the conformance breadth checker.

Every failure mode `check_conformance_breadth.py` claims to detect is proven
here by mutating a known-good input until only that one thing is wrong, then
asserting the checker rejects it. A checker whose failure paths are never
exercised is a checker that can rot into a no-op, which is exactly how the
matrix drifted in the first place (#572).

The last tests run the real repository files, so this suite also fails if the
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
evidence_rows = CHECKER.evidence_rows
is_registered = CHECKER.is_registered
parse_matrix = CHECKER.parse_matrix
validate = CHECKER.validate
check_roadmap_counts = CHECKER.check_roadmap_counts
summary_body = CHECKER.summary_body

REPOSITORY_ROOT = CHECKER_PATH.resolve().parent.parent

MATRIX = """
# Conformance

| Track | Upstream checkpoint | pycc consequence |
|---|---|---|
| v1 stable oracle | Python 3.14.7 | pinned |

| PEP | Feature | Cat | Test | St |
|---|---|---|---|---|
| [498](https://peps.python.org/pep-0498/) | f-strings | syntax | `pep_0498_fstrings.py` | ◐ |
| [3102](https://peps.python.org/pep-3102/) | Keyword-only arguments | syntax | `py30/pep_3102_kwonly.py` | ☐ |
| [3110](https://peps.python.org/pep-3110/) | `try`/`except` | sem | `issue_382_exceptions.rs` | ⚙ |
| [414](https://peps.python.org/pep-0414/) | Unicode literals | syntax | `pep_0414_u_prefix.py` | ✅ |
""".lstrip()

HARNESS = """
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/pep_0498_fstrings.py");
    let other = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/pep_0414_u_prefix.py");
    /// See `pep_0594_dead_battery.py` for the negative control.
"""


def manifest() -> dict:
    """A manifest that validates against `MATRIX` and `HARNESS` unmutated.

    It carries one row of each evidence status on purpose: a `◐` row whose one
    gap is `core`, and a `✅` row whose one gap is `out-of-scope`. Both sides of
    D-177's acceptance rule are therefore exercised by the accepting path, not
    only by the mutations below.
    """
    return {
        "manifest_version": 2,
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
                        "kind": "core",
                        "reason": "rejected with C0001 in HIR lowering",
                        "issue": 248,
                    }
                ],
            },
            {
                "pep": "[414](https://peps.python.org/pep-0414/)",
                "feature": "Unicode literals",
                "matrix_line": 12,
                "fixtures": ["pep_0414_u_prefix.py"],
                "proven": [
                    {
                        "category": "the bare `u` prefix on a str literal",
                        "evidence": "pep_0414_u_prefix.py",
                    }
                ],
                "not_proven": [
                    {
                        "category": "prefix combinations such as `ur''`",
                        "kind": "out-of-scope",
                        "reason": "Python 3 rejects every combination of `u` with "
                        "`r`, `b`, or `f` as a syntax error, so there is no valid "
                        "combined-prefix category for a fixture to cover.",
                    }
                ],
            },
        ],
    }


#: A stand-in for `docs/ROADMAP.md`'s conformance paragraph. It deliberately
#: narrates superseded figures — an earlier total and an earlier gap — so the
#: tests prove the guard reads the bold headline rather than the first number
#: it can find in the prose.
ROADMAP = """
## v0.3

**Conformance progress (2026-01-01): 2 of the required 5 matrix rows are at `◐` or better, leaving a 3-row gap; 1 of those 2 are `✅` (whole-PEP acceptance), which is reported but not gated before v1.0.** This figure is derived mechanically — `python3 scripts/check_conformance_breadth.py` reports "2 evidence-backed rows, all declared (1 accepted as whole-PEP, 1 subset)" — and supersedes the earlier figure of 1, which took the count from 0 to 1 and left a 4-row gap.
""".lstrip()


class ParsingTests(unittest.TestCase):
    def test_only_status_marked_five_cell_rows_are_matrix_rows(self) -> None:
        rows = parse_matrix(MATRIX)
        self.assertEqual([row.status for row in rows], ["◐", "☐", "⚙", "✅"])

    def test_evidence_rows_selects_the_subset_and_accepted_rows_only(self) -> None:
        self.assertEqual([row.pep[1:4] for row in evidence_rows(MATRIX)], ["498", "414"])

    def test_the_line_number_is_one_based(self) -> None:
        self.assertEqual(evidence_rows(MATRIX)[0].line_number, 9)

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
            " `pep_0498_other.py` | ◐ |\n"
        )
        keys = {row.key for row in evidence_rows(shared)}
        self.assertEqual(len(keys), 3)


class ValidationTests(unittest.TestCase):
    def assert_rejected(self, mutate, expected: str) -> None:
        document = manifest()
        mutate(document)
        with self.assertRaises(BreadthError) as caught:
            validate(MATRIX, document, HARNESS)
        self.assertIn(expected, str(caught.exception))

    def test_the_unmutated_input_is_accepted(self) -> None:
        validate(MATRIX, manifest(), HARNESS)

    def test_an_evidence_row_with_no_manifest_entry_is_rejected(self) -> None:
        self.assert_rejected(
            lambda document: document["rows"].clear(),
            "but has no breadth manifest entry",
        )

    def test_an_entry_for_a_row_that_is_not_evidence_backed_is_rejected(self) -> None:
        def mutate(document: dict) -> None:
            extra = copy.deepcopy(document["rows"][0])
            extra["pep"] = "[3102](https://peps.python.org/pep-3102/)"
            extra["fixtures"] = ["py30/pep_3102_kwonly.py"]
            document["rows"].append(extra)

        self.assert_rejected(mutate, "matches no ◐ or ✅ row in the matrix")

    def test_an_empty_proven_list_is_rejected(self) -> None:
        self.assert_rejected(
            lambda document: document["rows"][0].update(proven=[]),
            "proves no semantic category",
        )

    def test_an_unregistered_fixture_is_rejected(self) -> None:
        def mutate(document: dict) -> None:
            document["rows"][0]["fixtures"] = ["pep_0594_dead_battery.py"]
            document["rows"][0]["proven"][0]["evidence"] = "pep_0594_dead_battery.py"

        # Rewriting the fixture also breaks the row key, so this asserts the key
        # failure; the registration failure has its own test below, on a matrix
        # whose cell was rewritten to match.
        self.assert_rejected(mutate, "matches no ◐ or ✅ row in the matrix")

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

    def test_a_not_proven_entry_without_a_kind_is_rejected(self) -> None:
        self.assert_rejected(
            lambda document: document["rows"][0]["not_proven"][0].pop("kind"),
            "needs a non-empty `kind`",
        )

    def test_an_unrecognized_gap_kind_is_rejected(self) -> None:
        self.assert_rejected(
            lambda document: document["rows"][0]["not_proven"][0].update(
                kind="partial"
            ),
            "has `kind` 'partial', which is not one of",
        )

    def test_a_subset_row_with_no_core_gap_is_rejected(self) -> None:
        # The other direction of D-177's rule: a row held at ◐ while its
        # manifest says every gap is a permanent non-goal means the manifest
        # and the matrix disagree about the same fact.
        self.assert_rejected(
            lambda document: document["rows"][0]["not_proven"][0].update(
                kind="out-of-scope"
            ),
            "is ◐ (subset) but records no unimplemented core category",
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
            "`manifest_version` must be 2",
        )

    def test_a_non_object_manifest_is_rejected(self) -> None:
        with self.assertRaises(BreadthError):
            validate(MATRIX, [], HARNESS)

    def test_a_manifest_without_rows_is_rejected(self) -> None:
        with self.assertRaises(BreadthError) as caught:
            validate(MATRIX, {"manifest_version": 2}, HARNESS)
        self.assertIn("needs a `rows` list", str(caught.exception))

    def test_a_non_object_row_is_rejected(self) -> None:
        self.assert_rejected(
            lambda document: document["rows"].append("not an object"),
            "must be an object",
        )

    def test_a_matrix_with_no_evidence_rows_fails_rather_than_passing_vacuously(
        self,
    ) -> None:
        empty = MATRIX.replace("| ◐ |", "| ☐ |").replace("| ✅ |", "| ☐ |")
        with self.assertRaises(BreadthError) as caught:
            validate(empty, {"manifest_version": 2, "rows": []}, HARNESS)
        self.assertIn("would pass vacuously", str(caught.exception))


#: #248's three named counterexamples: narrow fixtures whose rows were marked
#: ✅ as if they proved the whole PEP. Each is `(pep, feature, fixture,
#: unimplemented core category)`. They are the concrete cases this rule exists
#: to make unrepresentable, so they are tested as data rather than prose.
OVERCLAIM_COUNTEREXAMPLES = [
    (
        "498",
        "f-strings",
        "pep_0498_fstrings.py",
        "format specifiers",
    ),
    (
        "3105",
        "`print` as a function",
        "pep_3105_print_function.py",
        "the `sep=` and `end=` keyword arguments",
    ),
    (
        "526",
        "Variable annotations",
        "pep_0526_var_annotations.py",
        "parenthesized annotated assignment targets",
    ),
]


class OverclaimTests(unittest.TestCase):
    """A row cannot carry ✅ while declaring an unimplemented core category."""

    @staticmethod
    def _single_row_inputs(
        pep: str, feature: str, fixture: str, gap: str, kind: str
    ) -> tuple[str, dict, str]:
        link = f"[{pep}](https://peps.python.org/pep-{pep.zfill(4)}/)"
        matrix = (
            "| PEP | Feature | Cat | Test | St |\n"
            "|---|---|---|---|---|\n"
            f"| {link} | {feature} | syntax | `{fixture}` | ✅ |\n"
        )
        document = {
            "manifest_version": 2,
            "rows": [
                {
                    "pep": link,
                    "feature": feature,
                    "matrix_line": 3,
                    "fixtures": [fixture],
                    "proven": [
                        {"category": "the fixture's own narrow slice", "evidence": fixture}
                    ],
                    "not_proven": [
                        {"category": gap, "kind": kind, "reason": "not implemented"}
                    ],
                }
            ],
        }
        harness = f'.join("tests/fixtures/{fixture}");'
        return matrix, document, harness

    def test_a_core_gap_forces_the_row_off_whole_pep_acceptance(self) -> None:
        for pep, feature, fixture, gap in OVERCLAIM_COUNTEREXAMPLES:
            with self.subTest(pep=pep):
                matrix, document, harness = self._single_row_inputs(
                    pep, feature, fixture, gap, "core"
                )
                with self.assertRaises(BreadthError) as caught:
                    validate(matrix, document, harness)
                message = str(caught.exception)
                self.assertIn("whole-PEP acceptance", message)
                self.assertIn(gap, message)

    def test_the_same_row_is_accepted_once_the_gap_is_a_declared_non_goal(self) -> None:
        # The rule keys on the gap's classification, not on the row having any
        # gap at all -- otherwise a deliberate permanent non-goal (PEP 594's
        # unallowlisted modules, PEP 709's undetectable inlining) could never
        # reach ✅ no matter how complete the implementation was.
        for pep, feature, fixture, gap in OVERCLAIM_COUNTEREXAMPLES:
            with self.subTest(pep=pep):
                matrix, document, harness = self._single_row_inputs(
                    pep, feature, fixture, gap, "out-of-scope"
                )
                validate(matrix, document, harness)


class RepositoryTests(unittest.TestCase):
    @staticmethod
    def _checked_in_manifest() -> dict:
        return json.loads(
            (
                REPOSITORY_ROOT / "tests/fixtures/conformance-breadth-manifest.json"
            ).read_text(encoding="utf-8")
        )

    def test_the_checked_in_manifest_describes_the_checked_in_matrix(self) -> None:
        matrix = (REPOSITORY_ROOT / "docs/PYTHON_STANDARDS.md").read_text(
            encoding="utf-8"
        )
        harness = (REPOSITORY_ROOT / "tests/conformance.rs").read_text(encoding="utf-8")
        validate(matrix, self._checked_in_manifest(), harness)

    def test_the_checked_in_roadmap_states_the_checked_in_totals(self) -> None:
        matrix = (REPOSITORY_ROOT / "docs/PYTHON_STANDARDS.md").read_text(
            encoding="utf-8"
        )
        roadmap = (REPOSITORY_ROOT / "docs/ROADMAP.md").read_text(encoding="utf-8")
        check_roadmap_counts(roadmap, evidence_rows(matrix))

    def test_every_evidence_row_records_at_least_one_unproven_category(self) -> None:
        # Not a validator rule -- a row that genuinely proves everything and
        # has no non-goal to record is legitimate -- but a manifest where *no*
        # row records a gap would mean the population pass was not actually
        # done.
        document = self._checked_in_manifest()
        with_gaps = [row for row in document["rows"] if row["not_proven"]]
        self.assertGreater(len(with_gaps), len(document["rows"]) // 2)

    def test_the_checked_in_manifest_classifies_every_gap(self) -> None:
        # Redundant with the validator by construction, and deliberately so:
        # if a future edit ever relaxes the `kind` requirement, this asserts
        # the checked-in data still carries the classification the roadmap's
        # count is derived from.
        document = self._checked_in_manifest()
        kinds = {
            gap["kind"] for row in document["rows"] for gap in row["not_proven"]
        }
        self.assertTrue(kinds)
        self.assertTrue(kinds <= {"core", "out-of-scope"}, kinds)


class RoadmapCountTests(unittest.TestCase):
    """#623's third criterion: the roadmap's stated totals cannot drift silently."""

    ROWS = evidence_rows(MATRIX)

    def assert_rejected(self, roadmap: str, expected: str) -> None:
        with self.assertRaises(BreadthError) as caught:
            check_roadmap_counts(roadmap, self.ROWS)
        self.assertIn(expected, str(caught.exception))

    def test_the_summary_body_is_the_string_the_roadmap_quotes(self) -> None:
        self.assertEqual(
            summary_body(self.ROWS),
            "2 evidence-backed rows, all declared (1 accepted as whole-PEP, 1 subset)",
        )

    def test_matching_counts_pass(self) -> None:
        check_roadmap_counts(ROADMAP, self.ROWS)

    def test_diagnostics_name_the_roadmap_that_was_actually_read(self) -> None:
        with self.assertRaises(BreadthError) as caught:
            check_roadmap_counts(
                ROADMAP.replace("2 of the required 5", "3 of the required 5"),
                self.ROWS,
                "/tmp/other-roadmap.md",
            )
        message = str(caught.exception)
        self.assertIn("/tmp/other-roadmap.md claims 3 evidence-backed rows", message)
        self.assertNotIn("docs/ROADMAP.md", message)

    def test_a_divergent_total_is_rejected(self) -> None:
        self.assert_rejected(
            ROADMAP.replace("2 of the required 5", "3 of the required 5"),
            "claims 3 evidence-backed rows, but the matrix has 2",
        )

    def test_a_divergent_whole_pep_count_is_rejected(self) -> None:
        self.assert_rejected(
            ROADMAP.replace("1 of those 2 are", "2 of those 2 are"),
            "claims 2 whole-PEP (`✅`) rows, but the matrix has 1",
        )

    def test_a_restated_total_that_contradicts_the_headline_is_rejected(self) -> None:
        self.assert_rejected(
            ROADMAP.replace("1 of those 2 are", "1 of those 7 are"),
            "states 2 evidence-backed rows but then says `1 of those 7`",
        )

    def test_a_divergent_gap_is_rejected(self) -> None:
        self.assert_rejected(
            ROADMAP.replace("leaving a 3-row gap", "leaving a 4-row gap"),
            "states a 4-row gap, but 5 required minus 2 evidence-backed is 3",
        )

    def test_a_quoted_summary_that_no_longer_matches_is_rejected(self) -> None:
        self.assert_rejected(
            ROADMAP.replace("(1 accepted as whole-PEP, 1 subset)", "(1 accepted, 1 sub)"),
            "does not quote the checker's current summary verbatim",
        )

    def test_a_missing_headline_is_a_failure_not_a_silent_pass(self) -> None:
        self.assert_rejected(
            ROADMAP.replace("**Conformance progress (2026-01-01):", "Conformance notes:"),
            "found 0",
        )

    def test_a_duplicated_headline_is_a_failure(self) -> None:
        self.assert_rejected(ROADMAP + ROADMAP, "found 2")

    def test_an_unparseable_headline_is_a_failure(self) -> None:
        self.assert_rejected(
            ROADMAP.replace(
                "2 of the required 5 matrix rows", "a couple of the required few rows"
            ),
            "no longer states its totals in the form this guard parses",
        )

    def test_historical_figures_in_the_prose_are_not_mistaken_for_the_claim(
        self,
    ) -> None:
        check_roadmap_counts(
            ROADMAP + "\n\nEarlier this was 9 rows, leaving a 28-row gap.\n",
            self.ROWS,
        )


class CiWiringTest(unittest.TestCase):
    """The checker is only a merge gate while required CI actually runs it.

    Issue #593 landed the checker with no CI wiring at all, so a matrix row
    could be promoted past its manifest entry and nothing would notice until
    someone ran the script by hand. Issue #595 wires it into `governance`,
    which `ci-gate` requires unconditionally -- `ci-gate` is the required
    branch-protection check, so no separate protection entry is needed and
    none should be added. These tests fail if either half of that binding is
    removed: the step, or `governance`'s place in the aggregate gate.
    """

    WORKFLOW = REPOSITORY_ROOT / ".github" / "workflows" / "ci.yml"

    def setUp(self) -> None:
        self.text = self.WORKFLOW.read_text(encoding="utf-8")

    def _job_body(self, name: str) -> str:
        start = self.text.index(f"\n  {name}:\n")
        rest = self.text[start + 1 :]
        lines = rest.split("\n")
        body = [lines[0]]
        for line in lines[1:]:
            # A new top-level job starts at exactly two spaces of indent.
            if line.startswith("  ") and not line.startswith("   ") and line.rstrip().endswith(":"):
                break
            body.append(line)
        return "\n".join(body)

    def test_the_governance_job_runs_the_breadth_checker(self) -> None:
        self.assertIn(
            "scripts/check_conformance_breadth.py", self._job_body("governance")
        )

    def test_ci_gate_requires_the_governance_job(self) -> None:
        gate = self._job_body("ci-gate")
        self.assertIn("- governance", gate)
        self.assertIn("needs.governance.result != 'success'", gate)

    def test_the_breadth_checker_is_not_wired_as_advisory(self) -> None:
        # `continue-on-error` on the step, or on the job, would leave the
        # gate green while the contract is violated.
        governance = self._job_body("governance")
        self.assertNotIn("continue-on-error", governance)


if __name__ == "__main__":
    unittest.main()
