#!/usr/bin/env python3
"""Measure how much of the vendored CodeContests corpus pycc compiles.

Reports three things over ``tests/corpus/codecontests``: how many problems
``pycc build`` accepts, how many of the resulting binaries reproduce the
expected output byte-for-byte, and the median speedup against CPython.  It also
tallies the diagnostics that stopped the failures, so the compile rate comes
with the reason it is not higher.

This is a reporting gate, not a merge gate.  Any measurement outcome -- a zero
compile rate, no qualifying speedup sample, or exhausting ``--max-seconds``
before the last problem -- exits 0.  Only a broken harness exits non-zero;
``docs/TESTING.md``'s corpus gate-status bullet owns the enumeration of what
counts as one, so this docstring does not restate it.

The script performs no network I/O and deliberately imports nothing
HTTP-capable: it reads only the vendored subset.  Downloading the dataset is
``scripts/select_codecontests_corpus.py``'s job and runs locally, never in CI.
It also never writes inside ``--corpus``; compiled binaries go to a scratch
directory.

Dataset content is untrusted third-party text.  A ``solution.py`` reaches a
subprocess only as an argv element, never a shell string, and a compiled binary
runs with its case input on stdin and nothing else.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import statistics
import subprocess
import sys
import tempfile
import time
from pathlib import Path

# `error[C0001]: <message>` is pycc's human diagnostic shape.
DIAGNOSTIC_RE = re.compile(r"^error\[([A-Z][0-9]{4})\]: (.*)$")

# One pinned seed is enough here: the selector only admits a solution whose
# output is identical under every seed in its own HASH_SEEDS sweep, so at
# measurement time the seed's only job is to stay constant across runs.
HASH_SEED = 0

DEFAULT_MAX_SECONDS = 1200.0
BUILD_TIMEOUT_SECONDS = 120.0
RUN_TIMEOUT_SECONDS = 30.0
TIMING_REPEATS = 3
# Below this CPython wall time the measurement is process startup, not the
# program, so the ratio says nothing about generated code.
STARTUP_FLOOR_SECONDS = 0.200

EXIT_OK = 0
EXIT_BROKEN_HARNESS = 2


class BrokenHarness(Exception):
    """The corpus or toolchain is unusable, as distinct from a low score."""


def normalize_output(text: str) -> bytes:
    """Normalize line endings and trailing newlines before comparing.

    Deliberately not a byte-exact comparison. The dataset's recorded outputs and
    a program's own final newline disagree about trailing whitespace often
    enough that a byte-exact rule would report correct programs as mismatched,
    so CRLF and CR become LF and a run of trailing newlines collapses to exactly
    one. Everything else is compared byte for byte, and output that is empty
    stays empty: a program that printed nothing did not print a blank line, and
    equating those two would hide a real difference. ``docs/TESTING.md``'s
    corpus Metric bullet is the canonical statement of this contract.
    """
    unified = text.replace("\r\n", "\n").replace("\r", "\n")
    if not unified:
        return b""
    return (unified.rstrip("\n") + "\n").encode()


def sha256_bytes(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def load_manifest(corpus: Path) -> dict:
    path = corpus / "manifest.json"
    try:
        raw = path.read_text(encoding="utf-8")
    except OSError as exc:
        raise BrokenHarness(f"cannot read {path}: {exc}") from exc
    try:
        manifest = json.loads(raw)
    except json.JSONDecodeError as exc:
        raise BrokenHarness(f"{path} is not valid JSON: {exc}") from exc
    if not isinstance(manifest, dict) or not isinstance(
        manifest.get("problems"), list
    ):
        raise BrokenHarness(f"{path} has no problem list")
    return manifest


def verify_file(corpus: Path, relative: str, entry: object) -> int:
    """Check one vendored file against its manifest record; return its size."""
    if not isinstance(entry, dict) or "sha256" not in entry or "bytes" not in entry:
        raise BrokenHarness(f"manifest entry for {relative} is incomplete")
    path = corpus / relative
    try:
        data = path.read_bytes()
    except OSError as exc:
        raise BrokenHarness(f"cannot read {path}: {exc}") from exc
    if len(data) != entry["bytes"]:
        raise BrokenHarness(
            f"{relative} is {len(data)} bytes, manifest says {entry['bytes']}"
        )
    digest = sha256_bytes(data)
    if digest != entry["sha256"]:
        raise BrokenHarness(
            f"{relative} sha256 {digest} does not match manifest {entry['sha256']}"
        )
    return len(data)


def verify_corpus(corpus: Path, manifest: dict, include_holdout: bool) -> list[dict]:
    """Verify every vendored file and return the selected problem records."""
    total_bytes = 0
    for name, entry in sorted((manifest.get("files") or {}).items()):
        total_bytes += verify_file(corpus, name, entry)

    selected: list[dict] = []
    for problem in manifest["problems"]:
        if not isinstance(problem, dict) or "id" not in problem:
            raise BrokenHarness("manifest contains a problem entry without an id")
        files = problem.get("files")
        if not isinstance(files, dict) or "solution.py" not in files:
            raise BrokenHarness(f"{problem['id']} has no solution.py manifest entry")
        # `load_cases` reads `tests.json` for every problem, so an entry for it
        # is as load-bearing as the solution's: without one the payload driving
        # every correctness verdict and every timing case would be the only
        # vendored file no digest covers.
        if "tests.json" not in files:
            raise BrokenHarness(f"{problem['id']} has no tests.json manifest entry")
        for filename, entry in sorted(files.items()):
            total_bytes += verify_file(corpus, f"{problem['id']}/{filename}", entry)
        # Validate the payload shape here rather than at first use, and for
        # every vendored problem rather than only the selected ones:
        # `load_cases` runs only for a problem `pycc build` already accepted,
        # and CI never passes `--include-holdout`, so anything checked after
        # the skip below would stay latent for months on half the corpus.
        load_cases(corpus, problem)
        if problem.get("set") == "holdout" and not include_holdout:
            continue
        selected.append(problem)

    # The manifest declares its own split; a record set that disagrees with it
    # means the vendored corpus is not the one the manifest describes, which is
    # a broken harness in the same sense as a digest mismatch.
    for key, label in (("steering_count", "steering"), ("holdout_count", "holdout")):
        declared = manifest.get(key)
        if not isinstance(declared, int):
            continue
        actual = sum(1 for p in manifest["problems"] if p.get("set") == label)
        if actual != declared:
            raise BrokenHarness(
                f"the manifest declares {key} {declared} but holds {actual} "
                f"{label} problem records"
            )

    budget = manifest.get("max_bytes")
    if isinstance(budget, int) and total_bytes > budget:
        raise BrokenHarness(
            f"the vendored corpus is {total_bytes} bytes, over its own "
            f"{budget}-byte budget"
        )
    return selected


def load_cases(corpus: Path, problem: dict) -> list[dict[str, str]]:
    path = corpus / problem["id"] / "tests.json"
    try:
        payload = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as exc:
        raise BrokenHarness(f"cannot read {path}: {exc}") from exc
    cases = payload.get("cases") if isinstance(payload, dict) else None
    if not isinstance(cases, list) or not cases:
        raise BrokenHarness(f"{path} has no cases")
    # Shape-check each case rather than trusting `dict.get` at use time: a case
    # that is not an object, or whose `input`/`output` is not a string, would
    # otherwise either inflate `matched` (an empty object compares an empty
    # input against an empty expectation and passes) or raise an uncaught
    # `AttributeError` mid-measurement instead of the documented exit 2.
    for index, case in enumerate(cases):
        if not isinstance(case, dict):
            raise BrokenHarness(f"{path} case {index} is not an object")
        for field in ("input", "output"):
            if not isinstance(case.get(field), str):
                raise BrokenHarness(f"{path} case {index} has no string `{field}`")
    return cases


def resolve_pycc(raw: str) -> str:
    # Executability, not mere existence: `compile_problem` catches a build
    # timeout and nothing else, so a path that exists but cannot be executed
    # would surface as an uncaught `OSError` instead of the clean broken-harness
    # exit this script promises for a `pycc` it cannot run.
    path = Path(raw)
    if os.access(path, os.X_OK) and path.is_file():
        return str(path)
    raise BrokenHarness(
        f"pycc binary {raw} not executable -- build it first "
        "(cargo build --release)"
    )


def compile_problem(
    pycc: str, source: Path, output: Path
) -> tuple[bool, str, int | None]:
    """Run ``pycc build``; return (succeeded, diagnostic text, exit status).

    ``--release`` is not optional here. The report's median speedup compares the
    generated program against CPython under the shipping profile, so timing an
    unoptimized build would measure something this report never claims. The exit status is returned because it is what separates a
    compiler's rejection of the program from an environment that cannot build
    anything -- see `measure`'s own use of it.
    """
    try:
        completed = subprocess.run(
            [pycc, "build", "--release", str(source), "-o", str(output)],
            capture_output=True,
            timeout=BUILD_TIMEOUT_SECONDS,
            check=False,
        )
    except subprocess.TimeoutExpired:
        return False, "error[X0000]: pycc build timed out", None
    text = completed.stderr.decode("utf-8", "replace") + completed.stdout.decode(
        "utf-8", "replace"
    )
    return (
        completed.returncode == 0 and output.exists(),
        text,
        completed.returncode,
    )


# Diagnostic messages that name a symbol chosen by the corpus author rather than
# a Python construct. Collapsing those names to `X` is what makes the tally a
# tally of *classes*: without it every distinct parameter name in the corpus
# reports as its own failure class, and the result cannot be compared against a
# previously recorded class breakdown. Deliberately narrow — a backtick span is
# left verbatim everywhere else, because elsewhere it names the construct or
# module that is the whole signal (`import of module `sys``, `expression kind
# not supported yet: a `lambda``), and merging those would destroy the ordering
# that says which gap to close next.
IDENTIFIER_ELISIONS: tuple[tuple[re.Pattern[str], str], ...] = (
    (
        re.compile(r"^parameter `[^`]*` of public function `[^`]*` (needs .*)$"),
        r"parameter X of public function X \1",
    ),
    (
        re.compile(r"^public function `[^`]*` (needs a return type annotation)$"),
        r"public function X \1",
    ),
    # Only the *subject* class name is author-chosen here. The base class the
    # compiler could not resolve stays verbatim: it is the actionable signal,
    # and it is frequently a builtin or a stdlib name (`object`, `math`) rather
    # than anything the corpus author invented.
    (
        re.compile(r"^class `[^`]*` (inherits from unknown class `.*)$"),
        r"class X \1",
    ),
)


def normalize_diagnostic_message(message: str) -> str:
    """Collapse author-chosen identifiers so messages aggregate into classes."""
    for pattern, replacement in IDENTIFIER_ELISIONS:
        normalized, count = pattern.subn(replacement, message)
        if count:
            return normalized
    return message


def diagnostic_classes(text: str) -> list[str]:
    """Extract the ordered diagnostic classes from pycc's output."""
    classes: list[str] = []
    for line in text.splitlines():
        match = DIAGNOSTIC_RE.match(line.strip())
        if match:
            message = normalize_diagnostic_message(match.group(2))
            classes.append(f"{match.group(1)} {message}")
    return classes


