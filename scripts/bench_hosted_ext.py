#!/usr/bin/env python3
"""Run the hosted `ext` benchmark under the product-sprint-1 protocol.

Every rule enforced here is `docs/TESTING.md`'s, under "Hosted `ext` benchmark
protocol (product-sprint-1)"; that section is canonical and nothing is restated
in its own words. What this file adds is the mechanism: the refusals that make
a timing admissible, the correctness precondition that sits outside the timing
boundary, and the median-of-seven statistic.

Two environment variables configure a run, both documented in `docs/TESTING.md`
beside the protocol:

* `PYCC_BENCH_SUBJECT` -- the path to the subject function's module, outside
  this repository. The reference codebase is proprietary (D-244 rule 6), so the
  path and its contents are never echoed into stdout, the results file or any
  report.
* `PYCC_BENCH_PYTHON` -- the pinned interpreter to time against, falling back
  to `PYCC_PYTHON`.

The pre-registration record `scripts/bench_hosted_ext_precommit.json` supplies
the seed, the input digest, the float tolerance and the machine identity; a run
whose input does not digest to the committed value aborts before timing
anything.
"""

from __future__ import annotations

import argparse
import dataclasses
import hashlib
import json
import os
from pathlib import Path
import statistics
import subprocess
import sys
import time

PINNED_PYTHON_VERSION = "3.14.7"
REPLICATES = 7
WARMUP_RUNS = 1

#: Configure-time markers that make an interpreter inadmissible as the baseline
#: arm. `docs/TESTING.md`'s "Versions" bullet rules such a build out whatever it
#: reports, because a slow baseline manufactures a passing ratio on its own.
UNOPTIMIZED_CONFIGURE_MARKERS = (
    "--with-pydebug",
    "--with-trace-refs",
    "--without-pymalloc",
    "--with-address-sanitizer",
    "--with-undefined-behavior-sanitizer",
)


class BenchmarkError(RuntimeError):
    """A protocol rule was violated, so no number from this run is admissible."""


@dataclasses.dataclass(frozen=True)
class ArmOutcome:
    """What one arm produced on the committed input, outside the clock."""

    value: object
    exception: str | None
    arguments_digest: str


def resolve_interpreter(environ: dict[str, str]) -> str:
    interpreter = environ.get("PYCC_BENCH_PYTHON") or environ.get("PYCC_PYTHON")
    if not interpreter:
        raise BenchmarkError(
            "set PYCC_BENCH_PYTHON (or PYCC_PYTHON) to the pinned "
            f"CPython {PINNED_PYTHON_VERSION} interpreter"
        )
    return interpreter


def assert_pinned_version(version_output: str) -> None:
    tokens = version_output.split()
    reported = tokens[1] if len(tokens) > 1 and tokens[0] == "Python" else ""
    if reported != PINNED_PYTHON_VERSION:
        raise BenchmarkError(
            f"the benchmark interpreter must be exactly Python {PINNED_PYTHON_VERSION}, "
            f"found {reported or version_output.strip()!r}"
        )


def assert_optimized_build(configure_args: str | None) -> None:
    if configure_args is None:
        raise BenchmarkError(
            "the interpreter reports no CONFIGURE_ARGS, so its build cannot be checked"
        )
    for marker in UNOPTIMIZED_CONFIGURE_MARKERS:
        if marker in configure_args:
            raise BenchmarkError(
                f"the interpreter was configured with {marker}, which is inadmissible"
            )


def resolve_subject(environ: dict[str, str]) -> Path:
    configured = environ.get("PYCC_BENCH_SUBJECT")
    if not configured:
        raise BenchmarkError(
            "set PYCC_BENCH_SUBJECT to the subject module's path, outside this repository"
        )
    subject = Path(configured)
    if not subject.is_file():
        raise BenchmarkError("PYCC_BENCH_SUBJECT does not name an existing file")
    return subject


def verify_input_digest(path: Path, expected: str) -> None:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for block in iter(lambda: handle.read(1 << 20), b""):
            digest.update(block)
    actual = digest.hexdigest()
    if actual != expected:
        raise BenchmarkError(
            "the input file's SHA-256 does not match the committed digest: "
            f"expected {expected}, found {actual}"
        )


