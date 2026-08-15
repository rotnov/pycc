#!/usr/bin/env python3
"""Behavior tests for fail-closed CI change routing.

Each literal expectation below catches a classifier branch that could otherwise
skip a CI gate affected by a repository change.
"""

from __future__ import annotations

import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

from classify_ci_changes import Selection, classify_paths


class ClassifyPathsTests(unittest.TestCase):
    def test_docs_only_skips_every_heavy_category(self):
        self.assertEqual(
            Selection(False, False, False),
            classify_paths(["docs/TESTING.md"], event_name="pull_request"),
        )

    def test_unknown_top_level_path_selects_everything(self):
        self.assertEqual(
            Selection(True, True, True),
            classify_paths(["future-build-input.toml"], event_name="pull_request"),
        )

    def test_mixed_site_and_compiler_change_unions_categories(self):
        self.assertEqual(
            Selection(True, True, False),
            classify_paths(
                ["crates/pycc_hir/src/lib.rs", "site/index.html"],
                event_name="pull_request",
            ),
        )

    def test_compiler_inputs_select_only_compiler(self):
        for path in (
            "src/main.rs",
            "crates/pycc_hir/src/lib.rs",
            "Cargo.toml",
            "Cargo.lock",
            "rust-toolchain",
            "rust-toolchain.toml",
            ".cargo/config.toml",
            "build.rs",
            "tests/conformance.rs",
            "benches/check_bench.rs",
        ):
            with self.subTest(path=path):
                self.assertEqual(
                    Selection(True, False, False),
                    classify_paths([path], event_name="pull_request"),
                )

    def test_compiler_and_performance_gate_scripts_select_compiler(self):
        for path in (
            "scripts/check_frontend_throughput.rb",
            "scripts/check_replicated_paired_perf_regression.rb",
            "scripts/test_check_replicated_paired_perf_regression.rb",
        ):
            with self.subTest(path=path):
                self.assertEqual(
                    Selection(True, False, False),
                    classify_paths([path], event_name="pull_request"),
                )

    def test_site_and_every_pages_gate_input_select_pages(self):
        for path, expected in (
            ("site/index.html", Selection(False, True, False)),
            ("scripts/run_pages_lighthouse.py", Selection(False, True, False)),
            (
                "scripts/run_pages_lighthouse_accessibility.py",
                Selection(False, True, False),
            ),
            (
                "scripts/check_pages_performance_budget.rb",
                Selection(False, True, False),
            ),
            ("scripts/check_site_accessibility.rb", Selection(False, True, False)),
            (
                "scripts/serve_pages_fixture.py",
                Selection(False, True, False),
            ),
            (
                "scripts/check_site_aria_conformance.py",
                Selection(False, True, False),
            ),
            (
                "scripts/check_site_reduced_motion.js",
                Selection(False, True, False),
            ),
            (
                "tests/fixtures/pages-performance-budget.json",
                Selection(True, True, False),
            ),
            (
                "tests/fixtures/pages-performance-manifest.json",
                Selection(True, True, False),
            ),
        ):
            with self.subTest(path=path):
                self.assertEqual(expected, classify_paths([path], event_name="pull_request"))

    def test_agent_roots_and_agent_gate_inputs_select_agent(self):
        for path in (
            ".agents/skills/example/SKILL.md",
            ".claude/skills/example/SKILL.md",
            ".harden/policy.yml",
            ".ievo/evolution/project.md",
            "AGENTS.md",
            "CLAUDE.md",
            "skills-lock.json",
            "scripts/run_alpha_skill_evals.py",
            "scripts/validate_agent_policies.py",
            "scripts/validate_agent_assets.py",
        ):
            with self.subTest(path=path):
                self.assertEqual(
                    Selection(False, False, True),
                    classify_paths([path], event_name="pull_request"),
                )

    def test_governance_only_paths_skip_every_heavy_category(self):
        for path in (
            "docs/TESTING.md",
            "scripts/check_ci_permissions.rb",
            "scripts/future_governance_check.py",
            ".github/workflows/link-check.yml",
            "tests/fixtures/policy-successor-manifest.json",
            "tests/fixtures/policy-successors/ci-d199.yml",
        ):
            with self.subTest(path=path):
                self.assertEqual(
                    Selection(False, False, False),
                    classify_paths([path], event_name="pull_request"),
                )

    def test_ci_workflow_and_classifier_self_changes_select_everything(self):
        for path in (
            ".github/workflows/ci.yml",
            "scripts/classify_ci_changes.py",
            "scripts/test_classify_ci_changes.py",
        ):
            with self.subTest(path=path):
                self.assertEqual(
                    Selection(True, True, True),
                    classify_paths([path], event_name="pull_request"),
                )

    def test_empty_or_unsupported_event_selects_everything(self):
        self.assertEqual(
            Selection(True, True, True),
            classify_paths([], event_name="pull_request"),
        )
        self.assertEqual(
            Selection(True, True, True),
            classify_paths(["docs/TESTING.md"], event_name="workflow_dispatch"),
        )
        self.assertEqual(
            Selection(True, True, True),
            classify_paths(["docs/TESTING.md"], event_name="push"),
        )

    def test_invalid_paths_select_everything(self):
        for path in (
            "/tmp/escape.rs",
            "../escape.rs",
            "src/../escape.rs",
            "site/\x00index.html",
            "site/index\n.html",
            "",
        ):
            with self.subTest(path=repr(path)):
                self.assertEqual(
                    Selection(True, True, True),
                    classify_paths([path], event_name="pull_request"),
                )

    def test_added_deleted_and_renamed_path_pairs_union_categories(self):
        self.assertEqual(
            Selection(True, False, False),
            classify_paths(
                ["src/added.rs", "src/deleted.rs", "crates/old.rs", "crates/new.rs"],
                event_name="pull_request",
            ),
        )
        self.assertEqual(
            Selection(False, True, False),
            classify_paths(
                ["site/old.html", "site/new.html"], event_name="pull_request"
            ),
        )