def isolated_env() -> dict[str, str]:
    """The environment a vendored solution is executed under.

    Mirrors ``select_codecontests_corpus.py``'s ``run_case``: the corpus is
    third-party data, so the interpreter that runs it gets none of the ambient
    ``PYTHONPATH`` and a pinned hash seed.  Without this, the same file the
    selector deliberately isolated would inherit the runner's whole import
    surface here, and the timing would depend on it.
    """
    env = dict(os.environ)
    env["PYTHONHASHSEED"] = str(HASH_SEED)
    env.pop("PYTHONPATH", None)
    return env


def cpython_argv(python: str, source: Path) -> list[str]:
    """How a vendored solution is handed to CPython.

    ``-I`` is the other half of ``isolated_env``: it drops the user site
    directory and the remaining ``PYTHON*`` variables, and keeps the script's
    own directory -- inside ``--corpus`` -- off ``sys.path``.  The selector
    admitted every one of these solutions under exactly this flag, so it also
    keeps the measurement faithful to what was validated.
    """
    return [python, "-I", str(source)]


def run_program(
    argv: list[str], case: dict[str, str], cwd: Path
) -> tuple[bool, float]:
    """Run one program on one case; return (output matched, wall seconds).

    ``cwd`` is a scratch directory, never the corpus and never the caller's
    directory: a solution needs no import to reach the filesystem, so a static
    import allowlist cannot be the only thing standing between third-party
    source and the tree it is measured in.

    This bounds an unqualified relative filename, which is what an ordinary
    competitive-programming solution would write; it is not a filesystem
    sandbox, and an absolute path or ``../`` escapes it. Two things carry that
    weight instead: every vendored byte is digest-verified against the manifest
    before anything executes, and the sibling directory holding the compiled
    binaries is randomly named, so no fixed relative path reaches it and
    enumerating it needs a module the selector's allowlist does not admit.
    """
    started = time.monotonic()
    try:
        completed = subprocess.run(
            argv,
            input=case.get("input", "").encode(),
            capture_output=True,
            timeout=RUN_TIMEOUT_SECONDS,
            env=isolated_env(),
            cwd=str(cwd),
            check=False,
        )
    except (subprocess.TimeoutExpired, OSError):
        return False, RUN_TIMEOUT_SECONDS
    elapsed = time.monotonic() - started
    if completed.returncode != 0:
        return False, elapsed
    expected = normalize_output(case.get("output", ""))
    actual = normalize_output(completed.stdout.decode("utf-8", "replace"))
    return expected == actual, elapsed


