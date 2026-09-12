#!/usr/bin/env python3
"""Measure how much of the vendored CodeContests corpus pycc compiles.

Reports three things over ``tests/corpus/codecontests``: how many problems
``pycc build`` accepts, how many of the resulting binaries reproduce the
expected output byte-for-byte, and the median speedup against CPython.  It also
tallies the diagnostics that stopped the failures, so the compile rate comes
with the reason it is not higher.

This is a reporting gate, not a merge gate.  Any measurement outcome -- a zero
compile rate, no qualifying speedup sample, or exhausting ``--max-seconds``
before the last problem -- exits 0.  Only a broken harness exits non-zero: a
missing or corrupt manifest entry, an unreadable corpus, a corpus over its own
byte budget, or a missing ``pycc`` binary.

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
    """Normalize line endings and trailing newlines before comparing."""
    unified = text.replace("\r\n", "\n").replace("\r", "\n")
    return (unified.rstrip("\n") + "\n").encode() if unified.strip("\n") else b"\n"


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
        for filename, entry in sorted(files.items()):
            total_bytes += verify_file(corpus, f"{problem['id']}/{filename}", entry)
        if problem.get("set") == "holdout" and not include_holdout:
            continue
        selected.append(problem)

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
    return cases


def resolve_pycc(raw: str) -> str:
    path = Path(raw)
    if path.exists():
        return str(path)
    raise BrokenHarness(
        f"pycc binary {raw} not found -- build it first (cargo build --release)"
    )


def compile_problem(pycc: str, source: Path, output: Path) -> tuple[bool, str]:
    """Run ``pycc build``; return (succeeded, combined diagnostic text)."""
    try:
        completed = subprocess.run(
            [pycc, "build", str(source), "-o", str(output)],
            capture_output=True,
            timeout=BUILD_TIMEOUT_SECONDS,
            check=False,
        )
    except subprocess.TimeoutExpired:
        return False, "error[X0000]: pycc build timed out"
    text = completed.stderr.decode("utf-8", "replace") + completed.stdout.decode(
        "utf-8", "replace"
    )
    return completed.returncode == 0 and output.exists(), text


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


def run_program(argv: list[str], case: dict[str, str]) -> tuple[bool, float]:
    """Run one program on one case; return (output matched, wall seconds)."""
    started = time.monotonic()
    try:
        completed = subprocess.run(
            argv,
            input=case.get("input", "").encode(),
            capture_output=True,
            timeout=RUN_TIMEOUT_SECONDS,
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


def best_of(argv: list[str], case: dict[str, str]) -> float | None:
    """Best of ``TIMING_REPEATS`` wall times, or None if a run misbehaved."""
    best: float | None = None
    for _ in range(TIMING_REPEATS):
        matched, elapsed = run_program(argv, case)
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
    deadline = time.monotonic() + args.max_seconds

    total = len(problems)
    evaluated = 0
    compiled = 0
    matched = 0
    first_tally: dict[str, int] = {}
    any_tally: dict[str, int] = {}
    ratios: list[float] = []
    startup_excluded = 0
    incomplete = False

    for problem in problems:
        if time.monotonic() >= deadline:
            incomplete = True
            break
        evaluated += 1
        source = corpus / problem["id"] / "solution.py"
        binary = scratch / problem["id"].replace("/", "__")
        ok, text = compile_problem(pycc, source, binary)
        if not ok:
            classes = diagnostic_classes(text)
            if not classes:
                classes = ["(no diagnostic code reported)"]
            first_tally[classes[0]] = first_tally.get(classes[0], 0) + 1
            for name in dict.fromkeys(classes):
                any_tally[name] = any_tally.get(name, 0) + 1
            continue
        compiled += 1

        cases = load_cases(corpus, problem)
        if all(run_program([str(binary)], case)[0] for case in cases):
            matched += 1
            case = largest_case(cases)
            cpython = best_of([args.python, str(source)], case)
            native = best_of([str(binary)], case)
            if cpython is None or native is None or native <= 0:
                continue
            if cpython < STARTUP_FLOOR_SECONDS:
                startup_excluded += 1
                continue
            ratios.append(cpython / native)

    return {
        "problems": total,
        "evaluated": evaluated,
        "incomplete": incomplete,
        "compiled": compiled,
        "matched": matched,
        "median_speedup": statistics.median(ratios) if ratios else None,
        "speedup_samples": len(ratios),
        "startup_dominated_excluded": startup_excluded,
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
            f"[startup-dominated, excluded: {result['startup_dominated_excluded']}]"
        )
    else:
        lines.append(
            f"median speedup: {result['median_speedup']:.2f}x "
            f"over {result['speedup_samples']} qualifying problems "
            f"[startup-dominated, excluded: {result['startup_dominated_excluded']}]"
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
    corpus = Path(args.corpus)
    try:
        manifest = load_manifest(corpus)
        problems = verify_corpus(corpus, manifest, args.include_holdout)
        with tempfile.TemporaryDirectory(
            prefix="corpus-compile-rate-", dir=os.environ.get("RUNNER_TEMP") or None
        ) as tmp:
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
