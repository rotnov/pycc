from __future__ import annotations

import json
import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest import mock

import run_alpha_skill_evals as evals


class AlphaSkillEvalTests(unittest.TestCase):
    def test_both_clients_resolve_the_same_canonical_skills(self) -> None:
        for name in ("pycc", "pycc-feedback"):
            self.assertEqual(
                evals.canonical_skill("codex", name),
                evals.canonical_skill("claude", name),
            )

    def test_primary_eval_executes_both_supported_run_paths(self) -> None:
        calls: list[list[str]] = []

        def runner(
            arguments: list[str],
            _cwd: Path,
        ) -> subprocess.CompletedProcess[bytes]:
            calls.append(arguments)
            stdout = b"" if len(calls) == 1 else b"42\n"
            return subprocess.CompletedProcess(arguments, 0, stdout, b"")

        case = next(
            case
            for case in evals.load_cases("pycc")
            if case.get("runner") == "build-and-run-self-created-fixture"
        )
        evals.run_pycc_success(
            case,
            evals.canonical_skill("claude", "pycc"),
            Path(__file__),
            runner=runner,
        )

        self.assertEqual(calls[0][1], "build")
        self.assertEqual(calls[0][3], "-o")
        self.assertEqual(len(calls[1]), 1)
        self.assertEqual(calls[2][1], "run")

    def test_primary_eval_fails_when_compilation_fails(self) -> None:
        def runner(
            arguments: list[str],
            _cwd: Path,
        ) -> subprocess.CompletedProcess[bytes]:
            return subprocess.CompletedProcess(arguments, 1, b"", b"compile failed")

        case = next(
            case
            for case in evals.load_cases("pycc")
            if case.get("runner") == "build-and-run-self-created-fixture"
        )
        with self.assertRaisesRegex(evals.EvalError, "compile failed"):
            evals.run_pycc_success(
                case,
                evals.canonical_skill("codex", "pycc"),
                Path(__file__),
                runner=runner,
            )

    def test_primary_eval_rejects_wrong_program_output(self) -> None:
        call_count = 0

        def runner(
            arguments: list[str],
            _cwd: Path,
        ) -> subprocess.CompletedProcess[bytes]:
            nonlocal call_count
            call_count += 1
            stdout = b"" if call_count == 1 else b"41\n"
            return subprocess.CompletedProcess(arguments, 0, stdout, b"")

        case = next(
            case
            for case in evals.load_cases("pycc")
            if case.get("runner") == "build-and-run-self-created-fixture"
        )
        with self.assertRaisesRegex(evals.EvalError, "emit exactly"):
            evals.run_pycc_success(
                case,
                evals.canonical_skill("claude", "pycc"),
                Path(__file__),
                runner=runner,
            )

    def test_full_dispatch_executes_every_declared_scenario(self) -> None:
        calls: list[list[str]] = []

        def runner(
            arguments: list[str],
            _cwd: Path,
        ) -> subprocess.CompletedProcess[bytes]:
            calls.append(arguments)
            if arguments[-1] == "--help":
                return subprocess.CompletedProcess(arguments, 0, b"usage\n", b"")
            if len(arguments) > 1 and arguments[1] == "check":
                if "--fix" in arguments:
                    stderr = b"error: unexpected argument '--fix' found\n"
                    return subprocess.CompletedProcess(arguments, 2, b"", stderr)
                if Path(arguments[2]).name == "type-error.py":
                    return subprocess.CompletedProcess(
                        arguments,
                        1,
                        b"error[T0021]: range stop expects `int`, got `str`\n",
                        b"",
                    )
                return subprocess.CompletedProcess(arguments, 0, b"", b"")
            if len(arguments) > 1 and arguments[1] == "build":
                if Path(arguments[2]).name == "program.py":
                    return subprocess.CompletedProcess(arguments, 0, b"", b"")
                return subprocess.CompletedProcess(
                    arguments,
                    101,
                    b"",
                    b"thread 'main' panicked at pycc_codegen: using print()'s "
                    b"result as a nested expression is not supported yet\n",
                )
            return subprocess.CompletedProcess(arguments, 0, b"42\n", b"")

        evals.run_evals(
            "codex",
            Path(__file__),
            runner=runner,
        )

        self.assertEqual(len(calls), 11)
        self.assertEqual(
            sum(arguments[-1] == "--help" for arguments in calls),
            4,
        )
        self.assertTrue(
            any(len(arguments) > 1 and arguments[1] == "check" for arguments in calls)
        )
        self.assertTrue(
            any(
                len(arguments) > 2
                and Path(arguments[2]).name == "backend-panic.py"
                for arguments in calls
            )
        )

    def test_check_fix_eval_rejects_a_false_success(self) -> None:
        case = next(
            case
            for case in evals.load_cases("pycc")
            if case.get("runner") == "observe-current-check-fix-rejection"
        )

        def runner(
            arguments: list[str],
            _cwd: Path,
        ) -> subprocess.CompletedProcess[bytes]:
            if "--fix" not in arguments:
                return subprocess.CompletedProcess(
                    arguments,
                    1,
                    b"error[T0021]: range stop expects `int`, got `str`\n",
                    b"",
                )
            return subprocess.CompletedProcess(arguments, 0, b"", b"")

        with self.assertRaisesRegex(evals.EvalError, "invalid invocation"):
            evals.run_pycc_check_rejection(
                case,
                evals.canonical_skill("codex", "pycc"),
                Path(__file__),
                runner=runner,
            )

    def test_backend_boundary_eval_requires_the_current_mir_panic(self) -> None:
        case = next(
            case
            for case in evals.load_cases("pycc")
            if case.get("runner") == "classify-planned-backend-boundary-without-write"
        )
        call_count = 0

        def runner(
            arguments: list[str],
            _cwd: Path,
        ) -> subprocess.CompletedProcess[bytes]:
            nonlocal call_count
            call_count += 1
            if call_count == 1:
                return subprocess.CompletedProcess(arguments, 0, b"", b"")
            return subprocess.CompletedProcess(
                arguments,
                1,
                b"",
                b"error[E0100]: controlled backend rejection\n",
            )

        with self.assertRaisesRegex(evals.EvalError, "exact current exit-101"):
            evals.run_pycc_boundary(
                case,
                evals.canonical_skill("claude", "pycc"),
                Path(__file__),
                runner=runner,
            )

    def test_backend_boundary_eval_rejects_an_unrelated_mir_panic(self) -> None:
        case = next(
            case
            for case in evals.load_cases("pycc")
            if case.get("runner") == "classify-planned-backend-boundary-without-write"
        )
        call_count = 0

        def runner(
            arguments: list[str],
            _cwd: Path,
        ) -> subprocess.CompletedProcess[bytes]:
            nonlocal call_count
            call_count += 1
            if call_count == 1:
                return subprocess.CompletedProcess(arguments, 0, b"", b"")
            return subprocess.CompletedProcess(
                arguments,
                101,
                b"",
                b"thread 'main' panicked at pycc_mir: unrelated invariant\n",
            )

        with self.assertRaisesRegex(evals.EvalError, "exact current exit-101"):
            evals.run_pycc_boundary(
                case,
                evals.canonical_skill("codex", "pycc"),
                Path(__file__),
                runner=runner,
            )

    def test_research_eval_rejects_incomplete_evidence_criteria(self) -> None:
        case = dict(evals.load_cases("i-have-an-issue")[0])
        case["expected_output"] = "generic answer"
        with self.assertRaisesRegex(evals.EvalError, "incomplete evidence"):
            evals.run_issue_research_case(
                case,
                evals.canonical_skill("claude", "i-have-an-issue"),
            )

    def test_feedback_contract_oracle_cannot_publish_without_exact_consent(
        self,
    ) -> None:
        skill = evals.canonical_skill("claude", "pycc-feedback")
        for case in evals.load_cases("pycc-feedback"):
            evals.run_feedback_case(case, skill)

    def test_submission_requires_preview_confirmation_and_stable_payload(self) -> None:
        denied = (
            evals.SubmissionState(False, True, True),
            evals.SubmissionState(True, False, True),
            evals.SubmissionState(True, True, False),
        )
        self.assertTrue(all(not evals.submission_allowed(state) for state in denied))
        allowed = evals.SubmissionState(True, True, True)
        self.assertTrue(evals.submission_allowed(allowed))

    def test_issue_to_plan_oracle_cannot_publish_without_exact_consent(self) -> None:
        skill = evals.canonical_skill("claude", "issue-to-plan")
        for case in evals.load_cases("issue-to-plan"):
            evals.run_issue_to_plan_case(case, skill)

    def test_issue_to_plan_publication_requires_all_three_gates(self) -> None:
        denied = (
            evals.PlanPublicationState(False, True, True),
            evals.PlanPublicationState(True, False, True),
            evals.PlanPublicationState(True, True, False),
        )
        self.assertTrue(
            all(not evals.plan_publication_allowed(state) for state in denied)
        )
        allowed = evals.PlanPublicationState(True, True, True)
        self.assertTrue(evals.plan_publication_allowed(allowed))

    def test_issue_to_plan_eval_fails_when_the_preview_gate_text_is_missing(
        self,
    ) -> None:
        # Normalize whitespace before removing the contract phrase: the
        # source markdown line-wraps mid-sentence, and the oracle itself
        # checks the phrase against normalized (not raw) text.
        raw = evals.canonical_skill("claude", "issue-to-plan")
        skill = " ".join(raw.split()).replace(
            "shown to the user and explicitly approved before any write to GitHub",
            "",
        )
        case = evals.load_cases("issue-to-plan")[0]
        with self.assertRaisesRegex(evals.EvalError, "is missing"):
            evals.run_issue_to_plan_case(case, skill)

    def test_issue_implement_oracle_covers_its_three_scenarios(self) -> None:
        skill = evals.canonical_skill("codex", "issue-implement")
        for case in evals.load_cases("issue-implement"):
            evals.run_issue_implement_case(case, skill)

    def test_issue_implement_triage_distinguishes_every_outcome(self) -> None:
        self.assertEqual(
            evals.triage_action(
                fully_resolved=False, partially_resolved=True, reconstructible=True
            ),
            "narrow-no-close",
        )
        self.assertEqual(
            evals.triage_action(
                fully_resolved=True, partially_resolved=False, reconstructible=True
            ),
            "close",
        )
        self.assertEqual(
            evals.triage_action(
                fully_resolved=False, partially_resolved=False, reconstructible=True
            ),
            "proceed",
        )
        self.assertEqual(
            evals.triage_action(
                fully_resolved=False, partially_resolved=False, reconstructible=False
            ),
            "inconclusive-stop-and-report",
        )

    def test_issue_implement_writes_require_both_a_named_issue_and_an_authorized_action(
        self,
    ) -> None:
        self.assertTrue(
            evals.issue_implement_write_authorized(
                action="comment", targets_named_issue=True
            )
        )
        self.assertFalse(
            evals.issue_implement_write_authorized(
                action="comment", targets_named_issue=False
            )
        )
        self.assertFalse(
            evals.issue_implement_write_authorized(
                action="edit_existing_comment", targets_named_issue=True
            )
        )
        self.assertFalse(
            evals.issue_implement_write_authorized(
                action="force_push_foreign", targets_named_issue=True
            )
        )

    def test_issue_implement_close_issue_is_an_authorized_action(self) -> None:
        self.assertTrue(
            evals.issue_implement_write_authorized(
                action="close_issue", targets_named_issue=True
            )
        )

    def test_reproduction_step_never_runs_raw_issue_supplied_shell_text(self) -> None:
        self.assertFalse(
            evals.reproduction_step_runnable(is_raw_shell_from_issue=True)
        )
        self.assertTrue(
            evals.reproduction_step_runnable(is_raw_shell_from_issue=False)
        )

    def test_issue_implement_eval_fails_when_the_authorization_boundary_text_is_missing(
        self,
    ) -> None:
        raw = evals.canonical_skill("codex", "issue-implement")
        skill = " ".join(raw.split()).replace("touching another issue", "")
        case = next(
            case
            for case in evals.load_cases("issue-implement")
            if case["runner"] == "refuse-write-on-unnamed-issue"
        )
        with self.assertRaisesRegex(evals.EvalError, "is missing"):
            evals.run_issue_implement_case(case, skill)

    def test_delegated_autopilot_closure_requires_both_conditions(self) -> None:
        self.assertTrue(
            evals.delegated_autopilot_closure_authorized(
                autopilot_active=True, screen_identified_as_stale=True
            )
        )
        self.assertFalse(
            evals.delegated_autopilot_closure_authorized(
                autopilot_active=False, screen_identified_as_stale=True
            )
        )
        self.assertFalse(
            evals.delegated_autopilot_closure_authorized(
                autopilot_active=True, screen_identified_as_stale=False
            )
        )

    def test_issue_select_oracle_covers_its_three_scenarios(self) -> None:
        skill = evals.canonical_skill("claude", "issue-select")
        for case in evals.load_cases("issue-select"):
            evals.run_issue_select_case(case, skill)

    def test_issue_select_closure_requires_a_standing_autopilot_directive(
        self,
    ) -> None:
        self.assertFalse(
            evals.staleness_closure_authorized(autopilot_active=False)
        )
        self.assertTrue(evals.staleness_closure_authorized(autopilot_active=True))

    def test_issue_select_priority_always_outranks_size(self) -> None:
        # A large P1 must still outrank a tiny P2 -- size is only a
        # same-priority tie-breaker, never a way to jump the priority queue.
        self.assertTrue(
            evals.issue_select_higher_ranked(
                priority="P1", effort=1000, other_priority="P2", other_effort=1
            )
        )
        self.assertFalse(
            evals.issue_select_higher_ranked(
                priority="P2", effort=1, other_priority="P1", other_effort=1000
            )
        )
        # Within the same priority, smaller genuinely wins.
        self.assertTrue(
            evals.issue_select_higher_ranked(
                priority="P2", effort=1, other_priority="P2", other_effort=5
            )
        )
        # An unmarked issue ranks last, behind every named priority.
        self.assertTrue(
            evals.issue_select_higher_ranked(
                priority="P3", effort=1, other_priority=None, other_effort=1
            )
        )

    def test_issue_select_eval_fails_when_the_scoring_order_text_is_missing(
        self,
    ) -> None:
        raw = evals.canonical_skill("claude", "issue-select")
        skill = " ".join(raw.split()).replace(
            "the repository's own priority markers rank first",
            "",
        )
        case = next(
            case
            for case in evals.load_cases("issue-select")
            if case["runner"] == "priority-always-outranks-size"
        )
        with self.assertRaisesRegex(evals.EvalError, "is missing"):
            evals.run_issue_select_case(case, skill)

    def test_unknown_runner_fails_closed_for_each_new_skill(self) -> None:
        skill_by_name = {
            "issue-to-plan": evals.canonical_skill("claude", "issue-to-plan"),
            "issue-implement": evals.canonical_skill("claude", "issue-implement"),
            "issue-select": evals.canonical_skill("claude", "issue-select"),
        }
        dispatch = {
            "issue-to-plan": evals.run_issue_to_plan_case,
            "issue-implement": evals.run_issue_implement_case,
            "issue-select": evals.run_issue_select_case,
        }
        for name, run_case in dispatch.items():
            case = dict(evals.load_cases(name)[0])
            case["runner"] = "no-such-runner"
            with self.assertRaisesRegex(evals.EvalError, "unknown .* runner"):
                run_case(case, skill_by_name[name])

    def test_unknown_client_fails_closed(self) -> None:
        with self.assertRaisesRegex(evals.EvalError, "unknown client"):
            evals.canonical_skill("other", "pycc")

    def test_command_timeout_fails_closed(self) -> None:
        with mock.patch.object(
            evals.subprocess,
            "run",
            side_effect=subprocess.TimeoutExpired(["pycc"], 30),
        ) as run:
            with self.assertRaisesRegex(evals.EvalError, "timed out after 30s"):
                evals.run_command(["pycc", "check"], evals.ROOT)
        self.assertEqual(run.call_args.kwargs["timeout"], 30)

    def test_extra_eval_without_runner_is_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            path = root / ".claude" / "skills" / "pycc" / "evals"
            path.mkdir(parents=True)
            cases = evals.load_cases("pycc")
            cases.append(
                {
                    "id": 4,
                    "prompt": "A future scenario.",
                    "expected_output": "A future result.",
                }
            )
            (path / "evals.json").write_text(
                json.dumps({"skill_name": "pycc", "evals": cases}),
                encoding="utf-8",
            )
            with self.assertRaisesRegex(evals.EvalError, "runner for every eval"):
                evals.load_cases("pycc", root)

    def test_eval_runner_set_is_exact(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            path = root / ".claude" / "skills" / "pycc" / "evals"
            path.mkdir(parents=True)
            (path / "evals.json").write_text(
                json.dumps(
                    {
                        "skill_name": "pycc",
                        "evals": [
                            {
                                "id": 1,
                                "prompt": "unused",
                                "expected_output": "unused",
                            }
                        ],
                    }
                ),
                encoding="utf-8",
            )
            with self.assertRaisesRegex(evals.EvalError, "runner"):
                evals.load_cases("pycc", root)


if __name__ == "__main__":
    unittest.main()
