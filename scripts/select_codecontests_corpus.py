#!/usr/bin/env python3
"""Select and vendor a deterministic subset of the DeepMind CodeContests dataset.

This script is the one regeneration path for ``tests/corpus/codecontests/``.  It
downloads a pinned set of parquet shards, applies a fixed filter chain, and
writes a byte-identical corpus tree plus a manifest.  It is developer tooling:
CI never runs it, and ``scripts/check_corpus_compile_rate.py`` deliberately does
not import anything this module needs (no network, no ``pyarrow``).

The dataset is third-party content from the public internet.  Problem statements
and solution sources are untrusted text: they are only ever written to files and
executed inside the CPython subprocess this script starts on purpose, never
passed to a shell.
"""

from __future__ import annotations

import argparse
import ast
import hashlib
import json
import os
import re
import shutil
import statistics
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.request
import warnings
from pathlib import Path

DATASET = "deepmind/code_contests"
DATASET_URL = "https://huggingface.co/datasets/deepmind/code_contests"
DEFAULT_REVISION = "802411c3010cb00d1b05bad57ca77365a3c699d6"

# ``solutions.language`` is a protobuf enum: UNKNOWN=0, PYTHON=1 (Python 2),
# CPP=2, PYTHON3=3, JAVA=4.  Only PYTHON3 solutions are eligible.
PYTHON3_LANGUAGE = 3

# Shards are pinned by name, byte size and sha256.  ``split_rank`` fixes the
# candidate order together with the problem name.
SHARDS = (
    {
        "split": "valid",
        "split_rank": 0,
        "path": "data/valid-00000-of-00001-5e672c5751f060d3.parquet",
        "bytes": 51829044,
        "sha256": "02e8c1ccedae716f1e43cc813fcb7823c3db666ea92638820aba80e8cef451ab",
    },
    {
        "split": "test",
        "split_rank": 1,
        "path": "data/test-00000-of-00001-9c49eeff30aacaa8.parquet",
        "bytes": 63077400,
        "sha256": "aa426cbdb202bf8703b658bcb31fd1878ca7cfd33ca07d3b703dc94ca6a2b651",
    },
    {
        "split": "train",
        "split_rank": 2,
        "path": "data/train-00000-of-00039-e991a271dbfa9925.parquet",
        "bytes": 180227735,
        "sha256": "950bfdefde1f274edf93963e7e23c93ab16034de344aa7b7c32d14145e8d5232",
    },
    # The first three shards yield only 278 of the 300 problems this corpus
    # needs, so a fourth is pinned.  Widening the pool is the documented
    # response to a short yield; relaxing a filter or the target counts is not.
    {
        "split": "train",
        "split_rank": 3,
        "path": "data/train-00001-of-00039-e092fe56fda18715.parquet",
        "bytes": 208507780,
        "sha256": "7bcc72d98a3d97f07be90ea70bf8db834c8f29ac2c7626d7cfc657bae6f5ddd9",
    },
)

# The next shard to add if the eligible yield ever starves again.  Named so the
# failure path can tell the operator exactly what to do.
NEXT_SHARD = "data/train-00002-of-00039-9cea23812e920e41.parquet"

# Explicit stdlib allowlist.  Never widen this to ``sys.stdlib_module_names``:
# this list is what keeps ``os``, ``socket`` and ``subprocess`` out of the
# programs this script executes.
ALLOWED_IMPORT_ROOTS = frozenset(
    {
        "array",
        "bisect",
        "collections",
        "decimal",
        "fractions",
        "functools",
        "heapq",
        "itertools",
        "math",
        "re",
        "statistics",
        "string",
        "sys",
    }
)

MAX_SOLUTION_LINES = 100
MAX_CASES = 25
MAX_PROBLEM_BYTES = 48 * 1024
DEFAULT_MAX_BYTES = 16 * 1024 * 1024
HASH_SEEDS = (0, 1, 12345)
PER_CASE_TIMEOUT_SECONDS = 1.0