def best_of(argv: list[str], case: dict[str, str], cwd: Path) -> float | None:
    """Best of ``TIMING_REPEATS`` wall times, or None if a run misbehaved."""
    best: float | None = None
    for _ in range(TIMING_REPEATS):
        matched, elapsed = run_program(argv, case, cwd)
        if not matched:
            return None
        best = elapsed if best is None else min(best, elapsed)
    return best


def largest_case(cases: list[dict[str, str]]) -> dict[str, str]:
    return max(cases, key=lambda case: len(case.get("input", "")))


def measure(
    args: argparse.Namespace, corpus: Path, problems: list[dict], scratch: Path
) -> dict:
    pycc = resolve_pycc(args.pycc)
    # Two randomly named siblings rather than fixed names: a solution executes
    # in one of them, and the other holds the compiled binaries -- including
    # those of problems not yet reached. A fixed layout would make each of
    # those output paths a derivable `../<name>` away from a program that is,
    # by construction, third-party code.
    workdir = Path(tempfile.mkdtemp(prefix="run-", dir=scratch))
    bindir = Path(tempfile.mkdtemp(prefix="bin-", dir=scratch))
    deadline = time.monotonic() + args.max_seconds

    def expired() -> bool:
        return time.monotonic() >= deadline

    total = len(problems)
    evaluated = 0
    compiled = 0
    matched = 0
    first_tally: dict[str, int] = {}
    any_tally: dict[str, int] = {}
    ratios: list[float] = []
    startup_excluded = 0
    timing_dropped = 0
    undiagnosed = 0
    incomplete = False

    # The deadline is re-checked at every phase boundary, not only between
    # problems: one problem can hold the loop for a build plus a run per case
    # plus both timing legs, which together far exceed the budget the CI job's
    # own `timeout-minutes` relies on.  Checking per phase caps the overshoot at
    # one build.  A problem abandoned before its outcome is final is not counted
    # at all -- `evaluated` is committed once that outcome is known -- so the
    # published tallies always reconcile against the problems behind them.
    for problem in problems:
        if expired():
            incomplete = True
            break
        source = corpus / problem["id"] / "solution.py"
        binary = bindir / problem["id"].replace("/", "__")
        ok, text, status = compile_problem(pycc, source, binary)
        if not ok:
            # `docs/CLI_SPEC.md` reserves exit 2 for an invalid invocation or a
            # broken environment. This script's invocation is fixed, so exit 2
            # can only be the environment -- a missing linker driver, an absent
            # runtime library -- and tallying that as though the compiler had
            # rejected the program would publish a zero compile rate as a score
            # while the job stayed green.
            if status == 2:
                lines = text.strip().splitlines()
                detail = lines[0] if lines else "no output"
                raise BrokenHarness(
                    "pycc reported an environment failure (exit 2) on "
                    f"{problem['id']}: {detail}"
                )
            evaluated += 1
            classes = diagnostic_classes(text)
            if not classes:
                undiagnosed += 1
                classes = ["(no diagnostic code reported)"]
            first_tally[classes[0]] = first_tally.get(classes[0], 0) + 1
            for name in dict.fromkeys(classes):
                any_tally[name] = any_tally.get(name, 0) + 1
            continue

        cases = load_cases(corpus, problem)
        every_case_matched = True
        abandoned = False
        for case in cases:
            if expired():
                abandoned = True
                break
            if not run_program([str(binary)], case, workdir)[0]:
                every_case_matched = False
                break
        if abandoned:
            incomplete = True
            break

        evaluated += 1
        compiled += 1
        if not every_case_matched:
            continue
        matched += 1
        case = largest_case(cases)
        cpython = None if expired() else best_of(
            cpython_argv(args.python, source), case, workdir
        )
        native = None if cpython is None or expired() else best_of(
            [str(binary)], case, workdir
        )
        if cpython is None or native is None or native <= 0:
            # A repeat run disagreed with the case, the native time was
            # unmeasurably small, or the budget ran out between the legs.
            # Counted so `matched` reconciles against the samples the report
            # actually publishes.
            timing_dropped += 1
            if expired():
                incomplete = True
                break
            continue
        if cpython < STARTUP_FLOOR_SECONDS:
            startup_excluded += 1
            continue
        ratios.append(cpython / native)

    # A build that fails while emitting no `error[CODE]` diagnostic at all is
    # not a rejection: pycc neither accepted nor diagnosed the program. One such
    # problem is a compiler defect worth tallying, but every evaluated problem
    # failing that way is a toolchain that cannot build anything -- the failure
    # mode the exit-2 arm above cannot see, because a linker driver that runs
    # and then fails exits 1 with no diagnostic of pycc's own.
    if evaluated and undiagnosed == evaluated:
        raise BrokenHarness(
            f"no problem produced a pycc diagnostic across all {evaluated} "
            "evaluated builds -- the toolchain, not the corpus, is what failed"
        )

    return {
        "problems": total,
        "evaluated": evaluated,
        "incomplete": incomplete,
        "compiled": compiled,
        "matched": matched,
        "median_speedup": statistics.median(ratios) if ratios else None,
        "speedup_samples": len(ratios),
        "startup_dominated_excluded": startup_excluded,
        "timing_dropped": timing_dropped,
        "undiagnosed_build_failures": undiagnosed,
        "include_holdout": bool(args.include_holdout),
        "first_diagnostic": dict(sorted(first_tally.items())),
        "any_diagnostic": dict(sorted(any_tally.items())),
    }


