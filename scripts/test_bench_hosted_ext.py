#!/usr/bin/env python3
"""Tests for the hosted `ext` benchmark runner.

The runner is pre-registration machinery, not a measurement: every rule it
enforces comes from `docs/TESTING.md`'s "Hosted `ext` benchmark protocol
(product-sprint-1)" section, and a rule that is not exercised here is a rule the
runner can quietly stop enforcing. The statistic tests pin median-of-seven and
ratio-of-medians; the guard tests pin the refusals that make a timing
admissible.
"""

from __future__ import annotations

import hashlib
import importlib.util
from pathlib import Path
import re
import sys
import tempfile
import unittest

MODULE_PATH = Path(__file__).with_name("bench_hosted_ext.py")
SPEC = importlib.util.spec_from_file_location("hosted_ext_benchmark_runner", MODULE_PATH)
if SPEC is None or SPEC.loader is None:
    raise RuntimeError("could not load the hosted ext benchmark runner")
RUNNER = importlib.util.module_from_spec(SPEC)
# Registered before execution so `dataclasses` can resolve the module's own
# namespace while it processes `ArmOutcome`.
sys.modules[SPEC.name] = RUNNER
SPEC.loader.exec_module(RUNNER)
BenchmarkError = RUNNER.BenchmarkError


class InterpreterGuardTest(unittest.TestCase):
    def test_resolves_the_benchmark_interpreter_before_the_compiler_one(self) -> None:
        environment = {"PYCC_BENCH_PYTHON": "/bench/python", "PYCC_PYTHON": "/other/python"}

        self.assertEqual(RUNNER.resolve_interpreter(environment), "/bench/python")

    def test_falls_back_to_the_compiler_interpreter(self) -> None:
        self.assertEqual(
            RUNNER.resolve_interpreter({"PYCC_PYTHON": "/other/python"}), "/other/python"
        )

    def test_refuses_when_no_interpreter_is_configured(self) -> None:
        with self.assertRaises(BenchmarkError) as raised:
            RUNNER.resolve_interpreter({})

        self.assertIn("PYCC_BENCH_PYTHON", str(raised.exception))

    def test_accepts_the_pinned_version(self) -> None:
        RUNNER.assert_pinned_version("Python 3.14.7 (main, Sep 1 2026, 00:00:00) [Clang]")

    def test_refuses_any_other_version(self) -> None:
        with self.assertRaises(BenchmarkError) as raised:
            RUNNER.assert_pinned_version("Python 3.14.6 (main, Sep 1 2026, 00:00:00) [Clang]")

        self.assertIn(RUNNER.PINNED_PYTHON_VERSION, str(raised.exception))

    def test_accepts_an_optimized_build(self) -> None:
        RUNNER.assert_optimized_build("--enable-optimizations --with-lto")

    def test_refuses_a_debug_build(self) -> None:
        for configure_args in (
            "--with-pydebug",
            "--with-trace-refs",
            "--without-pymalloc",
        ):
            with self.subTest(configure_args=configure_args):
                with self.assertRaises(BenchmarkError):
                    RUNNER.assert_optimized_build(configure_args)

    def test_refuses_when_configure_args_are_unavailable(self) -> None:
        with self.assertRaises(BenchmarkError):
            RUNNER.assert_optimized_build(None)


