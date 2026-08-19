#!/usr/bin/env python3
"""Fail-closed routing for CI jobs affected by changed repository paths."""

from __future__ import annotations

import argparse
import fcntl
import os
import sys
from dataclasses import dataclass
from typing import Sequence


@dataclass(frozen=True)
class Selection:
    compiler: bool
    pages: bool
    agent: bool

    @classmethod
    def full(cls) -> "Selection":
        return cls(True, True, True)


EMPTY_SELECTION = Selection(False, False, False)

# Every script executed by a compiler/performance or Pages heavy gate is bound
# here. Other scripts are governance inputs and do not require a heavy gate.
COMPILER_GATE_SCRIPTS = frozenset(
    {
        "scripts/check_frontend_throughput.rb",
        "scripts/check_replicated_paired_perf_regression.rb",
        "scripts/test_check_replicated_paired_perf_regression.rb",
    }
)
PAGES_GATE_SCRIPTS = frozenset(
    {
        "scripts/check_pages_performance_budget.rb",
        "scripts/check_site_accessibility.rb",
        "scripts/check_site_aria_conformance.py",
        "scripts/check_site_reduced_motion.js",
        "scripts/run_pages_lighthouse.py",
        "scripts/run_pages_lighthouse_accessibility.py",
        "scripts/serve_pages_fixture.py",
    }
)
AGENT_GATE_SCRIPTS = frozenset(
    {
        "scripts/run_alpha_skill_evals.py",
        "scripts/validate_agent_assets.py",
        "scripts/validate_agent_policies.py",
    }
)
PAGES_FIXTURES = frozenset(
    {
        "tests/fixtures/pages-performance-budget.json",
        "tests/fixtures/pages-performance-manifest.json",
    }
)
COMPILER_ROOTS = ("src/", "crates/", ".cargo/", "tests/", "benches/")
AGENT_ROOTS = (".agents/", ".claude/", ".harden/", ".ievo/")
COMPILER_FILES = frozenset(
    {
        "Cargo.toml",
        "Cargo.lock",
        "build.rs",
        "docs/DIAGNOSTICS.md",
        # Both of these documents are test inputs, not only prose.
        # `docs/DIAGNOSTICS.md` is read by the diagnostics-registry tests, and
        # `docs/PYTHON_STANDARDS.md` is read by `tests/conformance_matrix_guard.rs`
        # (issue #591). Left in the `docs/` bucket below they would classify as
        # EMPTY_SELECTION, so the one change most likely to break each guard --
        # editing the document it asserts on -- would be the change that skips it,
        # surfacing only on the later `main` push run.
        "docs/PYTHON_STANDARDS.md",
        "rust-toolchain",
        "rust-toolchain.toml",
    }
)
AGENT_FILES = frozenset({"AGENTS.md", "CLAUDE.md", "skills-lock.json"})
DOCUMENTATION_FILES = frozenset({"README.md", "LICENSE", "CITATION.cff"})


def _is_valid_repository_path(path: str) -> bool:
    if not isinstance(path, str) or not path:
        return False
    if path.startswith("/") or "\x00" in path or "\n" in path or "\r" in path:
        return False
    return all(part not in {"", ".", ".."} for part in path.split("/"))


def _selection_for_path(path: str) -> Selection | None:
    if path in {
        ".github/workflows/ci.yml",
        "scripts/classify_ci_changes.py",
        "scripts/test_classify_ci_changes.py",
    }:
        return Selection.full()
    if path in PAGES_FIXTURES:
        return Selection(True, True, False)
    if path.startswith("tests/fixtures/policy-successors/"):
        return EMPTY_SELECTION
    if path == "tests/fixtures/policy-successor-manifest.json":
        return EMPTY_SELECTION
    if path in COMPILER_GATE_SCRIPTS:
        return Selection(True, False, False)
    if path in COMPILER_FILES or path.startswith(COMPILER_ROOTS):
        # The offline agent contract evals execute the freshly built compiler
        # and bind diagnostics/backend boundaries (including D-072), so every
        # compiler input must select those evals as well as the compiler jobs.
        return Selection(True, False, True)
    if path in PAGES_GATE_SCRIPTS or path.startswith("site/"):
        return Selection(False, True, False)
    if path in AGENT_GATE_SCRIPTS or path in AGENT_FILES or path.startswith(AGENT_ROOTS):
        return Selection(False, False, True)
    if (
        path.startswith("docs/")
        or path.startswith("scripts/")
        or path.startswith(".github/workflows/")
        or path in DOCUMENTATION_FILES
    ):
        return EMPTY_SELECTION
    return None


def classify_paths(paths: Sequence[str], *, event_name: str) -> Selection:
    """Return the union of reviewed categories, failing closed by default."""
    if event_name == "push":
        return Selection.full()
    if event_name != "pull_request" or not paths:
        return Selection.full()

    selection = EMPTY_SELECTION
    for path in paths:
        if not _is_valid_repository_path(path):
            return Selection.full()
        matched = _selection_for_path(path)
        if matched is None:
            return Selection.full()
        selection = Selection(
            compiler=selection.compiler or matched.compiler,
            pages=selection.pages or matched.pages,
            agent=selection.agent or matched.agent,
        )
    return selection


def _parse_nul_delimited_paths(stream: bytes, *, event_name: str) -> Selection:
    if event_name == "pull_request" and stream and not stream.endswith(b"\x00"):
        return Selection.full()
    if not stream:
        return classify_paths([], event_name=event_name)
    try:
        paths = [path.decode("utf-8") for path in stream[:-1].split(b"\x00")]
    except UnicodeDecodeError:
        return Selection.full()
    return classify_paths(paths, event_name=event_name)


def _write_github_output(path: str, selection: Selection) -> None:
    output = (
        f"compiler={str(selection.compiler).lower()}\n"
        f"pages={str(selection.pages).lower()}\n"
        f"agent={str(selection.agent).lower()}\n"
    ).encode("ascii")
    descriptor = os.open(path, os.O_WRONLY | os.O_APPEND | os.O_CREAT, 0o600)
    try:
        fcntl.flock(descriptor, fcntl.LOCK_EX)
        offset = 0
        while offset < len(output):
            offset += os.write(descriptor, output[offset:])
    finally:
        os.close(descriptor)


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--event-name", required=True)
    parser.add_argument("--github-output", required=True)
    arguments = parser.parse_args(argv)

    selection = _parse_nul_delimited_paths(
        sys.stdin.buffer.read(), event_name=arguments.event_name
    )
    try:
        _write_github_output(arguments.github_output, selection)
    except OSError:
        print("could not write GitHub output", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