def render(result: dict) -> str:
    lines = [
        "corpus compile rate (tests/corpus/codecontests)",
        f"compiled {result['compiled']}/{result['problems']}",
        f"matched {result['matched']}/{result['compiled']}",
    ]
    if result["median_speedup"] is None:
        lines.append(
            "median speedup: n/a (0 qualifying) "
            f"[startup-dominated, excluded: {result['startup_dominated_excluded']}; "
            f"timing dropped: {result['timing_dropped']}]"
        )
    else:
        lines.append(
            f"median speedup: {result['median_speedup']:.2f}x "
            f"over {result['speedup_samples']} qualifying problems "
            f"[startup-dominated, excluded: {result['startup_dominated_excluded']}; "
            f"timing dropped: {result['timing_dropped']}]"
        )
    if result["incomplete"]:
        lines.append(
            f"INCOMPLETE: {result['evaluated']} of {result['problems']} evaluated"
        )
    lines.append("")
    lines.append("failure class (first / any):")
    if result["first_diagnostic"] or result["any_diagnostic"]:
        names = sorted(set(result["first_diagnostic"]) | set(result["any_diagnostic"]))
        for name in names:
            first = result["first_diagnostic"].get(name, 0)
            every = result["any_diagnostic"].get(name, 0)
            lines.append(f"  {first:>4} / {every:>4}  {name}")
    else:
        lines.append("  (none)")
    return "\n".join(lines) + "\n"


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--corpus", default="tests/corpus/codecontests")
    parser.add_argument("--pycc", default="target/release/pycc")
    parser.add_argument("--python", default=sys.executable)
    parser.add_argument("--max-seconds", type=float, default=DEFAULT_MAX_SECONDS)
    parser.add_argument("--include-holdout", action="store_true")
    parser.add_argument("--json", dest="json_path", default=None)
    return parser


