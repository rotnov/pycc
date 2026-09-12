#!/usr/bin/env python3
"""Tests for scripts/select_codecontests_corpus.py's pure helpers.

The selector's network and parquet paths are developer tooling that CI never
runs, so they are deliberately out of scope here. What is in scope is every
helper whose behavior the vendored corpus depends on: line counting, the
untrusted-name slug rule, the import-allowlist walk, manifest determinism, the
case packing, and the first-passing-solution choice.
"""

from __future__ import annotations

import ast
import importlib.util
import json
import tempfile
import unittest
from pathlib import Path
from unittest import mock

SELECTOR_PATH = Path(__file__).with_name("select_codecontests_corpus.py")
SELECTOR_SPEC = importlib.util.spec_from_file_location("corpus_selector", SELECTOR_PATH)
if SELECTOR_SPEC is None or SELECTOR_SPEC.loader is None:
    raise RuntimeError("could not load scripts/select_codecontests_corpus.py")
SELECTOR = importlib.util.module_from_spec(SELECTOR_SPEC)
SELECTOR_SPEC.loader.exec_module(SELECTOR)


class PhysicalLineCountTests(unittest.TestCase):
    def test_counts_lines_ignoring_one_trailing_newline(self) -> None:
        self.assertEqual(SELECTOR.physical_line_count("a\nb\nc\n"), 3)
        self.assertEqual(SELECTOR.physical_line_count("a\nb\nc"), 3)

    def test_counts_blank_interior_lines(self) -> None:
        self.assertEqual(SELECTOR.physical_line_count("a\n\nb\n"), 3)

    def test_empty_source_has_no_lines(self) -> None:
        self.assertEqual(SELECTOR.physical_line_count(""), 0)


class SlugifyTests(unittest.TestCase):
    def test_lowercases_and_joins_with_hyphens(self) -> None:
        self.assertEqual(SELECTOR.slugify("1575_A. Another Sorting"), "1575-a-another-sorting")

    def test_strips_path_traversal_and_separators(self) -> None:
        for hostile in ("../../etc/passwd", "..", "/", "a/../b", "C:\\evil"):
            slug = SELECTOR.slugify(hostile)
            self.assertNotIn("/", slug)
            self.assertNotIn("\\", slug)
            self.assertNotIn("..", slug)
            self.assertTrue(slug)

    def test_never_empty_and_bounded(self) -> None:
        self.assertEqual(SELECTOR.slugify("!!!"), "problem")
        self.assertEqual(SELECTOR.slugify(""), "problem")
        self.assertLessEqual(len(SELECTOR.slugify("x" * 400)), 48)

    def test_result_is_restricted_to_the_safe_alphabet(self) -> None:
        slug = SELECTOR.slugify("Ünïcode ☃ 42 -- name!")
        self.assertRegex(slug, r"^[a-z0-9-]+$")