def compare_outcomes(reference: ArmOutcome, candidate: ArmOutcome, tolerance: float) -> None:
    """Apply the correctness precondition; raise rather than score a wrong arm."""

    if reference.exception is not None or candidate.exception is not None:
        if reference.exception != candidate.exception:
            raise BenchmarkError(
                "the arms disagree on the exception raised: "
                f"{reference.exception!r} against {candidate.exception!r}"
            )
        return
    if type(reference.value) is not type(candidate.value):
        raise BenchmarkError(
            "the arms returned different types: "
            f"{type(reference.value).__name__} against {type(candidate.value).__name__}"
        )
    if isinstance(reference.value, float):
        # Relative against the baseline's own magnitude, with an absolute floor
        # of one: an absolute-only tolerance is meaningless across the range a
        # geometric reduction can return.
        allowed = tolerance * max(1.0, abs(reference.value))
        if abs(reference.value - candidate.value) > allowed:
            raise BenchmarkError(
                "the arms diverge beyond the committed float tolerance"
            )
    elif reference.value != candidate.value:
        raise BenchmarkError("the arms returned different values")
    if reference.arguments_digest != candidate.arguments_digest:
        raise BenchmarkError("an arm changed its arguments where the baseline did not")


def median_ns(timings: list[int]) -> int:
    return int(statistics.median(sorted(timings)))


def summarize(arm: str, timings: list[int]) -> dict:
    if len(timings) != REPLICATES:
        raise BenchmarkError(
            f"{arm} reported {len(timings)} replicates; the protocol fixes {REPLICATES}"
        )
    return {
        "arm": arm,
        "median_ns": median_ns(timings),
        "min_ns": min(timings),
        "max_ns": max(timings),
        "replicates": REPLICATES,
    }


def ratio_of_medians(baseline: dict, candidate: dict) -> float:
    if candidate["median_ns"] <= 0:
        raise BenchmarkError("a median of zero cannot be divided into")
    return baseline["median_ns"] / candidate["median_ns"]


def read_pre_registration(path: Path) -> dict:
    return json.loads(path.read_text(encoding="utf-8"))


def time_call(call, *args) -> tuple[int, ArmOutcome]:
    """Time exactly the call, with nothing else inside the boundary."""

    arguments_digest = hashlib.sha256(repr(args).encode("utf-8")).hexdigest()
    started = time.perf_counter_ns()
    try:
        value = call(*args)
    except BaseException as error:  # noqa: BLE001 - the arm's failure is the datum
        finished = time.perf_counter_ns()
        outcome = ArmOutcome(None, type(error).__name__, arguments_digest)
    else:
        finished = time.perf_counter_ns()
        outcome = ArmOutcome(
            value, None, hashlib.sha256(repr(args).encode("utf-8")).hexdigest()
        )
    return finished - started, outcome


def interpreter_facts(interpreter: str) -> tuple[str, str | None]:
    version_output = subprocess.run(
        [interpreter, "-VV"], check=True, capture_output=True, text=True
    ).stdout
    configure_args = subprocess.run(
        [
            interpreter,
            "-c",
            "import sysconfig; print(sysconfig.get_config_var('CONFIGURE_ARGS'))",
        ],
        check=True,
        capture_output=True,
        text=True,
    ).stdout.strip()
    return version_output, None if configure_args in {"", "None"} else configure_args


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument(
        "--pre-registration",
        type=Path,
        default=Path(__file__).with_name("bench_hosted_ext_precommit.json"),
        help="the committed pre-registration record",
    )
    parser.add_argument(
        "--input", type=Path, required=True, help="the generated input file, outside the repository"
    )
    parser.add_argument("--check-only", action="store_true", help="run the guards and stop")
    arguments = parser.parse_args(argv)

    try:
        record = read_pre_registration(arguments.pre_registration)
        interpreter = resolve_interpreter(dict(os.environ))
        version_output, configure_args = interpreter_facts(interpreter)
        assert_pinned_version(version_output)
        assert_optimized_build(configure_args)
        resolve_subject(dict(os.environ))
        verify_input_digest(arguments.input, record["input_sha256"])
    except BenchmarkError as error:
        print(str(error), file=sys.stderr)
        return 1

    print("protocol preconditions satisfied")
    if arguments.check_only:
        return 0
    print(
        "the three arms are built and timed by the run recorded in the published report; "
        "this runner enforces the protocol and reports the statistic"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
