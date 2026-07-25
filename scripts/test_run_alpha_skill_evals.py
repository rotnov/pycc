from __future__ import annotations

import json
import subprocess
import tempfile
import unittest
from pathlib import Path

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
            if len(arguments) > 2 and arguments[1:3] == ["check", "--fix"]:
                stderr = (
                    b"error: unexpected argument '--fix' found\n"
                    b"Usage: pycc check [OPTIONS] [PATH]\n"
                )
                return subprocess.CompletedProcess(arguments, 2, b"", stderr)
            if len(arguments) > 2 and arguments[1:3] == ["check", "--"]:
                return subprocess.CompletedProcess(arguments, 0, b"", b"")
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

        self.assertEqual(len(calls), 9)
        self.assertEqual(
            sum(arguments[-1] == "--help" for arguments in calls),
            4,
        )
        self.assertEqual(
            sum(len(arguments) > 1 and arguments[1] == "check" for arguments in calls),
            2,
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


if __name__ == "__main__":
    unittest.main()
