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
import json
import os
from pathlib import Path
import re
import subprocess
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


def committed_record() -> dict:
    """The record's content, without the git binding `read_pre_registration` adds."""

    return json.loads(
        Path(__file__).with_name("bench_hosted_ext_precommit.json").read_text(encoding="utf-8")
    )


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

    # `docs/TESTING.md`'s "Versions" bullet makes the pinned interpreter both
    # the baseline arm and the host that imports the `ext` artifact, and
    # `run_arm` times its callables in this process. Both directions below
    # compare two injected identities, so nothing here reads the ambient
    # interpreter and the test says the same thing on macOS and on Linux CI.
    def test_accepts_the_configured_interpreter_that_is_this_process(self) -> None:
        RUNNER.assert_hosts_the_arms("/opt/pinned/bin/python3.14", "/opt/pinned/bin/python3.14")

    def test_refuses_a_configured_interpreter_that_is_not_this_process(self) -> None:
        with self.assertRaises(BenchmarkError) as raised:
            RUNNER.assert_hosts_the_arms("/opt/pinned/bin/python3.14", "/usr/bin/python3.11")

        self.assertIn("PYCC_BENCH_PYTHON", str(raised.exception))

    def test_refuses_an_interpreter_that_reported_no_identity(self) -> None:
        with self.assertRaises(BenchmarkError):
            RUNNER.assert_hosts_the_arms("", "/usr/bin/python3.11")

    # Every other precondition in the runner refuses through `BenchmarkError`;
    # the two calls that ask the configured interpreter about itself must do the
    # same rather than raising a bare `FileNotFoundError` or
    # `CalledProcessError` through `main`. Both directions run the *current*
    # interpreter -- the portable fact that it exists and can exit non-zero on
    # demand -- rather than reading anything about the host, so this says the
    # same thing on macOS and on Linux CI.
    def test_returns_what_an_interpreter_that_answers_printed(self) -> None:
        self.assertEqual(
            RUNNER.ask_interpreter([sys.executable, "-c", "print('answered')"]).strip(),
            "answered",
        )

    def test_refuses_an_interpreter_that_cannot_be_run(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            absent = os.path.join(directory, "no-such-interpreter")
            with self.assertRaises(BenchmarkError) as raised:
                RUNNER.ask_interpreter([absent, "-VV"])

            self.assertIn("PYCC_BENCH_PYTHON", str(raised.exception))
            self.assertNotIn(absent, str(raised.exception))

    def test_refuses_an_interpreter_that_exits_non_zero(self) -> None:
        with self.assertRaises(BenchmarkError) as raised:
            RUNNER.ask_interpreter([sys.executable, "-c", "raise SystemExit(3)"])

        self.assertIn("PYCC_BENCH_PYTHON", str(raised.exception))

    def test_accepts_the_pinned_version(self) -> None:
        RUNNER.assert_pinned_version("Python 3.14.7 (main, Sep 1 2026, 00:00:00) [Clang]")

    def test_refuses_any_other_version(self) -> None:
        with self.assertRaises(BenchmarkError) as raised:
            RUNNER.assert_pinned_version("Python 3.14.6 (main, Sep 1 2026, 00:00:00) [Clang]")

        self.assertIn(RUNNER.PINNED_PYTHON_VERSION, str(raised.exception))

    # `docs/TESTING.md`'s "Versions" bullet requires a GIL-enabled host, and
    # `src/ext/pycc_ext_module.c`'s `PyInit_` refuses a free-threaded one
    # outright, so the `ext` arm cannot even be imported there. Both directions
    # are driven from a fabricated `-VV` payload rather than the ambient
    # interpreter, so this test says the same thing on macOS and on Linux CI.
    def test_accepts_a_gil_enabled_interpreter(self) -> None:
        RUNNER.assert_gil_enabled("Python 3.14.7 (main, Sep 1 2026, 00:00:00) [Clang]")

    def test_refuses_a_free_threaded_interpreter(self) -> None:
        with self.assertRaises(BenchmarkError) as raised:
            RUNNER.assert_gil_enabled(
                "Python 3.14.7 free-threading build (main, Sep 1 2026, 00:00:00) [Clang]"
            )

        self.assertIn("free-threading build", str(raised.exception))

    def test_the_pinned_version_guard_alone_admits_a_free_threaded_build(self) -> None:
        # The version guard parses only the first two tokens, so it cannot be
        # the detector; this pins why the refusal lives in its own guard.
        RUNNER.assert_pinned_version(
            "Python 3.14.7 free-threading build (main, Sep 1 2026, 00:00:00) [Clang]"
        )

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

    def test_accepts_every_positive_spelling_of_the_optimization_flag(self) -> None:
        for configure_args in (
            "--enable-optimizations",
            "'--prefix=/usr/local' '--enable-optimizations' '--with-lto'",
            "--enable-optimizations=yes --with-lto",
        ):
            with self.subTest(configure_args=configure_args):
                RUNNER.assert_optimized_build(configure_args)

    def test_refuses_a_build_that_merely_avoids_the_denied_markers(self) -> None:
        # An ordinary `./configure && make` denies nothing and optimizes
        # nothing; the protocol calls such a baseline inadmissible because it
        # manufactures a passing ratio on its own.
        for configure_args in (
            "--prefix=/usr/local",
            "",
            "--enable-optimizations=no --with-lto",
            "--enable-optimizations=0",
            "--enable-shared --enable-optimizations-experiment",
        ):
            with self.subTest(configure_args=configure_args):
                with self.assertRaises(BenchmarkError) as raised:
                    RUNNER.assert_optimized_build(configure_args)
                self.assertIn(
                    RUNNER.OPTIMIZED_CONFIGURE_MARKER, str(raised.exception)
                )

    def test_refuses_a_debug_build_even_when_it_claims_optimizations(self) -> None:
        with self.assertRaises(BenchmarkError) as raised:
            RUNNER.assert_optimized_build("--enable-optimizations --with-pydebug")

        self.assertIn("--with-pydebug", str(raised.exception))


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

    def test_refuses_an_unreadable_input_without_echoing_the_path(self) -> None:
        # A directory makes the read itself fail (`IsADirectoryError`, an
        # `OSError`) on every supported host, without depending on the ambient
        # user's privileges the way a permission bit would.
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "input.bin"
            path.mkdir()

            with self.assertRaises(BenchmarkError) as raised:
                RUNNER.verify_input_digest(path, "0" * 64)

            self.assertNotIn(str(path), str(raised.exception))
            self.assertIn("--input", str(raised.exception))

    def test_refuses_a_record_that_commits_no_input_digest(self) -> None:
        # `main` reads the digest with `.get`, so a record missing the field
        # must refuse here rather than raise a bare `KeyError` past the
        # `BenchmarkError` handler.
        with self.assertRaises(BenchmarkError) as raised:
            RUNNER.verify_input_digest(Path("/nonexistent"), None)

        self.assertIn("input_sha256", str(raised.exception))

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
                self.reference(value=None, exception=ValueError),
                self.reference(value=None, exception=TypeError),
                tolerance=1e-12,
            )

    def test_refuses_two_namesake_exception_classes(self) -> None:
        # A name is not an identity: an arm that defines its own `ValueError`
        # must not satisfy the precondition by raising a namesake of the one
        # the baseline raised.
        namesake = type("ValueError", (Exception,), {})

        with self.assertRaises(BenchmarkError) as raised:
            RUNNER.compare_outcomes(
                self.reference(value=None, exception=ValueError),
                self.reference(value=None, exception=namesake),
                tolerance=1e-12,
            )

        self.assertIn("builtins.ValueError", str(raised.exception))

    def test_accepts_two_arms_that_raised_the_same_exception(self) -> None:
        RUNNER.compare_outcomes(
            self.reference(value=None, exception=ValueError),
            self.reference(value=None, exception=ValueError),
            tolerance=1e-12,
        )

    def test_refuses_an_arm_that_raises_where_the_baseline_returned(self) -> None:
        with self.assertRaises(BenchmarkError):
            RUNNER.compare_outcomes(
                self.reference(),
                self.reference(value=None, exception=ValueError),
                tolerance=1e-12,
            )

    def test_refuses_an_arm_that_clobbered_its_arguments(self) -> None:
        with self.assertRaises(BenchmarkError):
            RUNNER.compare_outcomes(
                self.reference(), self.reference(arguments_digest="b"), tolerance=1e-12
            )

    def test_refuses_an_arm_that_clobbered_its_arguments_and_then_raised(self) -> None:
        # The exception path returns before the value comparison, so the
        # arguments are the only thing left that can catch this arm.
        with self.assertRaises(BenchmarkError) as raised:
            RUNNER.compare_outcomes(
                self.reference(value=None, exception=ValueError),
                self.reference(
                    value=None, exception=ValueError, arguments_digest="b"
                ),
                tolerance=1e-12,
            )

        self.assertIn("changed its arguments", str(raised.exception))

    def test_refuses_a_non_finite_float_against_a_finite_baseline(self) -> None:
        # `abs(1.0 - nan) > allowed` is false, so nothing but an explicit
        # refusal keeps a NaN out of the tolerance test.
        for divergent in (float("nan"), float("inf"), float("-inf")):
            with self.subTest(divergent=divergent):
                with self.assertRaises(BenchmarkError) as raised:
                    RUNNER.compare_outcomes(
                        self.reference(value=1.0),
                        self.reference(value=divergent),
                        tolerance=1e-9,
                    )
                self.assertIn("non-finite", str(raised.exception))

    def test_accepts_a_divergence_within_the_relative_tolerance(self) -> None:
        reference = RUNNER.ArmOutcome(1e6, None, "digest")
        candidate = RUNNER.ArmOutcome(1e6 + 1e-4, None, "digest")

        RUNNER.compare_outcomes(reference, candidate, 1e-9)

    def test_rejects_a_divergence_beyond_the_relative_tolerance(self) -> None:
        reference = RUNNER.ArmOutcome(1e6, None, "digest")
        candidate = RUNNER.ArmOutcome(1e6 + 1.0, None, "digest")

        with self.assertRaises(BenchmarkError):
            RUNNER.compare_outcomes(reference, candidate, 1e-9)

    def test_refuses_two_non_finite_floats_that_compare_equal(self) -> None:
        for value in (float("nan"), float("inf")):
            with self.subTest(value=value):
                with self.assertRaises(BenchmarkError):
                    RUNNER.compare_outcomes(
                        self.reference(value=value),
                        self.reference(value=value),
                        tolerance=1e-9,
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
        record = committed_record()

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

    # `json.loads` turns `1e999` into infinity, and `compare_outcomes` tests
    # `difference > allowed`: a tolerance that is not a finite, non-negative
    # number bounds nothing, so the record is refused rather than scored.
    def test_accepts_the_committed_float_tolerance(self) -> None:
        self.assertEqual(
            RUNNER.read_float_tolerance(committed_record()),
            committed_record()["float_tolerance"],
        )

    def test_refuses_an_infinite_float_tolerance(self) -> None:
        record = committed_record() | {"float_tolerance": json.loads("1e999")}

        with self.assertRaises(BenchmarkError) as raised:
            RUNNER.read_float_tolerance(record)

        self.assertIn("finite", str(raised.exception))

    def test_refuses_a_negative_float_tolerance(self) -> None:
        with self.assertRaises(BenchmarkError):
            RUNNER.read_float_tolerance(committed_record() | {"float_tolerance": -1e-09})

    def test_refuses_a_float_tolerance_that_is_not_a_number(self) -> None:
        for value in ("1e-09", None, True):
            with self.subTest(value=value):
                with self.assertRaises(BenchmarkError) as raised:
                    RUNNER.read_float_tolerance(committed_record() | {"float_tolerance": value})

                self.assertIn("number", str(raised.exception))

    def test_refuses_a_record_with_no_float_tolerance(self) -> None:
        record = committed_record()
        del record["float_tolerance"]

        with self.assertRaises(BenchmarkError):
            RUNNER.read_float_tolerance(record)

    def test_record_carries_no_result(self) -> None:
        record = committed_record()

        for field in ("median_ns", "ratio", "speedup", "compile_unchanged_count"):
            with self.subTest(field=field):
                self.assertNotIn(field, record)

    def test_machine_identity_carries_the_committed_shape(self) -> None:
        record = committed_record()

        for field in ("model_identifier", "cpu", "cores", "memory_bytes", "os", "power_profile"):
            with self.subTest(field=field):
                self.assertIn(field, record["machine"])

    def test_input_digest_is_a_sha256_hex_string(self) -> None:
        record = committed_record()

        self.assertRegex(record["input_sha256"], r"\A[0-9a-f]{64}\Z")
        self.assertRegex(record["compile_unchanged_set_sha256"], r"\A[0-9a-f]{64}\Z")




class TimingBoundaryTest(unittest.TestCase):
    """`time_call` and `run_arm` are the timing boundary itself."""

    def test_times_the_call_and_reports_its_value(self) -> None:
        elapsed, outcome = RUNNER.time_call(lambda left, right: left + right, 2, 3)

        self.assertGreaterEqual(elapsed, 0)
        self.assertEqual(outcome.value, 5)
        self.assertIsNone(outcome.exception)
        self.assertEqual(
            outcome.arguments_digest,
            hashlib.sha256(repr((2, 3)).encode("utf-8")).hexdigest(),
        )

    def test_records_the_exception_type_instead_of_propagating_it(self) -> None:
        def raises(value: int) -> int:
            raise ZeroDivisionError("arm failed")

        elapsed, outcome = RUNNER.time_call(raises, 7)

        self.assertGreaterEqual(elapsed, 0)
        self.assertIsNone(outcome.value)
        self.assertIs(outcome.exception, ZeroDivisionError)
        self.assertEqual(
            outcome.arguments_digest,
            hashlib.sha256(repr((7,)).encode("utf-8")).hexdigest(),
        )

    def test_digests_the_arguments_after_a_call_that_mutated_and_then_raised(self) -> None:
        def clobber_then_raise(values: list[int]) -> int:
            values.append(99)
            raise ZeroDivisionError("arm failed")

        def just_raise(values: list[int]) -> int:
            raise ZeroDivisionError("arm failed")

        baseline_arguments = [1, 2, 3]
        _, baseline = RUNNER.time_call(just_raise, baseline_arguments)
        candidate_arguments = [1, 2, 3]
        _, candidate = RUNNER.time_call(clobber_then_raise, candidate_arguments)

        self.assertEqual(candidate_arguments, [1, 2, 3, 99])
        self.assertEqual(
            candidate.arguments_digest,
            hashlib.sha256(repr(([1, 2, 3, 99],)).encode("utf-8")).hexdigest(),
        )
        self.assertNotEqual(baseline.arguments_digest, candidate.arguments_digest)
        with self.assertRaises(BenchmarkError):
            RUNNER.compare_outcomes(baseline, candidate, tolerance=1e-9)

    def test_runs_one_untimed_warm_up_and_seven_timed_replicates(self) -> None:
        calls: list[int] = []

        def counted(value: int) -> int:
            calls.append(len(calls))
            return value * 2

        summary, outcome = RUNNER.run_arm("ext", counted, lambda: (21,), 1e-9)

        self.assertEqual(len(calls), RUNNER.WARMUP_RUNS + RUNNER.REPLICATES)
        self.assertEqual(summary["arm"], "ext")
        self.assertEqual(summary["replicates"], RUNNER.REPLICATES)
        # The cross-arm correctness datum comes from the warm-up, which no
        # reported timing contains; the timed calls are validated against it.
        self.assertEqual(outcome.value, 42)

    def test_refuses_to_run_an_arm_without_a_warm_up(self) -> None:
        original = RUNNER.WARMUP_RUNS
        RUNNER.WARMUP_RUNS = 0
        try:
            with self.assertRaises(BenchmarkError):
                RUNNER.run_arm("ext", lambda value: value, lambda: (1,), 1e-9)
        finally:
            RUNNER.WARMUP_RUNS = original


class ValidatedReplicateTest(unittest.TestCase):
    """Finding: a median is admissible only over calls that were validated."""

    def test_refuses_an_arm_that_misbehaves_only_on_the_timed_calls(self) -> None:
        calls: list[int] = []

        def right_once(value: int) -> int:
            calls.append(value)
            return 1 if len(calls) == 1 else 2

        with self.assertRaises(BenchmarkError) as raised:
            RUNNER.run_arm("ext", right_once, lambda: (1,), 1e-9)

        self.assertIn("replicate 1", str(raised.exception))
        self.assertIn("warm-up", str(raised.exception))

    def test_refuses_an_arm_that_raises_only_on_the_timed_calls(self) -> None:
        calls: list[int] = []

        def raise_after_warm_up(value: int) -> int:
            calls.append(value)
            if len(calls) > 1:
                raise ZeroDivisionError("arm failed")
            return 1

        with self.assertRaises(BenchmarkError):
            RUNNER.run_arm("ext", raise_after_warm_up, lambda: (1,), 1e-9)

    def test_refuses_an_arm_that_clobbers_its_arguments_only_when_timed(self) -> None:
        calls: list[int] = []

        def clobber_after_warm_up(values: list[int]) -> int:
            calls.append(len(values))
            if len(calls) > 1:
                values.append(99)
            return 1

        with self.assertRaises(BenchmarkError) as raised:
            RUNNER.run_arm("ext", clobber_after_warm_up, lambda: ([1, 2, 3],), 1e-9)

        self.assertIn("changed its arguments", str(raised.exception))

    def test_admits_an_arm_that_answers_every_timed_call(self) -> None:
        summary, _ = RUNNER.run_arm("ext", lambda value: value + 1, lambda: (1,), 1e-9)

        self.assertEqual(summary["replicates"], RUNNER.REPLICATES)


class FreshArgumentsTest(unittest.TestCase):
    """Finding: an arm that mutates its arguments must not time modified data."""

    def test_every_invocation_receives_freshly_built_arguments(self) -> None:
        seen: list[list[int]] = []

        def mutating(values: list[int]) -> int:
            seen.append(list(values))
            values.append(len(values))
            return 1

        RUNNER.run_arm("ext", mutating, lambda: ([1, 2, 3],), 1e-9)

        self.assertEqual(len(seen), RUNNER.WARMUP_RUNS + RUNNER.REPLICATES)
        # Each call saw the committed input, not the previous call's leftovers.
        for observed in seen:
            self.assertEqual(observed, [1, 2, 3])

    def test_the_factory_is_called_once_per_invocation(self) -> None:
        built = []

        def make_arguments() -> tuple:
            built.append(object())
            return (1,)

        RUNNER.run_arm("ext", lambda value: value, make_arguments, 1e-9)

        self.assertEqual(len(built), RUNNER.WARMUP_RUNS + RUNNER.REPLICATES)


class CommittedRecordTest(unittest.TestCase):
    """Finding: a pre-registration record is binding only if it is committed."""

    def test_accepts_bytes_equal_to_the_committed_blob(self) -> None:
        RUNNER.assert_record_is_committed(b'{"seed": 1}\n', b'{"seed": 1}\n')

    def test_refuses_bytes_that_differ_from_the_committed_blob(self) -> None:
        with self.assertRaises(BenchmarkError) as raised:
            RUNNER.assert_record_is_committed(b'{"seed": 2}\n', b'{"seed": 1}\n')

        self.assertIn(RUNNER.PRE_REGISTRATION_RELATIVE_PATH, str(raised.exception))

    def test_refuses_a_record_differing_only_in_trailing_whitespace(self) -> None:
        # Raw bytes, never a stripped text read: otherwise a rewritten record
        # could pass as the committed one.
        with self.assertRaises(BenchmarkError):
            RUNNER.assert_record_is_committed(b'{"seed": 1}\n\n', b'{"seed": 1}\n')

    def test_reads_the_committed_blob_from_a_repository(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            target = root / RUNNER.PRE_REGISTRATION_RELATIVE_PATH
            target.parent.mkdir(parents=True)
            target.write_bytes(b'{"seed": 1}\n')
            self.init_repository(root)

            self.assertEqual(
                RUNNER.read_committed_blob(root, RUNNER.PRE_REGISTRATION_RELATIVE_PATH),
                b'{"seed": 1}\n',
            )

    def test_refuses_a_path_that_is_not_committed(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "placeholder").write_text("x", encoding="utf-8")
            self.init_repository(root)

            with self.assertRaises(BenchmarkError):
                RUNNER.read_committed_blob(root, RUNNER.PRE_REGISTRATION_RELATIVE_PATH)

    def test_reading_the_record_refuses_an_uncommitted_rewrite(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            target = root / RUNNER.PRE_REGISTRATION_RELATIVE_PATH
            target.parent.mkdir(parents=True)
            target.write_bytes(b'{"seed": 1}\n')
            self.init_repository(root)
            # The run rewrites the record after seeing a result.
            target.write_bytes(b'{"seed": 2}\n')

            with self.assertRaises(BenchmarkError):
                RUNNER.read_pre_registration(target, root)

    def test_reading_the_record_accepts_the_committed_bytes(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            target = root / RUNNER.PRE_REGISTRATION_RELATIVE_PATH
            target.parent.mkdir(parents=True)
            target.write_bytes(b'{"seed": 1}\n')
            self.init_repository(root)

            self.assertEqual(RUNNER.read_pre_registration(target, root), {"seed": 1})

    def test_refuses_a_record_that_cannot_be_read(self) -> None:
        # Unreadable is not "absent": without this the read raises a bare
        # `OSError` past `main`'s `BenchmarkError` handler.
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            target = root / RUNNER.PRE_REGISTRATION_RELATIVE_PATH
            target.mkdir(parents=True)

            with self.assertRaises(BenchmarkError) as raised:
                RUNNER.read_pre_registration(target, root)

            self.assertIn("pre-registration record", str(raised.exception))

    def init_repository(self, root: Path) -> None:
        """A hermetic repository, never the ambient worktree."""

        environment = {
            "PATH": os.environ.get("PATH", ""),
            "HOME": str(root),
            "GIT_CONFIG_GLOBAL": str(root / "gitconfig-absent"),
            "GIT_CONFIG_SYSTEM": str(root / "gitconfig-absent"),
        }
        for command in (
            ["git", "init", "-q", "-b", "main"],
            ["git", "add", "-A"],
            [
                "git",
                "-c",
                "user.email=bench@example.invalid",
                "-c",
                "user.name=bench",
                "-c",
                "commit.gpgsign=false",
                "commit",
                "-q",
                "-m",
                "pre-registration",
            ],
        ):
            subprocess.run(command, cwd=root, check=True, env=environment, capture_output=True)


class MachineIdentityTest(unittest.TestCase):
    """Finding: the run must happen on the pre-registered machine."""

    def committed(self, **overrides: object) -> dict:
        machine = {
            "model_identifier": "Mac15,9",
            "cpu": "Apple M3 Max",
            "cores": 16,
            "memory_bytes": 137438953472,
            "os": "macOS 26.5.1 (build 25F80)",
            "power_profile": "AC power",
        }
        machine.update(overrides)
        return machine

    def observed(self) -> dict:
        machine = self.committed()
        del machine["power_profile"]
        return machine

    def test_accepts_the_pre_registered_machine(self) -> None:
        RUNNER.compare_machine(self.observed(), self.committed())

    def test_refuses_every_observable_field_that_differs(self) -> None:
        for field, value in (
            ("model_identifier", "Mac16,1"),
            ("cpu", "Apple M4 Max"),
            ("cores", 24),
            ("memory_bytes", 274877906944),
            ("os", "macOS 26.6.0 (build 25G1)"),
        ):
            with self.subTest(field=field):
                with self.assertRaises(BenchmarkError) as raised:
                    RUNNER.compare_machine(self.observed(), self.committed(**{field: value}))
                self.assertIn(field, str(raised.exception))

    def test_ignores_the_field_no_host_reports(self) -> None:
        # `power_profile` is an operator assertion; nothing the host exposes
        # reproduces it, so it is restated in the report rather than checked.
        RUNNER.compare_machine(self.observed(), self.committed(power_profile="battery"))

    def test_refuses_a_record_with_no_machine_identity(self) -> None:
        for committed in (None, "Mac15,9"):
            with self.subTest(committed=committed):
                with self.assertRaises(BenchmarkError):
                    RUNNER.compare_machine(self.observed(), committed)

    def test_refuses_a_record_missing_an_observable_field(self) -> None:
        committed = self.committed()
        del committed["cpu"]

        with self.assertRaises(BenchmarkError) as raised:
            RUNNER.compare_machine(self.observed(), committed)

        self.assertIn("cpu", str(raised.exception))

    @unittest.skipUnless(
        sys.platform == "darwin",
        "`observe_machine` reads `sysctl`/`sw_vers`, which only a macOS host answers; "
        "the pre-registered machine is a Mac, so refusing every other host is the "
        "behaviour `test_refuses_a_host_that_does_not_answer` pins portably.",
    )
    def test_observes_this_host_in_the_committed_shape(self) -> None:
        observed = RUNNER.observe_machine()

        for field in RUNNER.OBSERVABLE_MACHINE_FIELDS:
            self.assertIn(field, observed)
        self.assertIsInstance(observed["cores"], int)
        self.assertIsInstance(observed["memory_bytes"], int)

    def test_refuses_a_host_that_does_not_answer(self) -> None:
        def failing(command, **kwargs):
            return subprocess.CompletedProcess(command, 1, stdout="", stderr="")

        with self.assertRaises(BenchmarkError):
            RUNNER.observe_machine(failing)


class SubjectDigestTest(unittest.TestCase):
    """Finding: the timed subject must be the pre-registered reference source."""

    SOURCE = b"def hot(a: float) -> float:\n    return a\n"

    def subject(self, directory: str, source: bytes | None = None) -> Path:
        path = Path(directory) / "hot_loop.py"
        path.write_bytes(self.SOURCE if source is None else source)
        return path

    def test_accepts_and_returns_the_pre_registered_source(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            subject = self.subject(directory)

            self.assertEqual(
                RUNNER.read_subject_source(subject, hashlib.sha256(self.SOURCE).hexdigest()),
                self.SOURCE,
            )

    def test_refuses_a_substituted_subject(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            subject = self.subject(directory, b"def hot(a):\n    return 0.0\n")

            with self.assertRaises(BenchmarkError) as raised:
                RUNNER.read_subject_source(subject, hashlib.sha256(self.SOURCE).hexdigest())

            self.assertIn("SHA-256", str(raised.exception))

    def test_refuses_an_unregistered_subject_digest(self) -> None:
        # The committed record carries `null` until the publishing run
        # registers the digest in its own stage commit.
        with tempfile.TemporaryDirectory() as directory:
            subject = self.subject(directory)

            for expected in (None, "", "not-a-digest", "A" * 64, 7):
                with self.subTest(expected=expected):
                    with self.assertRaises(BenchmarkError) as raised:
                        RUNNER.read_subject_source(subject, expected)
                    self.assertIn("subject_sha256", str(raised.exception))

    def test_refuses_an_unreadable_subject_without_echoing_the_path(self) -> None:
        # The read can fail after `resolve_subject` saw a file -- a permission
        # change, or the path becoming a directory. An uncaught `OSError` would
        # print the proprietary path, so the refusal is raised here instead. A
        # directory reproduces that failure without touching the ambient host's
        # privileges.
        with tempfile.TemporaryDirectory() as directory:
            subject = Path(directory) / "proprietary_hot_loop.py"
            subject.mkdir()

            with self.assertRaises(BenchmarkError) as raised:
                RUNNER.read_subject_source(subject, hashlib.sha256(self.SOURCE).hexdigest())

            self.assertNotIn(str(subject), str(raised.exception))
            self.assertNotIn("proprietary_hot_loop", str(raised.exception))
            self.assertIn("PYCC_BENCH_SUBJECT", str(raised.exception))

    def test_no_refusal_echoes_the_proprietary_path_or_source(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            subject = self.subject(directory, b"def proprietary_secret_name():\n    pass\n")

            with self.assertRaises(BenchmarkError) as raised:
                RUNNER.read_subject_source(subject, hashlib.sha256(self.SOURCE).hexdigest())

            self.assertNotIn(str(subject), str(raised.exception))
            self.assertNotIn("proprietary_secret_name", str(raised.exception))


class UnregisteredSubjectRecordTest(unittest.TestCase):
    def test_the_committed_record_registers_a_digest_or_declares_it_pending(self) -> None:
        record = committed_record()
        digest = record["subject_sha256"]

        self.assertTrue(
            digest is None or re.fullmatch(r"[0-9a-f]{64}", digest),
            "subject_sha256 is either a registered SHA-256 or null",
        )
        self.assertIn("subject_sha256_rule", record)


if __name__ == "__main__":
    unittest.main()
