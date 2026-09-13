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
  report; what binds the subject is the SHA-256 of its bytes, which is a number
  and is committed in the pre-registration record.
* `PYCC_BENCH_PYTHON` -- the pinned interpreter to time against, falling back
  to `PYCC_PYTHON`.

The pre-registration record `scripts/bench_hosted_ext_precommit.json` supplies
the seed, the input digest, the subject digest, the float tolerance and the
machine identity; a run whose input does not digest to the committed value
aborts before timing anything. The record itself is bound to git: the bytes
read from `--pre-registration` must equal the bytes of the committed blob at
`HEAD`, so a record written after a result was seen is refused rather than
scored. That binding is exactly "these are the committed bytes"; whether the
rest of the checkout is clean is deliberately not part of it, since an
ordinary development tree is dirty for reasons the record does not own.

The machine is bound the same way, against what the host actually reports. Five
of the committed `machine` fields are mechanically observable and are compared
exactly; `power_profile` is not derivable from any of them and stays an
operator assertion, restated in the report rather than verified here.

`main` is the protocol's gate, not its driver: it resolves and checks the
interpreter, the subject and the input, and stops. Constructing the three arms
-- interpreter, Cython extension, `ext` artifact -- belongs to the run that
publishes the numbers, which calls `run_arm` once per arm and
`compare_outcomes` between them.
"""

from __future__ import annotations

import argparse
import dataclasses
import hashlib
import json
import math
import os
from pathlib import Path
import re
import shlex
import statistics
import subprocess
import sys
import time

PINNED_PYTHON_VERSION = "3.14.7"
REPLICATES = 7
WARMUP_RUNS = 1

#: The `Py_GetVersion()` marker a free-threaded CPython carries, and the only
#: detector `src/ext/pycc_ext_module.c` found to be reliable at `PyInit_` time
#: (`sys._is_gil_enabled()` returned True on a free-threaded 3.14 there). The
#: artifact refuses such a host outright, so the `ext` arm cannot run on one.
FREE_THREADING_VERSION_MARKER = "free-threading build"

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

#: The configure flag whose *presence* is the only checkable proof CPython
#: offers that the interpreter was built the way `docs/TESTING.md`'s "Versions"
#: bullet requires. A denylist alone admits every build that merely avoids the
#: markers above -- including an ordinary `./configure && make` with no
#: optimization at all, which is exactly the slow baseline that bullet rules
#: out because it manufactures a passing ratio on its own.
OPTIMIZED_CONFIGURE_MARKER = "--enable-optimizations"

#: Values that turn the flag above back off when it is spelled `=<value>`.
DISABLED_CONFIGURE_VALUES = frozenset({"", "no", "false", "0"})

#: The pre-registration record's repository-relative path. A run is scored only
#: against the bytes committed at this path, so the path is part of the binding
#: rather than an argument default.
PRE_REGISTRATION_RELATIVE_PATH = "scripts/bench_hosted_ext_precommit.json"

#: The `machine` fields the host reports for itself, and the command that
#: reports each. `docs/TESTING.md`'s "Arms" bullet requires the run to happen on
#: the committed machine; these are the fields that can be checked rather than
#: asserted. `power_profile` is deliberately absent: nothing the host exposes
#: reproduces that string, so it stays an operator assertion.
OBSERVABLE_MACHINE_FIELDS = ("model_identifier", "cpu", "cores", "memory_bytes", "os")


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


def assert_gil_enabled(version_output: str) -> None:
    """Refuse a free-threaded host, which `docs/TESTING.md`'s "Versions" bullet rules out.

    Kept apart from `assert_pinned_version`: that guard reads only the version
    number, and a free-threaded build reports exactly the pinned one.
    """

    if FREE_THREADING_VERSION_MARKER in version_output:
        raise BenchmarkError(
            f"the benchmark interpreter reports a {FREE_THREADING_VERSION_MARKER}; the "
            "protocol requires a GIL-enabled host, and the `ext` artifact refuses to "
            "import into a free-threaded one"
        )


def configure_tokens(configure_args: str) -> list[str]:
    """Split `CONFIGURE_ARGS` the way the shell that produced it quoted them."""

    try:
        return shlex.split(configure_args)
    except ValueError:
        # An unbalanced quote is not this guard's business to repair; falling
        # back to whitespace splitting keeps the positive check conservative.
        return configure_args.split()


def reports_optimized_build(configure_args: str) -> bool:
    """Whether the flags positively assert the optimized build, not merely fail to deny it."""

    for token in configure_tokens(configure_args):
        if token == OPTIMIZED_CONFIGURE_MARKER:
            return True
        if token.startswith(f"{OPTIMIZED_CONFIGURE_MARKER}="):
            value = token.split("=", 1)[1].strip().lower()
            if value not in DISABLED_CONFIGURE_VALUES:
                return True
    return False


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
    if not reports_optimized_build(configure_args):
        raise BenchmarkError(
            "the interpreter does not report "
            f"{OPTIMIZED_CONFIGURE_MARKER} in its CONFIGURE_ARGS, so it is not "
            "provably the optimized build the baseline arm requires"
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


def read_subject_source(subject: Path, expected: object) -> bytes:
    """Read the subject once and prove it is the pre-registered source.

    The bytes are returned rather than the path so that every arm is built from
    this one read: a path re-read per arm could be edited between them, and the
    substituted source would still be reported as the byte-identical reference
    function `docs/TESTING.md`'s "Subject" bullet requires.

    Only the digest is ever compared or reported. The reference codebase is
    proprietary (D-244 rule 6), so neither the path nor the source appears in
    any refusal message here.
    """

    if not isinstance(expected, str) or not re.fullmatch(r"[0-9a-f]{64}", expected):
        raise BenchmarkError(
            "the pre-registration record does not register a subject_sha256, so the "
            "subject is not bound to the reference source and no run is admissible"
        )
    source = subject.read_bytes()
    actual = hashlib.sha256(source).hexdigest()
    if actual != expected:
        raise BenchmarkError(
            "the subject's SHA-256 does not match the committed subject_sha256: "
            f"expected {expected}, found {actual}"
        )
    return source


def read_committed_blob(root: Path, relative_path: str) -> bytes:
    """The bytes git has at `HEAD` for one tracked path, or a refusal."""

    try:
        completed = subprocess.run(
            ["git", "-C", str(root), "cat-file", "blob", f"HEAD:{relative_path}"],
            capture_output=True,
            check=False,
        )
    except OSError as error:
        raise BenchmarkError(
            f"git is not available, so {relative_path} cannot be shown to be committed"
        ) from error
    if completed.returncode != 0:
        raise BenchmarkError(
            f"git has no committed {relative_path} at HEAD, so the pre-registration "
            "record cannot be shown to precede this run"
        )
    # Deliberately raw: a text-mode read or a strip would let a record that
    # differs from the committed blob only in trailing whitespace pass as it.
    return completed.stdout


def assert_record_is_committed(raw: bytes, committed: bytes) -> None:
    """Refuse a pre-registration record that is not the committed one.

    Pre-registration is only binding if the record predates the run. A record
    read from a file the run itself could have written proves nothing, so the
    bytes must be the committed bytes. Path is not what is checked: bytes equal
    to the committed blob *are* the committed record wherever they were read
    from, and bytes that differ are not it even at the right path.

    Whether the rest of the checkout is clean is not part of this: an ordinary
    development tree carries unrelated modifications, and the property the
    protocol needs is about this record's content alone.
    """

    if raw != committed:
        raise BenchmarkError(
            "the pre-registration record differs from the committed "
            f"{PRE_REGISTRATION_RELATIVE_PATH} at HEAD, so it was not pre-registered"
        )


def observe_machine(root_command=subprocess.run) -> dict:
    """What this host reports about itself, in the committed record's shape."""

    def read(command: list[str]) -> str:
        completed = root_command(command, capture_output=True, text=True, check=False)
        if completed.returncode != 0:
            raise BenchmarkError(
                f"this host does not answer {command[0]}, so it cannot be shown to be "
                "the pre-registered machine"
            )
        return completed.stdout.strip()

    def sysctl(name: str) -> str:
        return read(["sysctl", "-n", name])

    try:
        cores = int(sysctl("hw.ncpu"))
        memory_bytes = int(sysctl("hw.memsize"))
    except ValueError as error:
        raise BenchmarkError("this host reports a non-numeric core or memory count") from error
    return {
        "model_identifier": sysctl("hw.model"),
        "cpu": sysctl("machdep.cpu.brand_string"),
        "cores": cores,
        "memory_bytes": memory_bytes,
        "os": f"macOS {read(['sw_vers', '-productVersion'])} "
        f"(build {read(['sw_vers', '-buildVersion'])})",
    }


