#!/usr/bin/env python3
"""Run deterministic offline contract checks for the alpha project skills.

These checks resolve each client's repository entrypoint and exercise code and
consent invariants without invoking an LLM. Authenticated model-response evals
remain a separate promotion requirement for the alpha skills.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import subprocess
import sys
import tempfile
from collections.abc import Callable
from dataclasses import dataclass
from pathlib import Path
from typing import Any


ROOT = Path(__file__).resolve().parents[1]
COMMAND_TIMEOUT_SECONDS = 30
CANONICAL_REFERENCE = re.compile(
    r"`(?P<path>\.claude/skills/(?P<name>[a-z][a-z0-9-]*)/SKILL\.md)`"
)
EXPECTED_RUNNERS = {
    "pycc": {
        "build-and-run-self-created-fixture",
        "classify-planned-backend-boundary-without-write",
        "observe-current-check-fix-rejection",
    },
    "pycc-feedback": {
        "refuse-accepted-pr5-boundary-publication",
        "refuse-private-automatic-publication",
        "require-exact-payload-preview",
    },
}
LOCKED_RESEARCH_CASES = {
    1: (
        "incremental compiler cache",
        ("preventive brief", "shipped fixes", "regression tests"),
    ),
    2: (
        "subinterpreters",
        ("version-aware applicability", "competing hypotheses", "experiment"),
    ),
    3: (
        "cooperative cancellation",
        ("mechanism-oriented", "analogical evidence", "resource cleanup"),
    ),
    4: (
        "obscure custom bytecode",
        ("negative result", "residual uncertainty", "profiler experiment"),
    ),
}
FEEDBACK_CONTRACT = (
    "explicit approval",
    "exact payload",
    "make no external change",
    "sanitize every outbound query",
    "user-authored code",
)
CommandRunner = Callable[
    [list[str], Path],
    subprocess.CompletedProcess[bytes],
]


class EvalError(RuntimeError):
    """An alpha-skill offline contract was not satisfied."""


@dataclass(frozen=True)
class SubmissionState:
    exact_payload_previewed: bool
    explicit_confirmation_after_preview: bool
    payload_unchanged: bool


def submission_allowed(state: SubmissionState) -> bool:
    return (
        state.exact_payload_previewed
        and state.explicit_confirmation_after_preview
        and state.payload_unchanged
    )


def maybe_publish(
    state: SubmissionState,
    publisher: Callable[[], None],
) -> bool:
    if not submission_allowed(state):
        return False
    publisher()
    return True


def run_command(
    arguments: list[str],
    cwd: Path,
) -> subprocess.CompletedProcess[bytes]:
    try:
        return subprocess.run(
            arguments,
            cwd=cwd,
            check=False,
            capture_output=True,
            timeout=COMMAND_TIMEOUT_SECONDS,
        )
    except subprocess.TimeoutExpired as error:
        raise EvalError(
            f"alpha eval command timed out after {COMMAND_TIMEOUT_SECONDS}s: "
            f"{arguments[0]}"
        ) from error


def canonical_skill(
    client: str,
    name: str,
    root: Path = ROOT,
) -> str:
    canonical_path = root / ".claude" / "skills" / name / "SKILL.md"
    if client == "claude":
        entrypoint = canonical_path
    elif client == "codex":
        entrypoint = root / ".agents" / "skills" / name / "SKILL.md"
    else:
        raise EvalError(f"unknown client {client!r}")

    try:
        entrypoint_text = entrypoint.read_text(encoding="utf-8")
    except OSError as error:
        raise EvalError(f"could not read {entrypoint}: {error}") from error

    if client == "codex":
        references = list(CANONICAL_REFERENCE.finditer(entrypoint_text))
        if len(references) != 1 or references[0].group("name") != name:
            raise EvalError(
                f"{entrypoint} must resolve exactly one canonical {name} skill"
            )
        canonical_path = root / references[0].group("path")

    try:
        return canonical_path.read_text(encoding="utf-8")
    except OSError as error:
        raise EvalError(f"could not read {canonical_path}: {error}") from error


def load_cases(name: str, root: Path = ROOT) -> list[dict[str, Any]]:
    path = root / ".claude" / "skills" / name / "evals" / "evals.json"
    try:
        payload = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise EvalError(f"could not load {path}: {error}") from error
    cases = payload.get("evals") if isinstance(payload, dict) else None
    if not isinstance(payload, dict) or payload.get("skill_name") != name:
        raise EvalError(f"{path} does not define evals for {name}")
    if not isinstance(cases, list):
        raise EvalError(f"{path} does not define evals for {name}")
    if not all(
        isinstance(case, dict)
        and isinstance(case.get("id"), int)
        and isinstance(case.get("prompt"), str)
        and isinstance(case.get("expected_output"), str)
        for case in cases
    ):
        raise EvalError(f"{path} contains a malformed eval")
    if name == "i-have-an-issue":
        identifiers = [case["id"] for case in cases]
        if identifiers != list(LOCKED_RESEARCH_CASES):
            raise EvalError(
                f"{path} must retain every reviewed research scenario"
            )
    else:
        if not all(isinstance(case.get("runner"), str) for case in cases):
            raise EvalError(f"{path} requires an executable runner for every eval")
        runner_names = {case["runner"] for case in cases}
        if runner_names != EXPECTED_RUNNERS[name]:
            raise EvalError(
                f"{path} must bind exactly the executable runners for {name}"
            )
    return cases


def require_success(
    result: subprocess.CompletedProcess[bytes],
    label: str,
) -> None:
    if result.returncode != 0:
        stderr = result.stderr.decode("utf-8", errors="replace")
        raise EvalError(f"{label} failed with {result.returncode}: {stderr}")


def run_pycc_success(
    case: dict[str, Any],
    skill_text: str,
    pycc_binary: Path,
    root: Path = ROOT,
    runner: CommandRunner = run_command,
) -> None:
    prompt = case["prompt"]
    expected = case["expected_output"]
    if "temporary self-contained" not in prompt:
        raise EvalError("pycc success eval must create its own temporary fixture")
    if "exact command, exit code, stdout, and stderr" not in expected:
        raise EvalError("pycc success eval must require complete command evidence")
    for contract in ("cargo build --workspace", "unique temporary directory"):
        if contract not in skill_text:
            raise EvalError(f"pycc skill is missing {contract!r}")
    if not pycc_binary.is_file():
        raise EvalError(f"pycc binary does not exist: {pycc_binary}")

    with tempfile.TemporaryDirectory(prefix="pycc-alpha-eval-") as directory:
        temporary = Path(directory)
        source = temporary / "program.py"
        source.write_text("print(42)\n", encoding="utf-8")
        executable_name = "program.exe" if os.name == "nt" else "program"
        executable = temporary / executable_name
        build = runner(
            [
                str(pycc_binary),
                "build",
                str(source),
                "-o",
                str(executable),
            ],
            root,
        )
        require_success(build, "alpha pycc build eval")
        execution = runner([str(executable)], root)
        require_success(execution, "alpha pycc executable eval")
        if execution.stdout != b"42\n" or execution.stderr != b"":
            raise EvalError(
                "alpha pycc executable must emit exactly stdout '42\\n' "
                "and empty stderr"
            )
        pycc_run = runner([str(pycc_binary), "run", str(source)], root)
        require_success(pycc_run, "alpha pycc run eval")
        if pycc_run.stdout != b"42\n" or pycc_run.stderr != b"":
            raise EvalError(
                "alpha pycc run must emit exactly stdout '42\\n' "
                "and empty stderr"
            )


def run_pycc_check_rejection(
    case: dict[str, Any],
    skill_text: str,
    pycc_binary: Path,
    root: Path = ROOT,
    runner: CommandRunner = run_command,
) -> None:
    if "check --fix" not in case["prompt"]:
        raise EvalError("pycc check eval must exercise the planned --fix path")
    expected = case["expected_output"]
    for fragment in (
        "implemented check command",
        "T0021 diagnostic",
        "--fix is not parsed",
    ):
        if fragment not in expected:
            raise EvalError("pycc check eval has an incomplete expected output")
    normalized_skill = " ".join(skill_text.split())
    for contract in (
        "strict type-checker subset",
        "`check --fix` is not parsed",
        "exit `1`",
    ):
        if contract not in normalized_skill:
            raise EvalError(f"pycc skill is missing {contract!r}")

    with tempfile.TemporaryDirectory(prefix="pycc-alpha-check-eval-") as directory:
        source = Path(directory) / "type-error.py"
        source.write_text(
            'for item in range("three"):\n    print(item)\n',
            encoding="utf-8",
        )
        check = runner([str(pycc_binary), "check", str(source)], root)
        if (
            check.returncode != 1
            or b"error[T0021]" not in check.stdout
            or check.stderr != b""
        ):
            raise EvalError(
                "implemented pycc check must emit the observed T0021 diagnostic"
            )

        fix = runner(
            [str(pycc_binary), "check", str(source), "--fix"],
            root,
        )
        stderr = fix.stderr.decode("utf-8", errors="replace")
        if (
            fix.returncode != 2
            or fix.stdout != b""
            or "unexpected argument '--fix' found" not in stderr
        ):
            raise EvalError(
                "pycc check --fix must remain an observed invalid invocation"
            )


def run_pycc_boundary(
    case: dict[str, Any],
    skill_text: str,
    pycc_binary: Path,
    root: Path = ROOT,
    runner: CommandRunner = run_command,
) -> None:
    if not all(
        fragment in case["prompt"]
        for fragment in ("passes pycc check", "pycc build panics")
    ):
        raise EvalError(
            "pycc failure eval must compare check with the backend panic"
        )
    expected = case["expected_output"]
    for fragment in (
        "check succeeds",
        "build exits 101",
        "pycc_mir backend lowering",
        "intentional temporary alpha boundary",
        "rather than a reportable defect",
        "does not offer or post pycc feedback",
    ):
        if fragment not in expected:
            raise EvalError("pycc failure eval has an incomplete expected output")
    normalized_skill = " ".join(skill_text.split())
    for contract in (
        "smallest self-contained",
        "D-035",
        "intentional temporary alpha boundary",
        "$pycc-feedback",
    ):
        if contract not in normalized_skill:
            raise EvalError(f"pycc skill is missing {contract!r}")

    with tempfile.TemporaryDirectory(prefix="pycc-alpha-failure-eval-") as directory:
        temporary = Path(directory)
        source = temporary / "backend-panic.py"
        source.write_text(
            "def main() -> None:\n"
            "    value = 1\n"
            "    print(42)\n\n"
            "main()\n",
            encoding="utf-8",
        )
        check = runner([str(pycc_binary), "check", str(source)], root)
        if check.returncode != 0 or check.stdout != b"" or check.stderr != b"":
            raise EvalError(
                "backend panic fixture must first pass frontend-only check"
            )

        executable_name = (
            "backend-panic.exe" if os.name == "nt" else "backend-panic"
        )
        build = runner(
            [
                str(pycc_binary),
                "build",
                str(source),
                "-o",
                str(temporary / executable_name),
            ],
            root,
        )
        if (
            build.returncode != 101
            or build.stdout != b""
            or b"panicked at" not in build.stderr
            or b"pycc_mir" not in build.stderr
        ):
            raise EvalError(
                "backend fixture must reproduce the current exit-101 MIR panic"
            )


def run_issue_research_case(
    case: dict[str, Any],
    skill_text: str,
    root: Path = ROOT,
    runner: CommandRunner = run_command,
) -> None:
    case_contract = LOCKED_RESEARCH_CASES.get(case["id"])
    if case_contract is None:
        raise EvalError(f"unknown i-have-an-issue eval {case['id']!r}")
    prompt_fragment, expected_fragments = case_contract
    if prompt_fragment not in case["prompt"]:
        raise EvalError(
            f"i-have-an-issue eval {case['id']} changed its reviewed prompt"
        )
    if not all(
        fragment in case["expected_output"]
        for fragment in expected_fragments
    ):
        raise EvalError(
            f"i-have-an-issue eval {case['id']} has incomplete evidence criteria"
        )
    normalized = " ".join(skill_text.split())
    for contract in (
        "Prefer primary evidence",
        "closed issue is not proof that a fix shipped",
        "Cluster evidence by failure mechanism",
        "remaining uncertainty",
        "useful negative result",
    ):
        if contract not in normalized:
            raise EvalError(f"i-have-an-issue skill is missing {contract!r}")

    helper = (
        root
        / ".claude"
        / "skills"
        / "i-have-an-issue"
        / "scripts"
        / "search_github.py"
    )
    result = runner([sys.executable, str(helper), "--help"], root)
    require_success(result, f"i-have-an-issue eval {case['id']} helper")


def run_feedback_case(
    case: dict[str, Any],
    skill_text: str,
) -> None:
    # This is a deterministic safety oracle for the checked-in scenario, not a
    # simulation of either client's model response. It keeps external writes
    # structurally impossible in required CI.
    normalized = " ".join(skill_text.split())
    for contract in FEEDBACK_CONTRACT:
        if contract not in normalized:
            raise EvalError(f"pycc-feedback skill is missing {contract!r}")

    runner_name = case["runner"]
    expected = case["expected_output"]
    publications: list[str] = []
    if runner_name == "refuse-accepted-pr5-boundary-publication":
        state = SubmissionState(False, False, True)
        required = (
            "Must reproduce",
            "read D-035",
            "not reportable",
            "no GitHub payload",
        )
    elif runner_name == "refuse-private-automatic-publication":
        state = SubmissionState(False, False, False)
        required = ("Must refuse automatic", "exact per-payload approval")
    elif runner_name == "require-exact-payload-preview":
        state = SubmissionState(False, True, True)
        required = ("Must not treat context-free consent", "exact repository")
    else:
        raise EvalError(f"unknown feedback runner {runner_name!r}")

    if not all(fragment in expected for fragment in required):
        raise EvalError(f"{runner_name} has an incomplete expected output")
    published = maybe_publish(state, lambda: publications.append("published"))
    if published or publications:
        raise EvalError(f"{runner_name} performed an unapproved external write")


def run_evals(
    client: str,
    pycc_binary: Path,
    root: Path = ROOT,
    runner: CommandRunner = run_command,
) -> None:
    pycc_skill = canonical_skill(client, "pycc", root)
    pycc_cases = load_cases("pycc", root)
    pycc_dispatch = {
        "build-and-run-self-created-fixture": run_pycc_success,
        "classify-planned-backend-boundary-without-write": run_pycc_boundary,
        "observe-current-check-fix-rejection": run_pycc_check_rejection,
    }
    for case in pycc_cases:
        runner_name = case.get("runner")
        handler = pycc_dispatch.get(runner_name)
        if handler is None:
            raise EvalError(f"unknown pycc runner {runner_name!r}")
        handler(case, pycc_skill, pycc_binary, root, runner)

    feedback_skill = canonical_skill(client, "pycc-feedback", root)
    for case in load_cases("pycc-feedback", root):
        run_feedback_case(case, feedback_skill)

    research_skill = canonical_skill(client, "i-have-an-issue", root)
    for case in load_cases("i-have-an-issue", root):
        run_issue_research_case(case, research_skill, root, runner)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--client",
        choices=("codex", "claude"),
        required=True,
        help=(
            "repository entrypoint to resolve; this offline runner does not "
            "launch the named client or a language model"
        ),
    )
    parser.add_argument("--pycc-bin", type=Path, required=True)
    arguments = parser.parse_args()
    try:
        run_evals(
            arguments.client,
            arguments.pycc_bin.resolve(),
        )
    except (EvalError, StopIteration) as error:
        print(f"error: {error}")
        return 1
    print(f"offline alpha contract evals ({arguments.client} entrypoint): valid")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