class ResolveProblemDirTests(unittest.TestCase):
    def test_builds_a_numbered_directory_inside_the_root(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            path = SELECTOR.resolve_problem_dir(root, "problems", 7, "1575_A. Sort")
            self.assertEqual(path.name, "007-1575-a-sort")
            self.assertEqual(path.parent, (root / "problems").resolve())

    def test_rejects_a_slug_that_would_escape_the_root(self) -> None:
        # Defense in depth: if the slug rule were ever weakened, the path guard
        # must still refuse to write outside the corpus directory.
        with tempfile.TemporaryDirectory() as tmp:
            with mock.patch.object(SELECTOR, "slugify", return_value="../escaped"):
                with self.assertRaises(ValueError):
                    SELECTOR.resolve_problem_dir(Path(tmp), "problems", 1, "x")


class ImportWalkTests(unittest.TestCase):
    def test_collects_plain_and_from_import_roots(self) -> None:
        tree = ast.parse("import sys, os.path\nfrom collections import deque\n")
        self.assertEqual(SELECTOR.import_roots(tree), {"sys", "os", "collections"})

    def test_collects_imports_nested_in_functions(self) -> None:
        tree = ast.parse("def f():\n    import socket\n")
        self.assertEqual(SELECTOR.import_roots(tree), {"socket"})

    def test_relative_import_is_recorded_as_unallowlistable(self) -> None:
        tree = ast.parse("from . import helper\n")
        self.assertEqual(SELECTOR.import_roots(tree), {""})
        self.assertFalse(SELECTOR.imports_allowlisted(SELECTOR.import_roots(tree)))

    def test_allowlist_accepts_the_documented_roots_only(self) -> None:
        self.assertTrue(SELECTOR.imports_allowlisted({"sys", "math", "collections"}))
        for denied in ("os", "socket", "subprocess", "pathlib", "random"):
            self.assertFalse(SELECTOR.imports_allowlisted({denied}), denied)

    def test_allowlist_excludes_every_dangerous_root(self) -> None:
        for denied in ("os", "socket", "subprocess", "shutil", "urllib", "ctypes"):
            self.assertNotIn(denied, SELECTOR.ALLOWED_IMPORT_ROOTS)

    def test_no_imports_is_allowlisted(self) -> None:
        self.assertTrue(SELECTOR.imports_allowlisted(SELECTOR.import_roots(ast.parse("x = 1\n"))))


class NormalizeSourceTests(unittest.TestCase):
    def test_converts_crlf_and_cr_to_lf(self) -> None:
        self.assertEqual(SELECTOR.normalize_source("a\r\nb\rc"), "a\nb\nc\n")

    def test_collapses_trailing_newlines_to_exactly_one(self) -> None:
        self.assertEqual(SELECTOR.normalize_source("a\n\n\n"), "a\n")


class CanonicalJsonTests(unittest.TestCase):
    def test_is_sorted_indented_and_newline_terminated(self) -> None:
        text = SELECTOR.canonical_json({"b": 1, "a": 2})
        self.assertEqual(text, '{\n  "a": 2,\n  "b": 1\n}\n')

    def test_is_stable_across_input_key_order(self) -> None:
        first = SELECTOR.canonical_json({"a": [1, 2], "b": {"d": 1, "c": 2}})
        second = SELECTOR.canonical_json({"b": {"c": 2, "d": 1}, "a": [1, 2]})
        self.assertEqual(first, second)

    def test_escapes_non_ascii_so_the_bytes_are_portable(self) -> None:
        self.assertNotIn("☃", SELECTOR.canonical_json({"k": "☃"}))


class CasePackingTests(unittest.TestCase):
    def test_zips_the_parallel_dataset_arrays(self) -> None:
        struct = {"input": ["1\n", "2\n"], "output": ["a\n", "b\n"]}
        self.assertEqual(
            SELECTOR.cases_from_struct(struct),
            [{"input": "1\n", "output": "a\n"}, {"input": "2\n", "output": "b\n"}],
        )

    def test_missing_or_empty_struct_yields_no_cases(self) -> None:
        self.assertEqual(SELECTOR.cases_from_struct(None), [])
        self.assertEqual(SELECTOR.cases_from_struct({}), [])

    def test_truncates_to_the_shorter_array_rather_than_inventing_a_case(self) -> None:
        struct = {"input": ["1\n", "2\n"], "output": ["a\n"]}
        self.assertEqual(len(SELECTOR.cases_from_struct(struct)), 1)

    def test_payload_is_canonical_json_with_a_cases_key(self) -> None:
        payload = SELECTOR.tests_payload([{"input": "1\n", "output": "a\n"}])
        self.assertEqual(json.loads(payload)["cases"][0]["input"], "1\n")
        self.assertTrue(payload.endswith("\n"))


class SolutionChoiceTests(unittest.TestCase):
    def test_keeps_only_python3_solutions_in_dataset_order(self) -> None:
        solutions = {
            "language": [2, 3, 1, 3, 4],
            "solution": ["cpp", "first", "py2", "second", "java"],
        }
        self.assertEqual(
            SELECTOR.choose_python3_solutions(solutions), ["first", "second"]
        )

    def test_python2_is_not_mistaken_for_python3(self) -> None:
        self.assertEqual(SELECTOR.PYTHON3_LANGUAGE, 3)
        self.assertEqual(
            SELECTOR.choose_python3_solutions({"language": [1], "solution": ["py2"]}), []
        )

    def test_missing_solutions_struct_yields_nothing(self) -> None:
        self.assertEqual(SELECTOR.choose_python3_solutions(None), [])


class ShardPinTests(unittest.TestCase):
    def test_every_shard_is_pinned_by_size_and_digest(self) -> None:
        self.assertEqual(len(SELECTOR.SHARDS), 4)
        for shard in SELECTOR.SHARDS:
            self.assertGreater(shard["bytes"], 0)
            self.assertRegex(shard["sha256"], r"^[0-9a-f]{64}$")

    def test_split_ranks_are_unique_and_order_the_candidate_pool(self) -> None:
        ranks = [shard["split_rank"] for shard in SELECTOR.SHARDS]
        self.assertEqual(ranks, sorted(set(ranks)))

    def test_shard_url_pins_the_revision(self) -> None:
        url = SELECTOR.shard_url("abc123", "data/x.parquet")
        self.assertIn("/resolve/abc123/data/x.parquet", url)


class ReportTests(unittest.TestCase):
    def test_report_lists_every_filter_even_at_zero(self) -> None:
        rejected = {name: 0 for name in SELECTOR.FILTERS}
        text = SELECTOR.format_report(0, rejected, [], [], 0)
        for name in SELECTOR.FILTERS:
            self.assertIn(name, text)
        self.assertIn("n/a (0 accepted)", text)

    def test_report_summarises_the_wall_time_distribution(self) -> None:
        rejected = {name: 0 for name in SELECTOR.FILTERS}
        records = [{"cpython_wall_seconds": value} for value in (0.05, 0.25, 0.5)]
        text = SELECTOR.format_report(10, rejected, records, [], 1234)
        self.assertIn("candidates examined: 10", text)
        self.assertIn("total vendored bytes: 1234", text)
        self.assertIn("200ms speedup floor: 2", text)


class ParserTests(unittest.TestCase):
    def test_defaults_match_the_documented_corpus_shape(self) -> None:
        args = SELECTOR.build_parser().parse_args([])
        self.assertEqual(args.target, 200)
        self.assertEqual(args.holdout, 100)
        self.assertEqual(args.max_bytes, 16 * 1024 * 1024)
        self.assertEqual(args.out, "tests/corpus/codecontests")
        self.assertEqual(args.revision, SELECTOR.DEFAULT_REVISION)


if __name__ == "__main__":
    unittest.main()