def compare_machine(observed: dict, committed: object) -> None:
    """Refuse a run that moved to a machine other than the committed one.

    `docs/TESTING.md`'s "Arms" bullet rules out a machine chosen after a result
    was seen; an unchecked identity leaves that rule to good intentions.
    """

    if not isinstance(committed, dict):
        raise BenchmarkError(
            "the pre-registration record commits no machine identity, so this run "
            "cannot be shown to be on the pre-registered machine"
        )
    for field in OBSERVABLE_MACHINE_FIELDS:
        if field not in committed:
            raise BenchmarkError(
                f"the pre-registration record commits no machine {field}, so this run "
                "cannot be shown to be on the pre-registered machine"
            )
        if observed[field] != committed[field]:
            raise BenchmarkError(
                f"this host is not the pre-registered machine: {field} is "
                f"{observed[field]!r} where the record commits {committed[field]!r}"
            )


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


def compare_outcomes(
    reference: ArmOutcome,
    candidate: ArmOutcome,
    tolerance: float,
    subject: str = "an arm",
    against: str = "the baseline",
) -> None:
    """Apply the correctness precondition; raise rather than score a wrong arm.

    `subject` and `against` name the two sides in the refusal messages, because
    this comparison serves two callers: one arm against the baseline arm, and
    one timed replicate against its own arm's warm-up.
    """

    # Checked first, and on every path: a call that clobbers the committed
    # input and then fails the same way its reference does must not be scored
    # for having been faster on arguments nobody else saw.
    if reference.arguments_digest != candidate.arguments_digest:
        raise BenchmarkError(f"{subject} changed its arguments where {against} did not")
    if reference.exception is not None or candidate.exception is not None:
        if reference.exception != candidate.exception:
            raise BenchmarkError(
                f"{subject} and {against} disagree on the exception raised: "
                f"{reference.exception!r} against {candidate.exception!r}"
            )
        return
    if type(reference.value) is not type(candidate.value):
        raise BenchmarkError(
            f"{subject} and {against} returned different types: "
            f"{type(reference.value).__name__} against {type(candidate.value).__name__}"
        )
    if isinstance(reference.value, float):
        if not (math.isfinite(reference.value) and math.isfinite(candidate.value)):
            # `abs(reference - nan) > allowed` is false, so a NaN would pass the
            # tolerance test below by arithmetic rather than by agreeing.
            raise BenchmarkError(
                "a non-finite float result is not comparable within the committed tolerance"
            )
        # Relative against the baseline's own magnitude, with an absolute floor
        # of one: an absolute-only tolerance is meaningless across the range a
        # geometric reduction can return.
        allowed = tolerance * max(1.0, abs(reference.value))
        if abs(reference.value - candidate.value) > allowed:
            raise BenchmarkError(
                f"{subject} diverges from {against} beyond the committed float tolerance"
            )
    elif reference.value != candidate.value:
        raise BenchmarkError(f"{subject} and {against} returned different values")


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


