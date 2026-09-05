from __future__ import annotations

import tempfile
import unittest
from pathlib import Path

import generate_decisions_index as gen


def write_decision(directory, filename, id_, title, status):
    content = f'---\nid: {id_}\ntitle: "{title}"\nstatus: {status}\n---\n\n# {id_}: {title}\n'
    (directory / filename).write_text(content, encoding="utf-8")


class ReadFrontmatterTests(unittest.TestCase):
    def test_reads_id_title_status(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "D-002-second.md"
            write_decision(Path(directory), "D-002-second.md", "D-002", "Second decision", "accepted")
            self.assertEqual(gen.read_frontmatter(path), ("D-002", "Second decision", "accepted"))

    def test_unescapes_quotes_in_title(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "D-003.md"
            content = '---\nid: D-003\ntitle: "Uses \\"quotes\\" and a backslash \\\\"\nstatus: accepted\n---\n\nbody\n'
            path.write_text(content, encoding="utf-8")
            _, title, _ = gen.read_frontmatter(path)
            self.assertEqual(title, 'Uses "quotes" and a backslash \\')

    def test_missing_frontmatter_raises(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "D-004.md"
            path.write_text("# no frontmatter here\n", encoding="utf-8")
            with self.assertRaises(ValueError):
                gen.read_frontmatter(path)


class GenerateIndexTests(unittest.TestCase):
    def test_sorts_numerically_not_lexically(self):
        with tempfile.TemporaryDirectory() as directory:
            d = Path(directory)
            write_decision(d, "D-002-second.md", "D-002", "Second", "accepted")
            write_decision(d, "D-010-tenth.md", "D-010", "Tenth", "accepted")
            write_decision(d, "D-001-first.md", "D-001", "First", "proposed")
            table = gen.generate_index(d)
            first_pos = table.index("D-001")
            second_pos = table.index("D-002")
            tenth_pos = table.index("D-010")
            self.assertLess(first_pos, second_pos)
            self.assertLess(second_pos, tenth_pos)

    def test_links_point_at_the_real_filename(self):
        with tempfile.TemporaryDirectory() as directory:
            d = Path(directory)
            write_decision(d, "D-002-second-decision.md", "D-002", "Second decision", "accepted")
            table = gen.generate_index(d)
            self.assertIn("[D-002](./D-002-second-decision.md)", table)

    def test_includes_status_column(self):
        with tempfile.TemporaryDirectory() as directory:
            d = Path(directory)
            write_decision(d, "D-001-first.md", "D-001", "First", "proposed")
            table = gen.generate_index(d)
            self.assertIn("| proposed |", table)

    def test_duplicate_id_raises(self):
        # Regression for https://github.com/rotnov/pycc/issues/803: two
        # distinct files whose frontmatter both claim id D-201 must fail
        # generation instead of silently producing a README with two rows
        # for the same decision number.
        with tempfile.TemporaryDirectory() as directory:
            d = Path(directory)
            write_decision(d, "D-201-first-thing.md", "D-201", "First thing", "accepted")
            write_decision(d, "D-201-second-thing.md", "D-201", "Second thing", "accepted")
            with self.assertRaisesRegex(ValueError, "duplicate decision id D-201"):
                gen.generate_index(d)

    def test_filename_prefix_mismatch_raises(self):
        # Regression for the P2 finding on
        # https://github.com/rotnov/pycc/pull/820: a file named with one
        # D-NNN prefix but frontmatter claiming a different id must fail
        # generation, since check_unique_ids alone would not catch it (the
        # frontmatter ids differ and are individually unique).
        with tempfile.TemporaryDirectory() as directory:
            d = Path(directory)
            write_decision(d, "D-201-first-thing.md", "D-999", "Mismatched", "accepted")
            with self.assertRaisesRegex(
                ValueError, r"D-201-first-thing\.md: filename prefix does not match"
            ):
                gen.generate_index(d)

    def test_filename_prefix_match_passes(self):
        with tempfile.TemporaryDirectory() as directory:
            d = Path(directory)
            write_decision(d, "D-201-first-thing.md", "D-201", "First thing", "accepted")
            table = gen.generate_index(d)
            self.assertIn("D-201", table)


class MainCheckModeTests(unittest.TestCase):
    def test_check_passes_when_readme_matches_generated(self):
        with tempfile.TemporaryDirectory() as directory:
            d = Path(directory)
            write_decision(d, "D-001-first.md", "D-001", "First", "accepted")
            readme = d / "README.md"
            readme.write_text(gen.generate_index(d), encoding="utf-8")
            exit_code = gen.main([str(d), str(readme), "--check"])
            self.assertEqual(exit_code, 0)

    def test_check_fails_when_readme_is_stale(self):
        with tempfile.TemporaryDirectory() as directory:
            d = Path(directory)
            write_decision(d, "D-001-first.md", "D-001", "First", "accepted")
            readme = d / "README.md"
            readme.write_text("stale content\n", encoding="utf-8")
            exit_code = gen.main([str(d), str(readme), "--check"])
            self.assertEqual(exit_code, 1)

    def test_writes_readme_without_check(self):
        with tempfile.TemporaryDirectory() as directory:
            d = Path(directory)
            write_decision(d, "D-001-first.md", "D-001", "First", "accepted")
            readme = d / "README.md"
            exit_code = gen.main([str(d), str(readme)])
            self.assertEqual(exit_code, 0)
            self.assertIn("D-001", readme.read_text(encoding="utf-8"))

    def test_main_fails_closed_on_duplicate_id_without_check(self):
        with tempfile.TemporaryDirectory() as directory:
            d = Path(directory)
            write_decision(d, "D-201-first-thing.md", "D-201", "First thing", "accepted")
            write_decision(d, "D-201-second-thing.md", "D-201", "Second thing", "accepted")
            readme = d / "README.md"
            exit_code = gen.main([str(d), str(readme)])
            self.assertEqual(exit_code, 1)
            self.assertFalse(readme.exists())

    def test_main_fails_closed_on_duplicate_id_with_check(self):
        with tempfile.TemporaryDirectory() as directory:
            d = Path(directory)
            write_decision(d, "D-201-first-thing.md", "D-201", "First thing", "accepted")
            write_decision(d, "D-201-second-thing.md", "D-201", "Second thing", "accepted")
            readme = d / "README.md"
            readme.write_text("stale content\n", encoding="utf-8")
            exit_code = gen.main([str(d), str(readme), "--check"])
            self.assertEqual(exit_code, 1)


class CiWiringTest(unittest.TestCase):
    """The checker is only a merge gate while required CI actually runs it.

    Issue #929: `--check` existed, with `check_unique_ids` and
    `check_filename_matches_id`, and was wired to nothing, so a real `D-227`
    id collision (a merge landing mid-review of #918) reached review with
    every required check green. The step lives in `governance`, which
    `ci-gate` -- the required branch-protection check -- needs
    unconditionally. These tests fail if either half of that binding is
    removed, or if the step is downgraded to advisory.
    """

    WORKFLOW = Path(__file__).resolve().parent.parent / ".github" / "workflows" / "ci.yml"
    STEP = "scripts/generate_decisions_index.py docs/decisions docs/decisions/README.md --check"

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

    def test_the_governance_job_runs_the_index_check(self) -> None:
        self.assertIn(self.STEP, self._job_body("governance"))

    def test_ci_gate_requires_the_governance_job(self) -> None:
        gate = self._job_body("ci-gate")
        self.assertIn("- governance", gate)
        self.assertIn("needs.governance.result != 'success'", gate)

    def test_the_index_check_is_not_wired_as_advisory(self) -> None:
        self.assertNotIn("continue-on-error", self._job_body("governance"))


if __name__ == "__main__":
    unittest.main()
