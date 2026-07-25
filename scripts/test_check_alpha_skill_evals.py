#!/usr/bin/env python3
"""Regression tests for deterministic alpha-skill eval execution."""

from __future__ import annotations

import json
import os
import signal
import shutil
import sys
import tempfile
import time
import unittest
from pathlib import Path
from unittest import mock

import check_alpha_skill_evals as evaluator


class AlphaSkillEvalTests(unittest.TestCase):
    def test_contracts_pass_for_both_client_entrypoints(self) -> None:
        self.assertEqual(evaluator.contract_failures("codex"), [])
        self.assertEqual(evaluator.contract_failures("claude"), [])

    def test_missing_expected_output_phrase_is_rejected(self) -> None:
        failures = evaluator.case_contract_failures(
            "The skill requires explicit approval.",
            {
                "expected_output": "Wait for approval.",
                "contract": {
                    "skill_must_contain": ["explicit approval"],
                    "expected_output_must_contain": ["exact payload"],
                },
            },
            "feedback eval",
        )
        self.assertTrue(any("exact payload" in failure for failure in failures))

    def test_negated_skill_requirement_is_rejected(self) -> None:
        failures = evaluator.case_contract_failures(
            "Never request explicit approval.",
            {
                "expected_output": "Waits for explicit approval.",
                "contract": {
                    "skill_must_contain": ["explicit approval"],
                    "expected_output_must_contain": ["explicit approval"],
                },
            },
            "feedback eval",
        )
        self.assertTrue(any("negated context" in failure for failure in failures))

    def test_distant_negated_skill_requirement_is_rejected(self) -> None:
        failures = evaluator.case_contract_failures(
            "Never under any circumstances ask the user for explicit approval.",
            {
                "expected_output": "Waits for explicit approval.",
                "contract": {
                    "skill_must_contain": ["explicit approval"],
                    "expected_output_must_contain": ["explicit approval"],
                },
            },
            "feedback eval",
        )
        self.assertTrue(any("negated context" in failure for failure in failures))

    def test_contradictory_skill_requirement_is_rejected(self) -> None:
        failures = evaluator.case_contract_failures(
            "Ask for explicit approval. Never ask for explicit approval.",
            {
                "expected_output": "Waits for explicit approval.",
                "contract": {
                    "skill_must_contain": ["explicit approval"],
                    "expected_output_must_contain": ["explicit approval"],
                },
            },
            "feedback eval",
        )
        self.assertTrue(any("negated context" in failure for failure in failures))

    def test_suffix_negated_skill_requirement_is_rejected(self) -> None:
        failures = evaluator.case_contract_failures(
            "Explicit approval must never be requested.",
            {
                "expected_output": "Waits for explicit approval.",
                "contract": {
                    "skill_must_contain": ["explicit approval"],
                    "expected_output_must_contain": ["explicit approval"],
                },
            },
            "feedback eval",
        )
        self.assertTrue(any("negated context" in failure for failure in failures))

    def test_contracted_negations_are_rejected(self) -> None:
        for skill_text in (
            "Don't request explicit approval.",
            "Explicit approval isn't required.",
            "Ask for explicit approval. Don't ask for explicit approval.",
            "Explicit approval isn’t required.",
        ):
            with self.subTest(skill_text=skill_text):
                failures = evaluator.case_contract_failures(
                    skill_text,
                    {
                        "expected_output": "Waits for explicit approval.",
                        "contract": {
                            "skill_must_contain": ["explicit approval"],
                            "expected_output_must_contain": [
                                "explicit approval"
                            ],
                        },
                    },
                    "feedback eval",
                )
                self.assertTrue(
                    any("negated context" in failure for failure in failures)
                )

    def test_negated_expected_action_is_rejected(self) -> None:
        failures = evaluator.case_contract_failures(
            "Ask for explicit approval.",
            {
                "expected_output": (
                    "Does not wait for explicit approval before any GitHub write."
                ),
                "contract": {
                    "skill_must_contain": ["explicit approval"],
                    "expected_output_must_contain": ["explicit approval"],
                    "expected_output_must_require": ["explicit approval"],
                },
            },
            "feedback eval",
        )
        self.assertTrue(any("negated context" in failure for failure in failures))

    def test_approval_unacceptable_is_not_a_positive_exception(self) -> None:
        failures = evaluator.case_contract_failures(
            "A preview does not make explicit approval acceptable.",
            {
                "expected_output": "Waits for explicit approval.",
                "contract": {
                    "skill_must_contain": ["explicit approval"],
                    "expected_output_must_contain": ["explicit approval"],
                },
            },
            "feedback eval",
        )
        self.assertTrue(any("negated context" in failure for failure in failures))

    def test_rejecting_a_panic_as_acceptable_is_a_positive_requirement(
        self,
    ) -> None:
        failures = evaluator.case_contract_failures(
            "A slice limit does not make an uncaught panic acceptable.",
            {
                "expected_output": "Reports the uncaught panic.",
                "contract": {
                    "skill_must_contain": ["uncaught panic"],
                    "expected_output_must_contain": ["uncaught panic"],
                },
            },
            "feedback eval",
        )
        self.assertEqual(failures, [])

    def test_primary_success_eval_uses_an_inline_fixture(self) -> None:
        evals = evaluator.load_evals(evaluator.ROOT, "pycc")
        case = evals["evals"][0]
        self.assertNotIn("examples/hello.py", case["prompt"])
        self.assertEqual(case["runtime"]["source"], "print(42)\n")
        self.assertEqual(case["runtime"]["expected_stdout"], "42\n")

    def test_feedback_eval_includes_its_reproduction_source(self) -> None:
        evals = evaluator.load_evals(evaluator.ROOT, "pycc-feedback")
        case = evals["evals"][0]
        source = case["reproduction"]["source"]
        self.assertIn(source, case["prompt"])
        self.assertEqual(case["reproduction"]["expected_exit"], 101)
        self.assertEqual(
            case["reproduction"]["stderr_must_contain"],
            "panicked at",
        )

    def test_successful_command_is_captured(self) -> None:
        result = evaluator.run_command(
            [sys.executable, "-c", "print('ok')"],
            evaluator.ROOT,
            timeout_seconds=2,
        )
        self.assertEqual(result.returncode, 0)
        self.assertEqual(result.stdout, "ok\n")
        self.assertEqual(result.stderr, "")

    @mock.patch("check_alpha_skill_evals.run_command")
    def test_command_timeout_becomes_a_clear_eval_failure(
        self,
        run: mock.Mock,
    ) -> None:
        run.side_effect = evaluator.EvalCommandTimeout(
            "command timed out after 30s: pycc run program.py"
        )
        failures: list[str] = []
        result = evaluator.run_runtime_command(
            failures,
            "pycc eval 1",
            ["pycc", "run", "program.py"],
            Path("/tmp"),
        )
        self.assertIsNone(result)
        self.assertEqual(len(failures), 1)
        self.assertIn("timed out after 30s", failures[0])
        self.assertIn("pycc run program.py", failures[0])

    @unittest.skipIf(os.name == "nt", "POSIX process-group assertion")
    def test_timeout_terminates_descendant_process(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            pid_path = Path(directory) / "child.pid"
            child_code = "import time; time.sleep(60)"
            parent_code = (
                "import pathlib, subprocess, sys, time; "
                "child=subprocess.Popen([sys.executable, '-c', sys.argv[2]]); "
                "pathlib.Path(sys.argv[1]).write_text(str(child.pid)); "
                "time.sleep(60)"
            )
            with self.assertRaisesRegex(
                evaluator.EvalCommandTimeout,
                "timed out after 0.5s",
            ):
                evaluator.run_command(
                    [
                        sys.executable,
                        "-c",
                        parent_code,
                        str(pid_path),
                        child_code,
                    ],
                    evaluator.ROOT,
                    timeout_seconds=0.5,
                )
            child_pid = int(pid_path.read_text(encoding="utf-8"))
            deadline = time.monotonic() + 2
            while time.monotonic() < deadline:
                try:
                    os.kill(child_pid, 0)
                except ProcessLookupError:
                    break
                time.sleep(0.05)
            else:
                os.kill(child_pid, signal.SIGKILL)
                self.fail("timed-out command left its descendant alive")

    @unittest.skipIf(os.name == "nt", "POSIX process-group assertion")
    def test_timeout_kills_descendant_after_parent_exits(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            pid_path = Path(directory) / "child.pid"
            child_code = "import time; time.sleep(60)"
            parent_code = (
                "import pathlib, subprocess, sys; "
                "child=subprocess.Popen([sys.executable, '-c', sys.argv[2]]); "
                "pathlib.Path(sys.argv[1]).write_text(str(child.pid))"
            )
            started = time.monotonic()
            with self.assertRaisesRegex(
                evaluator.EvalCommandTimeout,
                "timed out after 0.2s",
            ):
                evaluator.run_command(
                    [
                        sys.executable,
                        "-c",
                        parent_code,
                        str(pid_path),
                        child_code,
                    ],
                    evaluator.ROOT,
                    timeout_seconds=0.2,
                )
            self.assertLess(time.monotonic() - started, 1.5)
            child_pid = int(pid_path.read_text(encoding="utf-8"))
            deadline = time.monotonic() + 2
            while time.monotonic() < deadline:
                try:
                    os.kill(child_pid, 0)
                except ProcessLookupError:
                    break
                time.sleep(0.05)
            else:
                os.kill(child_pid, signal.SIGKILL)
                self.fail("exited parent left its timed-out descendant alive")

    def copy_evidence_fixture(self, root: Path) -> dict:
        evidence_path = evaluator.ROOT / evaluator.BEHAVIORAL_EVIDENCE
        evidence = json.loads(evidence_path.read_text(encoding="utf-8"))
        target = root / evaluator.BEHAVIORAL_EVIDENCE
        target.parent.mkdir(parents=True)
        for skill_name in evaluator.ALPHA_SKILLS:
            shutil.copytree(
                evaluator.ROOT / ".claude" / "skills" / skill_name,
                root / ".claude" / "skills" / skill_name,
            )
            codex_target = root / ".agents" / "skills" / skill_name
            codex_target.mkdir(parents=True)
            shutil.copy2(
                evaluator.ROOT / ".agents" / "skills" / skill_name / "SKILL.md",
                codex_target / "SKILL.md",
            )
        evidence["project_input_sha256"] = evaluator.project_input_sha256(root)
        target.write_text(json.dumps(evidence), encoding="utf-8")
        return evidence

    def test_stale_canonical_tree_evidence_is_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            self.copy_evidence_fixture(root)
            asset = root / ".claude/skills/pycc-feedback/assets/bug-report.md"
            asset.write_text("changed preview shape\n", encoding="utf-8")
            failures = evaluator.behavioral_evidence_failures(root)
        self.assertTrue(
            any("canonical_tree_sha256 is stale" in item for item in failures)
        )
        self.assertTrue(
            any("project_input_sha256 is stale" in item for item in failures)
        )

    def test_project_input_fingerprint_detects_non_skill_drift(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            self.copy_evidence_fixture(root)
            (root / "src").mkdir()
            (root / "src" / "compiler.rs").write_text(
                "changed compiler input\n",
                encoding="utf-8",
            )
            failures = evaluator.behavioral_evidence_failures(root)
        self.assertTrue(
            any("project_input_sha256 is stale" in item for item in failures)
        )

    def test_stale_codex_entrypoint_evidence_is_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            self.copy_evidence_fixture(root)
            wrapper = root / ".agents/skills/pycc/SKILL.md"
            wrapper.write_text("changed wrapper\n", encoding="utf-8")
            failures = evaluator.behavioral_evidence_failures(root)
        self.assertTrue(
            any("codex_entrypoint_sha256 is stale" in item for item in failures)
        )

    def test_stale_claude_entrypoint_evidence_is_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            self.copy_evidence_fixture(root)
            entrypoint = root / ".claude/skills/pycc/SKILL.md"
            entrypoint.write_text("changed entrypoint\n", encoding="utf-8")
            failures = evaluator.behavioral_evidence_failures(root)
        self.assertTrue(
            any("claude_entrypoint_sha256 is stale" in item for item in failures)
        )
        self.assertTrue(
            any("canonical_tree_sha256 is stale" in item for item in failures)
        )

    def test_invalid_evidence_metadata_is_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            evidence = self.copy_evidence_fixture(root)
            evidence["schema_version"] = 999
            evidence["source_revision"] = "0"
            evidence["executed_at"] = "not-a-timestamp"
            evidence["clients"]["codex"]["version"] = "x"
            evidence["clients"]["codex"]["entrypoints"] = []
            evidence["clients"]["codex"]["cases"][0]["observation"] = "x"
            target = root / evaluator.BEHAVIORAL_EVIDENCE
            target.write_text(json.dumps(evidence), encoding="utf-8")
            failures = evaluator.behavioral_evidence_failures(root)
        joined = "\n".join(failures)
        self.assertIn("schema_version", joined)
        self.assertIn("source_revision", joined)
        self.assertIn("executed_at", joined)
        self.assertIn("codex version", joined)
        self.assertIn("codex entrypoints", joined)
        self.assertIn("observation is too weak", joined)

    def test_every_manifest_case_requires_unique_evidence(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            evidence = self.copy_evidence_fixture(root)
            evidence["clients"]["claude"]["cases"] = evidence["clients"]["claude"][
                "cases"
            ][:-1]
            evidence["clients"]["codex"]["cases"].append(
                evidence["clients"]["codex"]["cases"][0]
            )
            target = root / evaluator.BEHAVIORAL_EVIDENCE
            target.write_text(json.dumps(evidence), encoding="utf-8")
            failures = evaluator.behavioral_evidence_failures(root)
        joined = "\n".join(failures)
        self.assertIn("claude case coverage mismatch", joined)
        self.assertIn("codex duplicates", joined)

    def test_behavioral_observations_must_contain_case_evidence(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            evidence = self.copy_evidence_fixture(root)
            for client in evidence["clients"].values():
                for case in client["cases"]:
                    case["observation"] = (
                        "This sentence has enough words but proves absolutely "
                        "nothing about the required behavior."
                    )
            target = root / evaluator.BEHAVIORAL_EVIDENCE
            target.write_text(json.dumps(evidence), encoding="utf-8")
            failures = evaluator.behavioral_evidence_failures(root)
        self.assertTrue(
            any("observation lacks required evidence" in item for item in failures)
        )


if __name__ == "__main__":
    unittest.main()