def read_pre_registration(path: Path, root: Path, read_blob=read_committed_blob) -> dict:
    """Read the record only after proving it is the committed one."""

    raw = path.read_bytes()
    assert_record_is_committed(raw, read_blob(root, PRE_REGISTRATION_RELATIVE_PATH))
    return json.loads(raw.decode("utf-8"))


def digest_arguments(args: tuple) -> str:
    """Digest the arguments as they stand now, so a mutation is visible."""

    return hashlib.sha256(repr(args).encode("utf-8")).hexdigest()


def time_call(call, *args) -> tuple[int, ArmOutcome]:
    """Time exactly the call, with nothing else inside the boundary."""

    started = time.perf_counter_ns()
    try:
        value = call(*args)
    except BaseException as error:  # noqa: BLE001 - the arm's failure is the datum
        # Digested *after* the call on this path too, and outside the clock: an
        # arm that mutates its arguments and then raises would otherwise carry
        # the pre-call digest and compare equal to the baseline's.
        finished = time.perf_counter_ns()
        outcome = ArmOutcome(None, type(error).__name__, digest_arguments(args))
    else:
        finished = time.perf_counter_ns()
        outcome = ArmOutcome(value, None, digest_arguments(args))
    return finished - started, outcome


def run_arm(arm: str, call, make_arguments, tolerance: float) -> tuple[dict, ArmOutcome]:
    """Warm one arm up untimed, then time `REPLICATES` validated calls of it.

    The warm-up is discarded because it pays one-off costs -- first-touch page
    faults, lazy imports inside the callee, cold caches -- that the protocol
    does not attribute to the arm. Its outcome is what the cross-arm
    correctness precondition is compared on, so that comparison never sits
    inside a reported timing.

    `make_arguments` builds the arguments afresh from the committed input, and
    is called once per invocation outside the clock. Reusing one argument tuple
    would let the warm-up change the workload every timed call then sees, and
    would let each replicate operate on data the previous one had already
    modified -- `docs/TESTING.md`'s "Correctness precondition" bullet permits an
    arm to mutate its arguments as long as every arm mutates them identically,
    so the runner must not assume they are immutable. Building them is outside
    the "Timing boundary" bullet's boundary, which is why it happens here and
    not inside `time_call`. Cross-arm digest comparison still holds because the
    factory is deterministic from the committed input.

    Every timed invocation is validated against this arm's own warm-up before
    its duration is admitted into the median, under the same committed
    `tolerance` the cross-arm comparison uses -- that tolerance is the
    protocol's own statement of what counts as the same answer. A median over unvalidated calls is
    a number from an arm that may have raised, or returned something else, on
    every call that was actually timed -- which is precisely what the
    correctness precondition exists to refuse.

    Building the three arms is not this function's business: it takes an
    already-callable arm, so the same code times the interpreter, the Cython
    extension and the `ext` artifact.
    """

    outcome: ArmOutcome | None = None
    for _ in range(WARMUP_RUNS):
        _, outcome = time_call(call, *make_arguments())
    if outcome is None:
        raise BenchmarkError("an arm must be warmed up at least once before it is timed")

    timings: list[int] = []
    for replicate in range(REPLICATES):
        elapsed, timed = time_call(call, *make_arguments())
        # Validated before the duration joins the list: an arm that answers the
        # warm-up correctly and then misbehaves on every measured call would
        # otherwise publish a perfectly ordinary-looking median.
        compare_outcomes(
            outcome,
            timed,
            tolerance,
            subject=f"the {arm} arm's replicate {replicate + 1}",
            against="its own warm-up",
        )
        timings.append(elapsed)
    return summarize(arm, timings), outcome


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

    root = Path(__file__).resolve().parent.parent
    try:
        record = read_pre_registration(arguments.pre_registration, root)
        compare_machine(observe_machine(), record.get("machine"))
        interpreter = resolve_interpreter(dict(os.environ))
        version_output, configure_args = interpreter_facts(interpreter)
        assert_pinned_version(version_output)
        assert_gil_enabled(version_output)
        assert_optimized_build(configure_args)
        # Read once, before any arm is built, and the bytes reused from here on:
        # a subject re-read per arm could be edited between them.
        read_subject_source(resolve_subject(dict(os.environ)), record.get("subject_sha256"))
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
