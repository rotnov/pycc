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

    def test_primary_eval_executes_build_and_generated_program(self) -> None:
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
                stderr = (
                    b"error: unexpected argument '--fix' found\n"
                    b"Usage: pycc check [PATH]\n"
                )
                return subprocess.CompletedProcess(arguments, 2, b"", stderr)
            if len(arguments) > 1 and arguments[1] == "build":
                if Path(arguments[2]).name == "program.py":
                    return subprocess.CompletedProcess(arguments, 0, b"", b"")
                return subprocess.CompletedProcess(
                    arguments,
                    1,
                    b"",
                    b"error[L0001]: synthetic parser error\n",
                )
            return subprocess.CompletedProcess(arguments, 0, b"42\n", b"")

        evals.run_evals(
            "codex",
            Path(__file__),
            runner=runner,
        )

        self.assertEqual(len(calls), 8)
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
                and Path(arguments[2]).name == "parser-error.py"
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
            return subprocess.CompletedProcess(arguments, 0, b"", b"")

        with self.assertRaisesRegex(evals.EvalError, "invalid invocation"):
            evals.run_pycc_check_rejection(
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

    def test_unknown_client_fails_closed(self) -> None:
        with self.assertRaisesRegex(evals.EvalError, "unknown client"):
            evals.canonical_skill("other", "pycc")

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
            with self.assertRaisesRegex(evals.EvalError, "bind exactly"):
                evals.load_cases("pycc", root)

    def test_runnerless_feedback_case_is_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            path = root / ".claude" / "skills" / "pycc-feedback" / "evals"
            path.mkdir(parents=True)
            payload = json.loads(
                (
                    evals.ROOT
                    / ".claude/skills/pycc-feedback/evals/evals.json"
                ).read_text(encoding="utf-8")
            )
            payload["evals"].append(
                {
                    "id": 4,
                    "prompt": "runnerless",
                    "expected_output": "must not be skipped",
                }
            )
            (path / "evals.json").write_text(
                json.dumps(payload),
                encoding="utf-8",
            )

            with self.assertRaisesRegex(evals.EvalError, "bind exactly"):
                evals.load_cases("pycc-feedback", root)

    @mock.patch.object(evals.hardening, "run_command")
    def test_default_runner_converts_timeout_to_eval_failure(
        self,
        run: mock.Mock,
    ) -> None:
        run.side_effect = evals.hardening.EvalCommandTimeout(
            "command timed out after 30s: pycc build program.py"
        )

        with self.assertRaisesRegex(evals.EvalError, "timed out after 30s"):
            evals.run_command(["pycc", "build", "program.py"], evals.ROOT)

    @mock.patch.object(evals.hardening, "behavioral_evidence_failures")
    @mock.patch.object(evals.hardening, "feedback_reproduction_failures")
    @mock.patch.object(evals.hardening, "runtime_failures")
    @mock.patch.object(evals.hardening, "contract_failures")
    def test_codex_hardening_uses_the_selected_compiler(
        self,
        contracts: mock.Mock,
        runtime: mock.Mock,
        feedback: mock.Mock,
        evidence: mock.Mock,
    ) -> None:
        contracts.return_value = []
        runtime.return_value = []
        feedback.return_value = []
        evidence.return_value = []
        selected = Path("/tmp/selected-pycc")

        evals.run_hardening_checks("codex", selected)

        contracts.assert_called_once_with("codex")
        runtime.assert_called_once_with(pycc_bin=selected)
        feedback.assert_called_once_with(pycc_bin=selected)
        evidence.assert_called_once_with()

    @mock.patch.object(evals.hardening, "behavioral_evidence_failures")
    @mock.patch.object(evals.hardening, "feedback_reproduction_failures")
    @mock.patch.object(evals.hardening, "runtime_failures")
    @mock.patch.object(evals.hardening, "contract_failures")
    def test_claude_hardening_does_not_repeat_client_neutral_runtime(
        self,
        contracts: mock.Mock,
        runtime: mock.Mock,
        feedback: mock.Mock,
        evidence: mock.Mock,
    ) -> None:
        contracts.return_value = []

        evals.run_hardening_checks("claude", Path("/tmp/selected-pycc"))

        contracts.assert_called_once_with("claude")
        runtime.assert_not_called()
        feedback.assert_not_called()
        evidence.assert_not_called()

    @mock.patch.object(evals.hardening, "contract_failures")
    def test_hardening_failures_stop_the_offline_gate(
        self,
        contracts: mock.Mock,
    ) -> None:
        contracts.return_value = ["contract drift"]

        with self.assertRaisesRegex(evals.EvalError, "contract drift"):
            evals.run_hardening_checks("claude", Path("/tmp/selected-pycc"))


if __name__ == "__main__":
    unittest.main()