FILTERS = (
    "no_python3_solution",
    "no_test_cases",
    "solution_too_long",
    "solution_unparsable",
    "import_not_allowlisted",
    "too_many_cases",
    "problem_too_large",
    "output_not_reproducible",
)

VENDORED_ROOT_FILES = ("LICENSE", "NOTICE", "README.md")


# --- pure helpers -----------------------------------------------------------


def physical_line_count(source: str) -> int:
    """Count physical lines, ignoring one optional trailing newline."""
    if not source:
        return 0
    return len(source.rstrip("\n").split("\n"))


def slugify(name: str) -> str:
    """Reduce an untrusted dataset problem name to a safe path component.

    The result is restricted to ``[a-z0-9-]``, is never empty, and is capped so
    that no filesystem path limit can be reached.
    """
    lowered = name.lower()
    cleaned = re.sub(r"[^a-z0-9]+", "-", lowered).strip("-")
    cleaned = cleaned[:48].strip("-")
    return cleaned or "problem"


def import_roots(tree: ast.AST) -> set[str]:
    """Collect the root module name of every import in a parsed module."""
    roots: set[str] = set()
    for node in ast.walk(tree):
        if isinstance(node, ast.Import):
            for alias in node.names:
                roots.add(alias.name.split(".")[0])
        elif isinstance(node, ast.ImportFrom):
            if node.level:
                roots.add("")
            elif node.module:
                roots.add(node.module.split(".")[0])
    return roots


def imports_allowlisted(roots: set[str]) -> bool:
    return roots.issubset(ALLOWED_IMPORT_ROOTS)


def canonical_json(payload: object) -> str:
    """Serialize deterministically: sorted keys, 2-space indent, LF, newline."""
    return json.dumps(payload, sort_keys=True, indent=2, ensure_ascii=True) + "\n"