class ClassifyCliTests(unittest.TestCase):
    SCRIPT = Path(__file__).with_name("classify_ci_changes.py")

    def run_cli(self, *, stream: bytes, event_name: str, output_path: Path):
        return subprocess.run(
            [
                sys.executable,
                "-B",
                str(self.SCRIPT),
                "--event-name",
                event_name,
                "--github-output",
                str(output_path),
            ],
            input=stream,
            capture_output=True,
        )

    def test_nul_delimited_site_change_writes_exact_fixed_output(self):
        with tempfile.TemporaryDirectory() as temporary_directory:
            output_path = Path(temporary_directory) / "github-output"
            result = self.run_cli(
                stream=b"site/index.html\x00",
                event_name="pull_request",
                output_path=output_path,
            )

            self.assertEqual(0, result.returncode)
            self.assertEqual(
                "compiler=false\npages=true\nagent=false\n",
                output_path.read_text(),
            )
            self.assertNotIn(b"site/index.html", result.stdout + result.stderr)

    def test_push_selects_everything_regardless_of_path_stream(self):
        with tempfile.TemporaryDirectory() as temporary_directory:
            output_path = Path(temporary_directory) / "github-output"
            result = self.run_cli(
                stream=b"docs/TESTING.md\x00",
                event_name="push",
                output_path=output_path,
            )

            self.assertEqual(0, result.returncode)
            self.assertEqual(
                "compiler=true\npages=true\nagent=true\n", output_path.read_text()
            )

    def test_output_is_appended_without_replacing_prior_step_output(self):
        with tempfile.TemporaryDirectory() as temporary_directory:
            output_path = Path(temporary_directory) / "github-output"
            output_path.write_text("prior=true\n")
            result = self.run_cli(
                stream=b"docs/TESTING.md\x00",
                event_name="pull_request",
                output_path=output_path,
            )

            self.assertEqual(0, result.returncode)
            self.assertEqual(
                "prior=true\ncompiler=false\npages=false\nagent=false\n",
                output_path.read_text(),
            )

    def test_nonterminated_pull_request_stream_fails_closed_to_everything(self):
        with tempfile.TemporaryDirectory() as temporary_directory:
            output_path = Path(temporary_directory) / "github-output"
            result = self.run_cli(
                stream=b"docs/TESTING.md",
                event_name="pull_request",
                output_path=output_path,
            )

            self.assertEqual(0, result.returncode)
            self.assertEqual(
                "compiler=true\npages=true\nagent=true\n", output_path.read_text()
            )

    def test_invalid_output_destination_exits_nonzero_without_echoing_paths(self):
        with tempfile.TemporaryDirectory() as temporary_directory:
            result = self.run_cli(
                stream=b"site/private-name.html\x00",
                event_name="pull_request",
                output_path=Path(temporary_directory),
            )

            self.assertNotEqual(0, result.returncode)
            self.assertNotIn(b"site/private-name.html", result.stdout + result.stderr)


if __name__ == "__main__":
    unittest.main()
