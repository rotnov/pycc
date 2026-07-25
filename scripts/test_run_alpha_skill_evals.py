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

    def test_feedback_evals_do_not_publish_without_exact_consent(self) -> None:
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
