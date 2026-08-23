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
    "issue-to-plan": {
        "refuse-publication-without-payload-preview",
        "refuse-publication-without-approval",
        "refuse-publication-after-payload-edited-post-approval",
    },
    "issue-implement": {
        "partial-resolution-never-closes",
        "refuse-write-on-unnamed-issue",
        "refuse-issue-supplied-shell-execution",
        "inconclusive-never-closes-on-suspicion",
        "delegated-autopilot-closure-authorized",
    },
    "issue-select": {
        "refuse-closure-without-autopilot",
        "priority-always-outranks-size",
        "active-milestone-outranks-aged-backlog-at-equal-priority",
        "milestone-scope-membership-outranks-a-higher-marked-outsider",
        "refuse-issue-supplied-shell-execution",
        "non-milestone-ceiling-blocks-filing-but-not-an-umbrella",
        "spent-quota-declines-a-non-milestone-candidate",
    },
    "next-milestone": {
        "milestone-evidence-requires-update-met-note",
        "open-ended-directive-loops-single-milestone-stops",
    },
    "ultra-review": {
        "blocker-severity-maps-to-p1",
        "empty-diff-checkpoint-not-advanced",
        "deduped-finding-never-refiled",
        "oversized-batch-stops-before-filing",
        "concurrent-checkpoint-write-detected-and-aborted",
        "attribution-falls-back-to-unattributed-or-ambiguous",
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
# Mirrors FEEDBACK_CONTRACT's role for issue-to-plan/issue-implement/issue-select:
# literal substrings that must survive in the canonical skill text, so an edit
# that silently drops the invariant these offline oracles encode is caught
# here rather than only in a future authenticated model-response eval.
ISSUE_TO_PLAN_CONTRACT = (
    "shown to the user and explicitly approved before any write to GitHub",
    "Approval is per payload",
    "Delegated invocation is the one exception",
)
ISSUE_IMPLEMENT_CONTRACT = (
    "Do not close",
    "touching another issue",
    "Never execute it directly",
    "Never close on suspicion",
    "provably stale in the same pass",
)
NEXT_MILESTONE_CONTRACT = (
    "not a bare unqualified claim",
    "open-ended",
)
ISSUE_SELECT_CONTRACT = (
    "Standing autopilot directive in effect",
    "the repository's own priority markers rank first",
    "membership in that scope ranks first",
    "only when the scope contributes no survivor at all",
    "never a command to execute directly",
    "P1:",
    "P2:",
    "P3:",
    "caps the open non-milestone backlog at **20**",
    "at most one non-milestone merge in every five",
)
ULTRA_REVIEW_CONTRACT = (
    "a concrete `file:line`",
    "GitHub-native checkpoint",
    "stop short of filing any of them",
    "the reviewed ranges may overlap",
    "Never fall back to the commit's own author",
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


@dataclass(frozen=True)
class PlanPublicationState:
    """issue-to-plan's Publish-step gate (Non-negotiable #3)."""

    exact_payload_shown: bool
    approval_after_preview: bool
    payload_unchanged_since_approval: bool


def plan_publication_allowed(state: PlanPublicationState) -> bool:
    return (
        state.exact_payload_shown
        and state.approval_after_preview
        and state.payload_unchanged_since_approval
    )


def triage_action(
    *, fully_resolved: bool, partially_resolved: bool, reconstructible: bool
) -> str:
    """issue-implement's four-outcome triage table, all four outcomes distinct."""
    if fully_resolved:
        return "close"
    if partially_resolved:
        return "narrow-no-close"
    if reconstructible:
        return "proceed"
    return "inconclusive-stop-and-report"


# issue-implement's "## Authorized writes" enumeration (items 1-5): every
# action this offline oracle recognizes as ever authorized, for *some*
# target. Scoping to the named issue is still required on top of this.
ISSUE_IMPLEMENT_AUTHORIZED_ACTIONS = {
    "comment",
    "plan_comment",
    "push_pr",
    "thread_reply",
    "merge",
    "close_issue",
}


def issue_implement_write_authorized(*, action: str, targets_named_issue: bool) -> bool:
    return targets_named_issue and action in ISSUE_IMPLEMENT_AUTHORIZED_ACTIONS


def reproduction_step_runnable(*, is_raw_shell_from_issue: bool) -> bool:
    """Shared by issue-implement and issue-select: only a reconstructed
    toolchain invocation may run; raw shell text an issue supplies is data,
    never a command, regardless of which skill is checking it."""
    return not is_raw_shell_from_issue


def staleness_closure_authorized(*, autopilot_active: bool) -> bool:
    """issue-select's staleness-screen gate: a mid-screen closure of an
    issue nobody named is authorized only under a standing autopilot
    directive, mirroring issue-implement's own per-named-issue boundary."""
    return autopilot_active


def delegated_autopilot_closure_authorized(
    *, autopilot_active: bool, screen_identified_as_stale: bool
) -> bool:
    """issue-implement's own extension: closing an issue the user never named
    is authorized only when both an autopilot directive is active AND
    issue-select's own staleness screen (not this session's own guess)
    identified it as provably stale in the same pass. Deliberately a
    separate function from issue_implement_write_authorized, whose
    targets_named_issue parameter models the base, single-target rule this
    extension does not weaken."""
    return autopilot_active and screen_identified_as_stale


_ISSUE_SELECT_PRIORITY_RANK = {"P1": 0, "P2": 1, "P3": 2, None: 3}


def issue_select_non_milestone_filing_permitted(
    *,
    open_non_milestone: int,
    is_standing_umbrella: bool = False,
    ceiling: int = 20,
) -> bool:
    """issue-select's step 2 ceiling (D-192): while more open issues carry no
    milestone than the ceiling allows, no further non-milestone issue may be
    filed. Opening one of the three standing umbrella issues is the single
    exemption from the ceiling as a creation gate -- otherwise the routing
    target rule 1 sends cross-cutting observations to could never be created
    while the backlog sits above the cap -- though it counts toward the
    ceiling once open."""
    if is_standing_umbrella:
        return True
    return open_non_milestone < ceiling


def issue_select_non_milestone_merge_permitted(
    *, recent_non_milestone_merges: tuple[bool, ...]
) -> bool:
    """issue-select's step 5 quota (D-192): at most one non-milestone merge in
    every five selection-output merges, so proposing a non-milestone candidate
    is permitted only when none of the four preceding ones was itself
    non-milestone. A merge delivering an umbrella-issue checklist item closes
    no issue yet is selection output, so the caller counts it as non-milestone
    here rather than skipping it."""
    return not any(recent_non_milestone_merges[:4])


def issue_select_higher_ranked(
    *,
    priority: str | None,
    effort: int,
    other_priority: str | None,
    other_effort: int,
    milestone_scope_in_effect: bool = False,
    active_milestone: bool = False,
    other_active_milestone: bool = False,
) -> bool:
    """issue-select's step 5 scoring order (D-191, superseding D-144's
    tie-break): when a milestone scope is in effect, membership in that scope
    ranks first, ahead of the priority marker -- so an out-of-scope issue is
    reached only once the scope contributes no survivor. Inside a group
    (in-scope, or out-of-scope among themselves) the order is priority marker
    then size, so a large P1 still outranks a tiny P2. With no milestone scope
    in effect the membership component is inert for both sides, leaving exactly
    the priority-then-size order."""
    key = (
        milestone_scope_in_effect and not active_milestone,
        _ISSUE_SELECT_PRIORITY_RANK[priority],
        effort,
    )
    other_key = (
        milestone_scope_in_effect and not other_active_milestone,
        _ISSUE_SELECT_PRIORITY_RANK[other_priority],
        other_effort,
    )
    return key < other_key


def next_milestone_loop_continues(*, directive_scope: str) -> bool:
    """A directive naming exactly one milestone stops at step 6 once it
    completes; an open-ended directive re-enters step 1."""
    return directive_scope == "open-ended"


_ULTRA_REVIEW_SEVERITY_PRIORITY = {"blocker": "P1", "warning": "P2", "note": "P3"}


def ultra_review_severity_priority(severity: str) -> str:
    """ultra-review step 5's fixed severity-to-priority mapping -- the same
    blocker/warning/note scale the pinned deep-reviewer already returns."""
    try:
        return _ULTRA_REVIEW_SEVERITY_PRIORITY[severity]
    except KeyError as error:
        raise EvalError(f"unknown ultra-review severity {severity!r}") from error


def ultra_review_checkpoint_should_advance(*, diff_is_empty: bool) -> bool:
    """ultra-review step 3/9: an empty diff since the last checkpoint is a
    clean no-op -- no dispatch, and the checkpoint issue is left untouched."""
    return not diff_is_empty


def ultra_review_may_file(
    *, has_file_line_evidence: bool, already_tracked: bool
) -> bool:
    """ultra-review step 7's publish gate: a finding is only ever filed when
    it carries concrete file:line evidence AND step 6's dedup pass found no
    existing `ultra-review`-labeled issue already tracking it."""
    return has_file_line_evidence and not already_tracked


ULTRA_REVIEW_BATCH_GUARD_THRESHOLD = 15


def ultra_review_batch_within_guard(*, candidate_count: int) -> bool:
    """ultra-review step 7's batch-size guard: a run whose dedup-survived
    candidate count exceeds this threshold stops short of filing any of them
    and reports the batch instead of auto-filing a flood."""
    return candidate_count <= ULTRA_REVIEW_BATCH_GUARD_THRESHOLD


def ultra_review_checkpoint_write_is_safe(orig_sha: str, fresh_sha: str) -> bool:
    """ultra-review step 9's overlapping-range guard: a fresh re-read of the
    checkpoint issue immediately before writing must still show the same
    `Last reviewed commit` this run read back in step 2. A mismatch means a
    concurrent run already advanced the checkpoint, so this run must not
    write at all -- writing on top would double-count whatever range the
    concurrent run already reviewed."""
    return orig_sha == fresh_sha


def ultra_review_attribution_bucket(
    blamed_sha_is_in_range: bool, trailer_names: list[str]
) -> str:
    """ultra-review step 8's per-finding attribution bucket. A blamed line
    outside the reviewed range is always `unattributed`, regardless of any
    trailer names supplied -- a pre-existing line's trailers are never
    consulted. Inside the range: zero distinct trailer names is
    `unattributed`; exactly one distinct name is attributed to that name,
    unless it contains a literal `,` or `:` (unsafe to serialize into the
    checkpoint's `Cumulative by model` line, so it folds into `ambiguous`
    instead); two or more distinct names is `ambiguous`. Names are
    deduplicated before counting -- a squash-merged commit can repeat an
    identical `Co-Authored-By` trailer line several times, and that is one
    distinct name, not several."""
    if not blamed_sha_is_in_range:
        return "unattributed"
    distinct_names = set(trailer_names)
    if not distinct_names:
        return "unattributed"
    if len(distinct_names) > 1:
        return "ambiguous"
    (name,) = distinct_names
    if "," in name or ":" in name:
        return "ambiguous"
    return name


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
        "pycc_codegen backend lowering",
        "intentional temporary alpha boundary",
        "rather than a reportable defect",
        "does not offer or post pycc feedback",
    ):
        if fragment not in expected:
            raise EvalError("pycc failure eval has an incomplete expected output")
    normalized_skill = " ".join(skill_text.split())
    for contract in (
        "smallest self-contained",
        "D-072",
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
            "    value = print(42)\n\n"
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
            or b"pycc_codegen: using print()'s result as a nested expression "
            b"is not supported yet" not in build.stderr
        ):
            raise EvalError(
                "backend fixture must reproduce the exact current exit-101 "
                "D-072 codegen panic"
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
            "read D-072",
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


def run_issue_to_plan_case(case: dict[str, Any], skill_text: str) -> None:
    # Deterministic safety oracle for the checked-in scenario, mirroring
    # run_feedback_case: no real GitHub write is structurally possible here.
    normalized = " ".join(skill_text.split())
    for contract in ISSUE_TO_PLAN_CONTRACT:
        if contract not in normalized:
            raise EvalError(f"issue-to-plan skill is missing {contract!r}")

    runner_name = case["runner"]
    expected = case["expected_output"]
    if runner_name == "refuse-publication-without-payload-preview":
        state = PlanPublicationState(False, True, True)
        required = ("payload was never shown", "payload-preview gate")
    elif runner_name == "refuse-publication-without-approval":
        state = PlanPublicationState(True, False, True)
        required = ("no explicit approval", "published yet")
    elif runner_name == "refuse-publication-after-payload-edited-post-approval":
        state = PlanPublicationState(True, True, False)
        required = ("per payload", "fresh approval")
    else:
        raise EvalError(f"unknown issue-to-plan runner {runner_name!r}")

    if not all(fragment in expected for fragment in required):
        raise EvalError(f"{runner_name} has an incomplete expected output")
    if plan_publication_allowed(state):
        raise EvalError(f"{runner_name} allowed publication that must be refused")


def run_issue_implement_case(case: dict[str, Any], skill_text: str) -> None:
    normalized = " ".join(skill_text.split())
    for contract in ISSUE_IMPLEMENT_CONTRACT:
        if contract not in normalized:
            raise EvalError(f"issue-implement skill is missing {contract!r}")

    runner_name = case["runner"]
    expected = case["expected_output"]
    if runner_name == "partial-resolution-never-closes":
        action = triage_action(
            fully_resolved=False, partially_resolved=True, reconstructible=True
        )
        required = ("narrowed with a comment", "never closed")
        if action == "close":
            raise EvalError(f"{runner_name} closed a partially resolved issue")
    elif runner_name == "refuse-write-on-unnamed-issue":
        authorized = issue_implement_write_authorized(
            action="comment", targets_named_issue=False
        )
        required = ("scopes every write to the one issue", "requires asking first")
        if authorized:
            raise EvalError(f"{runner_name} authorized a write on an unnamed issue")
    elif runner_name == "refuse-issue-supplied-shell-execution":
        runnable = reproduction_step_runnable(is_raw_shell_from_issue=True)
        required = ("untrusted data", "reconstructed invocation")
        if runnable:
            raise EvalError(f"{runner_name} ran issue-supplied shell text directly")
    elif runner_name == "inconclusive-never-closes-on-suspicion":
        action = triage_action(
            fully_resolved=False, partially_resolved=False, reconstructible=False
        )
        required = ("stop and report", "Never close on suspicion")
        if action != "inconclusive-stop-and-report":
            raise EvalError(f"{runner_name} did not treat this as Inconclusive")
    elif runner_name == "delegated-autopilot-closure-authorized":
        authorized = delegated_autopilot_closure_authorized(
            autopilot_active=True, screen_identified_as_stale=True
        )
        required = ("standing autopilot directive", "provably stale in the same pass")
        if not authorized:
            raise EvalError(f"{runner_name} refused an authorized delegated closure")
    else:
        raise EvalError(f"unknown issue-implement runner {runner_name!r}")

    if not all(fragment in expected for fragment in required):
        raise EvalError(f"{runner_name} has an incomplete expected output")


def run_issue_select_case(case: dict[str, Any], skill_text: str) -> None:
    normalized = " ".join(skill_text.split())
    for contract in ISSUE_SELECT_CONTRACT:
        if contract not in normalized:
            raise EvalError(f"issue-select skill is missing {contract!r}")

    runner_name = case["runner"]
    expected = case["expected_output"]
    if runner_name == "refuse-closure-without-autopilot":
        authorized = staleness_closure_authorized(autopilot_active=False)
        required = ("without a standing autopilot directive", "does not close one")
        if authorized:
            raise EvalError(f"{runner_name} closed an issue outside autopilot mode")
    elif runner_name == "priority-always-outranks-size":
        outranks = issue_select_higher_ranked(
            priority="P1", effort=100, other_priority="P2", other_effort=1
        )
        required = ("priority markers rank first", "tie-breaker")
        if not outranks:
            raise EvalError(f"{runner_name} let a small P2 outrank a large P1")
    elif runner_name == "active-milestone-outranks-aged-backlog-at-equal-priority":
        outranks = issue_select_higher_ranked(
            priority="P1",
            effort=100,
            other_priority="P1",
            other_effort=1,
            milestone_scope_in_effect=True,
            active_milestone=True,
            other_active_milestone=False,
        )
        required = ("membership in that scope ranks first", "regardless of size")
        if not outranks:
            raise EvalError(
                f"{runner_name} let a smaller out-of-scope P1 "
                f"outrank a larger in-scope P1"
            )
    elif runner_name == "milestone-scope-membership-outranks-a-higher-marked-outsider":
        outranks = issue_select_higher_ranked(
            priority="P2",
            effort=50,
            other_priority="P1",
            other_effort=1,
            milestone_scope_in_effect=True,
            active_milestone=True,
            other_active_milestone=False,
        )
        unreachable_without_scope = issue_select_higher_ranked(
            priority="P2",
            effort=50,
            other_priority="P1",
            other_effort=1,
        )
        required = (
            "membership in that scope ranks first",
            "no survivor at all",
        )
        if not outranks:
            raise EvalError(
                f"{runner_name} left the in-scope member unreachable behind a "
                f"higher-marked out-of-scope issue"
            )
        if unreachable_without_scope:
            raise EvalError(
                f"{runner_name} changed the no-scope ordering, where the "
                f"priority marker still ranks first"
            )
    elif runner_name == "non-milestone-ceiling-blocks-filing-but-not-an-umbrella":
        blocked = issue_select_non_milestone_filing_permitted(open_non_milestone=70)
        umbrella = issue_select_non_milestone_filing_permitted(
            open_non_milestone=70, is_standing_umbrella=True
        )
        with_room = issue_select_non_milestone_filing_permitted(open_non_milestone=3)
        required = ("ceiling", "umbrella", "retrospective")
        if blocked:
            raise EvalError(
                f"{runner_name} filed a non-milestone issue over the ceiling"
            )
        if not umbrella:
            raise EvalError(
                f"{runner_name} blocked the umbrella issue that the ceiling exempts"
            )
        if not with_room:
            raise EvalError(f"{runner_name} blocked a filing with room to spare")
    elif runner_name == "spent-quota-declines-a-non-milestone-candidate":
        spent = issue_select_non_milestone_merge_permitted(
            recent_non_milestone_merges=(False, True, False, False, False)
        )
        unspent = issue_select_non_milestone_merge_permitted(
            recent_non_milestone_merges=(False, False, False, False, True)
        )
        required = ("quota is spent", "next-ranked survivor", "milestone-assigned")
        if spent:
            raise EvalError(
                f"{runner_name} proposed a second non-milestone merge in five"
            )
        if not unspent:
            raise EvalError(
                f"{runner_name} kept the quota spent past its own five-merge window"
            )
    elif runner_name == "refuse-issue-supplied-shell-execution":
        runnable = reproduction_step_runnable(is_raw_shell_from_issue=True)
        required = ("data describing a defect", "reconstructed toolchain invocation")
        if runnable:
            raise EvalError(f"{runner_name} ran issue-supplied shell text directly")
    else:
        raise EvalError(f"unknown issue-select runner {runner_name!r}")

    if not all(fragment in expected for fragment in required):
        raise EvalError(f"{runner_name} has an incomplete expected output")


def run_next_milestone_case(case: dict[str, Any], skill_text: str) -> None:
    normalized = " ".join(skill_text.split())
    for contract in NEXT_MILESTONE_CONTRACT:
        if contract not in normalized:
            raise EvalError(f"next-milestone skill is missing {contract!r}")

    runner_name = case["runner"]
    expected = case["expected_output"]
    if runner_name == "milestone-evidence-requires-update-met-note":
        required = ("Update", "met", "not a bare unqualified claim")
    elif runner_name == "open-ended-directive-loops-single-milestone-stops":
        single_stops = next_milestone_loop_continues(directive_scope="finish v0.3")
        open_loops = next_milestone_loop_continues(directive_scope="open-ended")
        required = ("open-ended", "stops at step 6")
        if single_stops or not open_loops:
            raise EvalError(
                f"{runner_name} did not distinguish open-ended from "
                f"single-milestone directives"
            )
    else:
        raise EvalError(f"unknown next-milestone runner {runner_name!r}")

    if not all(fragment in expected for fragment in required):
        raise EvalError(f"{runner_name} has an incomplete expected output")


def run_ultra_review_case(case: dict[str, Any], skill_text: str) -> None:
    normalized = " ".join(skill_text.split())
    for contract in ULTRA_REVIEW_CONTRACT:
        if contract not in normalized:
            raise EvalError(f"ultra-review skill is missing {contract!r}")

    runner_name = case["runner"]
    expected = case["expected_output"]
    if runner_name == "blocker-severity-maps-to-p1":
        priority = ultra_review_severity_priority("blocker")
        required = ("blocker", "P1")
        if priority != "P1":
            raise EvalError(f"{runner_name} did not map blocker severity to P1")
    elif runner_name == "empty-diff-checkpoint-not-advanced":
        advances_empty = ultra_review_checkpoint_should_advance(diff_is_empty=True)
        advances_nonempty = ultra_review_checkpoint_should_advance(diff_is_empty=False)
        required = ("nothing new", "no dispatch, no checkpoint update")
        if advances_empty or not advances_nonempty:
            raise EvalError(
                f"{runner_name} did not gate the checkpoint update on a "
                f"non-empty diff"
            )
    elif runner_name == "deduped-finding-never-refiled":
        may_file = ultra_review_may_file(
            has_file_line_evidence=True, already_tracked=True
        )
        required = ("already tracked", "never re-filed")
        if may_file:
            raise EvalError(
                f"{runner_name} filed a finding the dedup pass already found tracked"
            )
    elif runner_name == "oversized-batch-stops-before-filing":
        within_guard = ultra_review_batch_within_guard(candidate_count=16)
        required = ("stop short of filing any of them", "report the batch")
        if within_guard:
            raise EvalError(f"{runner_name} let an oversized batch pass the guard")
    elif runner_name == "concurrent-checkpoint-write-detected-and-aborted":
        safe_when_unchanged = ultra_review_checkpoint_write_is_safe("abc123", "abc123")
        safe_when_changed = ultra_review_checkpoint_write_is_safe("abc123", "def456")
        required = ("reviewed ranges may overlap", "does not write")
        if not safe_when_unchanged or safe_when_changed:
            raise EvalError(
                f"{runner_name} did not detect the overlapping-range race correctly"
            )
    elif runner_name == "attribution-falls-back-to-unattributed-or-ambiguous":
        zero_names = ultra_review_attribution_bucket(True, [])
        two_names = ultra_review_attribution_bucket(
            True, ["Claude Sonnet 5", "Codex"]
        )
        out_of_range = ultra_review_attribution_bucket(False, ["Claude Sonnet 5"])
        escaped_name = ultra_review_attribution_bucket(True, ["Doe, Jane"])
        required = ("unattributed", "ambiguous")
        if (
            zero_names != "unattributed"
            or two_names != "ambiguous"
            or out_of_range != "unattributed"
            or escaped_name != "ambiguous"
        ):
            raise EvalError(
                f"{runner_name} did not fall back to unattributed/ambiguous correctly"
            )
    else:
        raise EvalError(f"unknown ultra-review runner {runner_name!r}")

    if not all(fragment in expected for fragment in required):
        raise EvalError(f"{runner_name} has an incomplete expected output")


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

    issue_to_plan_skill = canonical_skill(client, "issue-to-plan", root)
    for case in load_cases("issue-to-plan", root):
        run_issue_to_plan_case(case, issue_to_plan_skill)

    issue_implement_skill = canonical_skill(client, "issue-implement", root)
    for case in load_cases("issue-implement", root):
        run_issue_implement_case(case, issue_implement_skill)

    issue_select_skill = canonical_skill(client, "issue-select", root)
    for case in load_cases("issue-select", root):
        run_issue_select_case(case, issue_select_skill)

    next_milestone_skill = canonical_skill(client, "next-milestone", root)
    for case in load_cases("next-milestone", root):
        run_next_milestone_case(case, next_milestone_skill)

    ultra_review_skill = canonical_skill(client, "ultra-review", root)
    for case in load_cases("ultra-review", root):
        run_ultra_review_case(case, ultra_review_skill)


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
