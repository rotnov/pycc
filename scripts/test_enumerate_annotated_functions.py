#!/usr/bin/env python3
"""Tests for the annotated-function enumerator.

`docs/TESTING.md`'s "The compile-unchanged count" bullet requires the
denominator -- how many annotated functions are attempted -- and a digest over
that exact set to be committed before any compile may be scored, and it
forbids publishing the names themselves. This enumerator is the reproducible
derivation of both numbers, so its predicate and its digest rule are what these
tests pin.
"""

from __future__ import annotations

import hashlib
import importlib.util
from pathlib import Path
import tempfile
import unittest

MODULE_PATH = Path(__file__).with_name("enumerate_annotated_functions.py")
SPEC = importlib.util.spec_from_file_location("annotated_function_enumerator", MODULE_PATH)
if SPEC is None or SPEC.loader is None:
    raise RuntimeError("could not load the annotated-function enumerator")
ENUMERATOR = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(ENUMERATOR)


def write_tree(root: Path, files: dict[str, str]) -> None:
    for relative, source in files.items():
        path = root / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(source, encoding="utf-8")


class EnumeratePredicateTest(unittest.TestCase):
    def test_collects_only_fully_annotated_module_level_functions(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            write_tree(
                root,
                {
                    "geometry/area.py": (
                        "def annotated(a: float, b: float) -> float:\n"
                        "    return a * b\n"
                        "\n"
                        "def missing_return(a: float):\n"
                        "    return a\n"
                        "\n"
                        "def missing_parameter(a, b: float) -> float:\n"
                        "    return b\n"
                        "\n"
                        "class Holder:\n"
                        "    def method(self, a: float) -> float:\n"
                        "        return a\n"
                        "\n"
                        "async def coroutine(a: float) -> float:\n"
                        "    return a\n"
                        "\n"
                        "def outer(a: float) -> float:\n"
                        "    def inner(b: float) -> float:\n"
                        "        return b\n"
                        "    return inner(a)\n"
                    ),
                    "geometry/__init__.py": "",
                },
            )

            names = ENUMERATOR.collect_annotated_functions(root)

        self.assertEqual(names, ["geometry.area.annotated", "geometry.area.outer"])

    def test_annotates_every_parameter_kind(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            write_tree(
                root,
                {
                    "kinds.py": (
                        "def positional(a: int, /, b: int, *args: int,"
                        " c: int = 0, **kwargs: int) -> int:\n"
                        "    return a\n"
                        "\n"
                        "def unannotated_vararg(a: int, *args) -> int:\n"
                        "    return a\n"
                        "\n"
                        "def unannotated_kwarg(a: int, **kwargs) -> int:\n"
                        "    return a\n"
                        "\n"
                        "def unannotated_keyword_only(a: int, *, b) -> int:\n"
                        "    return a\n"
                    )
                },
            )

            names = ENUMERATOR.collect_annotated_functions(root)

        self.assertEqual(names, ["kinds.positional"])

    def test_skips_ignored_directories_and_unparsable_files(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            write_tree(
                root,
                {
                    "kept.py": "def kept(a: int) -> int:\n    return a\n",
                    ".venv/hidden.py": "def hidden(a: int) -> int:\n    return a\n",
                    "build/generated.py": "def generated(a: int) -> int:\n    return a\n",
                    "broken.py": "def broken(a: int) -> int\n",
                },
            )

            names = ENUMERATOR.collect_annotated_functions(root)

        self.assertEqual(names, ["kept.kept"])

    def test_names_are_sorted_byte_wise_ascending(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            write_tree(
                root,
                {
                    "b.py": "def one(a: int) -> int:\n    return a\n",
                    "a.py": "def Zed(a: int) -> int:\n    return a\n",
                },
            )

            names = ENUMERATOR.collect_annotated_functions(root)

        self.assertEqual(names, ["a.Zed", "b.one"])


class DigestRuleTest(unittest.TestCase):
    def test_digest_is_sha256_over_newline_joined_names_without_trailing_newline(self) -> None:
        names = ["alpha.one", "beta.two"]

        expected = hashlib.sha256("alpha.one\nbeta.two".encode("utf-8")).hexdigest()

        self.assertEqual(ENUMERATOR.digest_of(names), expected)

    def test_empty_set_digests_the_empty_string(self) -> None:
        self.assertEqual(
            ENUMERATOR.digest_of([]), hashlib.sha256(b"").hexdigest()
        )

    def test_digest_rule_text_matches_the_pre_registration_record(self) -> None:
        record = ENUMERATOR.read_pre_registration(
            Path(__file__).with_name("bench_hosted_ext_precommit.json")
        )

        self.assertEqual(
            record["compile_unchanged_set_digest_rule"], ENUMERATOR.DIGEST_RULE
        )



class SubtreeRestrictionTest(unittest.TestCase):
    """The denominator counts first-party source, not everything on disk.

    A tree that vendors third-party checkouts and carries its own test suite
    would otherwise contribute functions that are nobody's idea of "the
    reference codebase's annotated functions", so the enumerated subtrees are
    named explicitly and recorded in the pre-registration record.
    """

    def test_restricts_enumeration_to_the_named_subtrees(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            write_tree(
                root,
                {
                    "src/alpha.py": "def one(value: int) -> int:\n    return value\n",
                    "tests/beta.py": "def two(value: int) -> int:\n    return value\n",
                    "third/gamma.py": "def three(value: int) -> int:\n    return value\n",
                },
            )

            names = ENUMERATOR.collect_annotated_functions(root, ["src"])

            self.assertEqual(names, ["src.alpha.one"])

    def test_enumerating_without_subtrees_walks_the_whole_root(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            write_tree(
                root,
                {
                    "src/alpha.py": "def one(value: int) -> int:\n    return value\n",
                    "tests/beta.py": "def two(value: int) -> int:\n    return value\n",
                },
            )

            names = ENUMERATOR.collect_annotated_functions(root)

            self.assertEqual(names, ["src.alpha.one", "tests.beta.two"])

    def test_rejects_a_subtree_that_is_not_a_directory(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            write_tree(root, {"src/alpha.py": "def one(value: int) -> int:\n    return value\n"})

            with self.assertRaises(ValueError):
                ENUMERATOR.collect_annotated_functions(root, ["absent"])

    def test_rejects_a_subtree_that_escapes_the_root(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            write_tree(root, {"src/alpha.py": "def one(value: int) -> int:\n    return value\n"})

            with self.assertRaises(ValueError):
                ENUMERATOR.collect_annotated_functions(root, ["../elsewhere"])

    def test_pre_registration_records_the_subtrees_that_were_enumerated(self) -> None:
        record = ENUMERATOR.read_pre_registration(
            Path(__file__).with_name("bench_hosted_ext_precommit.json")
        )

        subtrees = record["compile_unchanged_subtrees"]
        self.assertIsInstance(subtrees, list)
        self.assertTrue(subtrees)
        for subtree in subtrees:
            with self.subTest(subtree=subtree):
                self.assertIsInstance(subtree, str)


class ReportTest(unittest.TestCase):
    def test_report_publishes_only_the_count_and_the_digest(self) -> None:
        names = ["private.alpha", "private.beta"]

        report = ENUMERATOR.format_report(names)

        self.assertEqual(
            report,
            f"count={len(names)}\ndigest={ENUMERATOR.digest_of(names)}",
        )
        for name in names:
            self.assertNotIn(name, report)


if __name__ == "__main__":
    unittest.main()
