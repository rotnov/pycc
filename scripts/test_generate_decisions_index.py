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


if __name__ == "__main__":
    unittest.main()