def main(argv: list[str] | None = None) -> int:
    args = build_parser().parse_args(argv)
    # Resolved, not taken as given: `measure` runs both the compiled binary and
    # CPython with `cwd` set to a scratch directory, so a relative `--corpus`
    # (the default is one) would stop naming the corpus the moment a program is
    # launched, and every timing leg would fail to find its own `solution.py`.
    corpus = Path(args.corpus).resolve()
    try:
        manifest = load_manifest(corpus)
        problems = verify_corpus(corpus, manifest, args.include_holdout)
        scratch_parent = os.environ.get("RUNNER_TEMP") or None
        try:
            scratch = tempfile.TemporaryDirectory(
                prefix="corpus-compile-rate-", dir=scratch_parent
            )
        except OSError as exc:
            # An unusable scratch parent -- a `RUNNER_TEMP` pointing at a
            # missing or unwritable path -- is a broken harness, not a
            # measurement outcome, so it must exit with the documented status
            # rather than as an unhandled traceback.
            raise BrokenHarness(
                f"cannot create a scratch directory under "
                f"{scratch_parent or 'the default temporary directory'}: {exc}"
            ) from exc
        with scratch as tmp:
            result = measure(args, corpus, problems, Path(tmp))
    except BrokenHarness as exc:
        print(f"error: {exc}", file=sys.stderr)
        return EXIT_BROKEN_HARNESS

    report = render(result)
    sys.stdout.write(report)
    summary = os.environ.get("GITHUB_STEP_SUMMARY")
    if summary:
        try:
            with open(summary, "a", encoding="utf-8") as handle:
                handle.write("```\n" + report + "```\n")
        except OSError as exc:
            print(f"warning: could not write the step summary: {exc}", file=sys.stderr)
    if args.json_path:
        try:
            Path(args.json_path).write_text(
                json.dumps(result, sort_keys=True, indent=2) + "\n", encoding="utf-8"
            )
        except OSError as exc:
            print(f"error: cannot write {args.json_path}: {exc}", file=sys.stderr)
            return EXIT_BROKEN_HARNESS
    return EXIT_OK


if __name__ == "__main__":
    raise SystemExit(main())