class SubjectGuardTest(unittest.TestCase):
    def test_refuses_when_the_subject_is_not_configured(self) -> None:
        with self.assertRaises(BenchmarkError) as raised:
            RUNNER.resolve_subject({})

        self.assertIn("PYCC_BENCH_SUBJECT", str(raised.exception))

    def test_refuses_a_missing_subject_without_echoing_the_path(self) -> None:
        secret = "/private/reference/hot_loop.py"

        with self.assertRaises(BenchmarkError) as raised:
            RUNNER.resolve_subject({"PYCC_BENCH_SUBJECT": secret})

        self.assertNotIn(secret, str(raised.exception))

    def test_accepts_an_existing_subject(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            subject = Path(directory) / "hot_loop.py"
            subject.write_text("def hot(a: float) -> float:\n    return a\n", encoding="utf-8")

            self.assertEqual(
                RUNNER.resolve_subject({"PYCC_BENCH_SUBJECT": str(subject)}), subject
            )


class InputDigestTest(unittest.TestCase):
    def test_accepts_the_committed_digest(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "input.bin"
            path.write_bytes(b"payload")

            RUNNER.verify_input_digest(path, hashlib.sha256(b"payload").hexdigest())

    def test_refuses_a_digest_mismatch(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "input.bin"
            path.write_bytes(b"payload")

            with self.assertRaises(BenchmarkError) as raised:
                RUNNER.verify_input_digest(path, "0" * 64)

        self.assertIn("SHA-256", str(raised.exception))


class CorrectnessPreconditionTest(unittest.TestCase):
    def reference(self, **overrides: object) -> object:
        fields = {"value": 3, "exception": None, "arguments_digest": "a"}
        fields.update(overrides)
        return RUNNER.ArmOutcome(**fields)

    def test_accepts_identical_integers(self) -> None:
        RUNNER.compare_outcomes(self.reference(), self.reference(), tolerance=1e-9)

    def test_refuses_a_differing_integer(self) -> None:
        with self.assertRaises(BenchmarkError):
            RUNNER.compare_outcomes(self.reference(), self.reference(value=4), tolerance=1e-9)

    def test_refuses_a_bool_where_an_int_was_returned(self) -> None:
        with self.assertRaises(BenchmarkError):
            RUNNER.compare_outcomes(
                self.reference(value=1), self.reference(value=True), tolerance=1e-9
            )

    def test_accepts_a_float_inside_the_committed_tolerance(self) -> None:
        RUNNER.compare_outcomes(
            self.reference(value=1.0),
            self.reference(value=1.0 + 5e-13),
            tolerance=1e-12,
        )

    def test_refuses_a_float_outside_the_committed_tolerance(self) -> None:
        with self.assertRaises(BenchmarkError):
            RUNNER.compare_outcomes(
                self.reference(value=1.0),
                self.reference(value=1.0 + 5e-11),
                tolerance=1e-12,
            )

    def test_refuses_a_differing_exception_type(self) -> None:
        with self.assertRaises(BenchmarkError):
            RUNNER.compare_outcomes(
                self.reference(value=None, exception="ValueError"),
                self.reference(value=None, exception="TypeError"),
                tolerance=1e-12,
            )

    def test_refuses_an_arm_that_raises_where_the_baseline_returned(self) -> None:
        with self.assertRaises(BenchmarkError):
            RUNNER.compare_outcomes(
                self.reference(),
                self.reference(value=None, exception="ValueError"),
                tolerance=1e-12,
            )

    def test_refuses_an_arm_that_clobbered_its_arguments(self) -> None:
        with self.assertRaises(BenchmarkError):
            RUNNER.compare_outcomes(
                self.reference(), self.reference(arguments_digest="b"), tolerance=1e-12
            )


class StatisticTest(unittest.TestCase):
    def test_replicate_count_is_seven(self) -> None:
        self.assertEqual(RUNNER.REPLICATES, 7)

    def test_median_is_the_middle_of_seven(self) -> None:
        self.assertEqual(RUNNER.median_ns([9, 1, 5, 3, 7, 11, 13]), 7)

    def test_median_ignores_one_descheduled_replicate(self) -> None:
        self.assertEqual(RUNNER.median_ns([9, 1, 5, 3, 7, 11, 10**9]), 7)

    def test_refuses_a_replicate_count_other_than_seven(self) -> None:
        with self.assertRaises(BenchmarkError):
            RUNNER.summarize("cpython", [1, 2, 3])

    def test_summary_reports_median_minimum_and_maximum(self) -> None:
        summary = RUNNER.summarize("cpython", [9, 1, 5, 3, 7, 11, 13])

        self.assertEqual(
            summary,
            {"arm": "cpython", "median_ns": 7, "min_ns": 1, "max_ns": 13, "replicates": 7},
        )

    def test_speedup_is_a_ratio_of_medians(self) -> None:
        baseline = RUNNER.summarize("cpython", [10, 10, 10, 10, 10, 10, 10])
        candidate = RUNNER.summarize("ext", [2, 2, 2, 2, 2, 2, 2])

        self.assertEqual(RUNNER.ratio_of_medians(baseline, candidate), 5.0)


class ProtocolWordingTest(unittest.TestCase):
    def test_the_runner_never_uses_the_forbidden_average(self) -> None:
        source = MODULE_PATH.read_text(encoding="utf-8")

        self.assertIsNone(re.search(r"\bmean\b", source, re.IGNORECASE))


class PreRegistrationTest(unittest.TestCase):
    def test_record_carries_every_field_the_protocol_commits(self) -> None:
        record = RUNNER.read_pre_registration(
            Path(__file__).with_name("bench_hosted_ext_precommit.json")
        )

        for field in (
            "generator_path",
            "seed",
            "triangles",
            "input_sha256",
            "float_tolerance",
            "float_tolerance_rationale",
            "machine",
            "compile_unchanged_denominator",
            "compile_unchanged_set_sha256",
            "compile_unchanged_set_digest_rule",
        ):
            with self.subTest(field=field):
                self.assertIn(field, record)

    def test_record_carries_no_result(self) -> None:
        record = RUNNER.read_pre_registration(
            Path(__file__).with_name("bench_hosted_ext_precommit.json")
        )

        for field in ("median_ns", "ratio", "speedup", "compile_unchanged_count"):
            with self.subTest(field=field):
                self.assertNotIn(field, record)

    def test_machine_identity_carries_the_committed_shape(self) -> None:
        record = RUNNER.read_pre_registration(
            Path(__file__).with_name("bench_hosted_ext_precommit.json")
        )

        for field in ("model_identifier", "cpu", "cores", "memory_bytes", "os", "power_profile"):
            with self.subTest(field=field):
                self.assertIn(field, record["machine"])

    def test_input_digest_is_a_sha256_hex_string(self) -> None:
        record = RUNNER.read_pre_registration(
            Path(__file__).with_name("bench_hosted_ext_precommit.json")
        )

        self.assertRegex(record["input_sha256"], r"\A[0-9a-f]{64}\Z")
        self.assertRegex(record["compile_unchanged_set_sha256"], r"\A[0-9a-f]{64}\Z")


if __name__ == "__main__":
    unittest.main()