def sha256_bytes(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def normalize_source(source: str) -> str:
    """Normalize line endings and guarantee exactly one trailing newline."""
    text = source.replace("\r\n", "\n").replace("\r", "\n")
    return text.rstrip("\n") + "\n"


def cases_from_struct(struct: dict | None) -> list[dict[str, str]]:
    """Build a case list from the dataset's parallel input/output arrays."""
    if not struct:
        return []
    inputs = struct.get("input") or []
    outputs = struct.get("output") or []
    return [
        {"input": inp, "output": out} for inp, out in zip(inputs, outputs, strict=False)
    ]


def tests_payload(cases: list[dict[str, str]]) -> str:
    return canonical_json({"cases": cases})


def choose_python3_solutions(solutions: dict | None) -> list[str]:
    """Return the PYTHON3 solutions in dataset order."""
    if not solutions:
        return []
    languages = solutions.get("language") or []
    sources = solutions.get("solution") or []
    return [
        src
        for lang, src in zip(languages, sources, strict=False)
        if lang == PYTHON3_LANGUAGE
    ]


def resolve_problem_dir(root: Path, subdir: str, index: int, name: str) -> Path:
    """Build a problem directory path and prove it stays inside ``root``."""
    candidate = (root / subdir / f"{index:03d}-{slugify(name)}").resolve()
    base = (root / subdir).resolve()
    if base != candidate.parent:
        raise ValueError(f"refusing to write outside the corpus root: {candidate}")
    return candidate


# --- download ---------------------------------------------------------------


def shard_url(revision: str, path: str) -> str:
    return f"https://huggingface.co/datasets/{DATASET}/resolve/{revision}/{path}"


def fetch_shard(revision: str, shard: dict, cache_dir: Path) -> Path:
    target = cache_dir / Path(shard["path"]).name
    if not target.exists():
        url = shard_url(revision, shard["path"])
        print(f"downloading {url}", flush=True)
        cache_dir.mkdir(parents=True, exist_ok=True)
        tmp = target.with_suffix(target.suffix + ".part")
        try:
            with urllib.request.urlopen(url) as response, tmp.open("wb") as handle:
                shutil.copyfileobj(response, handle)
        except urllib.error.URLError as exc:  # pragma: no cover - network path
            raise SystemExit(f"error: could not download {url}: {exc}") from exc
        tmp.replace(target)
    data = target.read_bytes()
    if len(data) != shard["bytes"]:
        raise SystemExit(
            f"error: {target} is {len(data)} bytes, expected {shard['bytes']}"
        )
    digest = sha256_bytes(data)
    if digest != shard["sha256"]:
        raise SystemExit(
            f"error: {target} sha256 {digest} does not match pinned {shard['sha256']}"
        )
    return target


def read_candidates(shard: dict, shard_file: Path) -> list[dict]:
    try:
        import pyarrow.parquet as pq
    except ModuleNotFoundError as exc:  # pragma: no cover - environment path
        raise SystemExit(
            "error: pyarrow is required to read the dataset shards. Install it in a "
            "throwaway virtualenv (`python3 -m venv .venv && .venv/bin/pip install "
            "pyarrow`) and re-run. It is intentionally not a repository dependency: "
            "scripts/check_corpus_compile_rate.py must never import it."
        ) from exc
    table = pq.read_table(
        shard_file, columns=["name", "public_tests", "private_tests", "solutions"]
    )
    rows = table.to_pylist()
    for row in rows:
        row["split"] = shard["split"]
        row["split_rank"] = shard["split_rank"]
    return rows


# --- execution filter -------------------------------------------------------


def run_case(
    python: str, source_path: Path, case: dict[str, str], seed: int, cwd: Path
) -> tuple[bool, float]:
    """Run one case under one hash seed; return (matched, wall seconds)."""
    env = dict(os.environ)
    env["PYTHONHASHSEED"] = str(seed)
    env.pop("PYTHONPATH", None)
    started = time.monotonic()
    try:
        completed = subprocess.run(
            [python, "-I", str(source_path)],
            input=case["input"].encode(),
            capture_output=True,
            timeout=PER_CASE_TIMEOUT_SECONDS,
            cwd=str(cwd),
            env=env,
            check=False,
        )
    except subprocess.TimeoutExpired:
        return False, PER_CASE_TIMEOUT_SECONDS
    elapsed = time.monotonic() - started
    if completed.returncode != 0:
        return False, elapsed
    expected = normalize_source(case["output"]).encode()
    actual = normalize_source(completed.stdout.decode("utf-8", "replace")).encode()
    return expected == actual, elapsed


def reproduces_expected_output(
    python: str, source: str, cases: list[dict[str, str]], workdir: Path
) -> tuple[bool, float]:
    """Check byte-for-byte reproduction under every hash seed.

    Returns ``(ok, largest_case_wall_seconds)`` where the wall time is the
    slowest observed run of the largest case.
    """
    source_path = workdir / "candidate.py"
    source_path.write_text(source, encoding="utf-8")
    largest = max(range(len(cases)), key=lambda i: len(cases[i]["input"]))
    largest_wall = 0.0
    for seed in HASH_SEEDS:
        for index, case in enumerate(cases):
            matched, elapsed = run_case(python, source_path, case, seed, workdir)
            if not matched:
                return False, 0.0
            if index == largest:
                largest_wall = max(largest_wall, elapsed)
    return True, largest_wall


# --- selection --------------------------------------------------------------


def evaluate_candidate(
    row: dict, python: str, workdir: Path, rejected: dict[str, int]
) -> dict | None:
    """Apply the filter chain; return an accepted record or ``None``.

    Filter order is a security property, not only a performance one: the import
    allowlist runs strictly before anything is executed.
    """
    solutions = choose_python3_solutions(row.get("solutions"))
    if not solutions:
        rejected["no_python3_solution"] += 1
        return None

    cases = cases_from_struct(row.get("public_tests")) + cases_from_struct(
        row.get("private_tests")
    )
    if not cases:
        rejected["no_test_cases"] += 1
        return None
    if len(cases) > MAX_CASES:
        rejected["too_many_cases"] += 1
        return None
    packed_tests = tests_payload(cases)

    reasons: list[str] = []
    for raw in solutions:
        source = normalize_source(raw)
        if physical_line_count(source) > MAX_SOLUTION_LINES:
            reasons.append("solution_too_long")
            continue
        try:
            # Untrusted sources routinely emit SyntaxWarning (invalid escapes,
            # stray decimal literals); that is data about them, not our output.
            with warnings.catch_warnings():
                warnings.simplefilter("ignore")
                tree = ast.parse(source)
        except SyntaxError:
            reasons.append("solution_unparsable")
            continue
        roots = import_roots(tree)
        if not imports_allowlisted(roots):
            reasons.append("import_not_allowlisted")
            continue
        total = len(source.encode()) + len(packed_tests.encode())
        if total > MAX_PROBLEM_BYTES:
            reasons.append("problem_too_large")
            continue
        ok, wall = reproduces_expected_output(python, source, cases, workdir)
        if not ok:
            reasons.append("output_not_reproducible")
            continue
        return {
            "name": row["name"],
            "split": row["split"],
            "solution": source,
            "tests": packed_tests,
            "solution_lines": physical_line_count(source),
            "imports": sorted(roots),
            "case_count": len(cases),
            "largest_case_bytes": max(len(case["input"]) for case in cases),
            "cpython_wall_seconds": wall,
        }

    # Attribute the rejection to the first filter that stopped the first
    # PYTHON3 solution, so the histogram stays one-reason-per-problem.
    rejected[reasons[0]] += 1
    return None


def write_problem(directory: Path, record: dict) -> dict[str, dict[str, object]]:
    directory.mkdir(parents=True, exist_ok=True)
    files: dict[str, dict[str, object]] = {}
    for filename, text in (
        ("solution.py", record["solution"]),
        ("tests.json", record["tests"]),
    ):
        data = text.encode()
        (directory / filename).write_bytes(data)
        files[filename] = {"bytes": len(data), "sha256": sha256_bytes(data)}
    return files


def format_report(
    examined: int,
    rejected: dict[str, int],
    steering: list[dict],
    holdout: list[dict],
    total_bytes: int,
) -> str:
    lines = [
        "selection report",
        f"  candidates examined: {examined}",
        "  rejected:",
    ]
    for name in FILTERS:
        lines.append(f"    {name}: {rejected[name]}")
    lines.append(f"  accepted steering: {len(steering)}")
    lines.append(f"  accepted holdout: {len(holdout)}")
    lines.append(f"  total vendored bytes: {total_bytes}")
    walls = sorted(
        record["cpython_wall_seconds"] for record in steering + holdout
    )
    if walls:
        quantiles = {
            "min": walls[0],
            "p50": statistics.median(walls),
            "p90": walls[min(len(walls) - 1, int(0.9 * len(walls)))],
            "max": walls[-1],
        }
        rendered = ", ".join(f"{key} {value * 1000:.1f}ms" for key, value in quantiles.items())
        lines.append(f"  largest-case CPython wall time: {rendered}")
        qualifying = sum(1 for wall in walls if wall >= 0.2)
        lines.append(f"  problems at or above the 200ms speedup floor: {qualifying}")
    else:
        lines.append("  largest-case CPython wall time: n/a (0 accepted)")
    return "\n".join(lines)


def select(args: argparse.Namespace) -> int:
    cache_dir = Path(args.cache_dir).expanduser()
    out_root = Path(args.out).expanduser()
    wanted = args.target + args.holdout

    rejected = {name: 0 for name in FILTERS}
    accepted: list[dict] = []
    examined = 0

    candidates: list[dict] = []
    for shard in SHARDS:
        shard_file = fetch_shard(args.revision, shard, cache_dir)
        candidates.extend(read_candidates(shard, shard_file))
    candidates.sort(key=lambda row: (row["split_rank"], row["name"]))

    with tempfile.TemporaryDirectory(prefix="codecontests-select-") as tmp:
        workdir = Path(tmp)
        for row in candidates:
            if len(accepted) >= wanted:
                break
            examined += 1
            record = evaluate_candidate(row, args.python, workdir, rejected)
            if record is not None:
                accepted.append(record)

    steering = accepted[: args.target]
    holdout = accepted[args.target :]

    if len(accepted) < wanted:
        print(
            format_report(examined, rejected, steering, holdout, 0),
            file=sys.stderr,
        )
        print(
            f"error: only {len(accepted)} of {wanted} problems passed every filter. "
            f"Widen the pool by pinning the next shard ({NEXT_SHARD}) in SHARDS and "
            "recording its byte size and sha256. Never relax the stdlib allowlist, "
            "the output-reproduction filter, or the target counts.",
            file=sys.stderr,
        )
        return 1

    for subdir in ("problems", "holdout"):
        shutil.rmtree(out_root / subdir, ignore_errors=True)
    out_root.mkdir(parents=True, exist_ok=True)

    problems: list[dict] = []
    total_bytes = 0
    for set_name, subdir, records in (
        ("steering", "problems", steering),
        ("holdout", "holdout", holdout),
    ):
        for index, record in enumerate(records, start=1):
            directory = resolve_problem_dir(out_root, subdir, index, record["name"])
            files = write_problem(directory, record)
            total_bytes += sum(int(entry["bytes"]) for entry in files.values())
            problems.append(
                {
                    "id": f"{subdir}/{directory.name}",
                    "set": set_name,
                    "name": record["name"],
                    "source_split": record["split"],
                    "solution_lines": record["solution_lines"],
                    "imports": record["imports"],
                    "case_count": record["case_count"],
                    "largest_case_bytes": record["largest_case_bytes"],
                    "files": files,
                }
            )

    root_files: dict[str, dict[str, object]] = {}
    for filename in VENDORED_ROOT_FILES:
        path = out_root / filename
        if not path.exists():
            print(
                f"error: {path} is missing. The licence, attribution and provenance "
                "files are hand-authored and must exist before the manifest is "
                "written.",
                file=sys.stderr,
            )
            return 1
        data = path.read_bytes()
        total_bytes += len(data)
        root_files[filename] = {"bytes": len(data), "sha256": sha256_bytes(data)}

    if total_bytes > args.max_bytes:
        print(
            f"error: the vendored corpus is {total_bytes} bytes, over the "
            f"{args.max_bytes}-byte budget.",
            file=sys.stderr,
        )
        return 1

    manifest = {
        "dataset": DATASET,
        "dataset_url": DATASET_URL,
        "revision": args.revision,
        "data_license": "CC BY 4.0",
        # Recorded so a third party can reproduce the eligibility predicate
        # exactly: the determinism filter is fixed-seed, never sampled.
        "hash_seeds": list(HASH_SEEDS),
        "per_case_timeout_seconds": PER_CASE_TIMEOUT_SECONDS,
        "shards": [
            {
                "path": shard["path"],
                "split": shard["split"],
                "bytes": shard["bytes"],
                "sha256": shard["sha256"],
            }
            for shard in SHARDS
        ],
        "steering_count": len(steering),
        "holdout_count": len(holdout),
        "total_bytes": total_bytes,
        "max_bytes": args.max_bytes,
        "files": root_files,
        "problems": problems,
    }
    (out_root / "manifest.json").write_text(canonical_json(manifest), encoding="utf-8")

    print(format_report(examined, rejected, steering, holdout, total_bytes))
    return 0


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--revision", default=DEFAULT_REVISION)
    parser.add_argument("--out", default="tests/corpus/codecontests")
    parser.add_argument("--target", type=int, default=200)
    parser.add_argument("--holdout", type=int, default=100)
    parser.add_argument("--max-bytes", type=int, default=DEFAULT_MAX_BYTES)
    parser.add_argument("--cache-dir", default=".cache/codecontests")
    parser.add_argument("--python", default=sys.executable)
    return parser


def main(argv: list[str] | None = None) -> int:
    return select(build_parser().parse_args(argv))


if __name__ == "__main__":
    raise SystemExit(main())
