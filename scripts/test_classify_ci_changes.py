#!/usr/bin/env python3
"""Behavior tests for fail-closed CI change routing.

Each literal expectation below catches a classifier branch that could otherwise
skip a CI gate affected by a repository change.
"""

from __future__ import annotations

import fcntl
import os
import subprocess
import sys
import tempfile
import time
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
            Selection(True, True, True),
            classify_paths(
                ["crates/pycc_hir/src/lib.rs", "site/index.html"],
                event_name="pull_request",
            ),
        )

    def test_compiler_inputs_also_select_agent_evals_for_the_fresh_binary(self):
        for path in (
            "src/main.rs",
            "crates/pycc_hir/src/lib.rs",
            "Cargo.toml",
            "Cargo.lock",
            "rust-toolchain",
            "rust-toolchain.toml",
            ".cargo/config.toml",
            "build.rs",
            "docs/DIAGNOSTICS.md",
            "docs/PYTHON_STANDARDS.md",
            "tests/conformance.rs",
            "benches/check_bench.rs",
        ):
            with self.subTest(path=path):
                self.assertEqual(
                    Selection(True, False, True),
                    classify_paths([path], event_name="pull_request"),
                )

    def test_every_input_the_conformance_matrix_guard_reads_selects_compiler(self):
        """`tests/conformance_matrix_guard.rs` reads three inputs, and a change to
        any one of them can break it. Two live under `tests/` and were already
        covered by `COMPILER_ROOTS`; the third, `docs/PYTHON_STANDARDS.md`, sat in
        the `docs/` bucket and classified as EMPTY_SELECTION until issue #591 --
        so a docs-only matrix edit skipped the guard on its own pull request and
        turned `main` red after merge instead. These assertions fail if that path
        is dropped back into the cheap-docs bucket.
        """
        for path in (
            "docs/PYTHON_STANDARDS.md",
            "tests/conformance.rs",
            "tests/conformance_matrix_guard.rs",
        ):
            with self.subTest(path=path):
                selection = classify_paths([path], event_name="pull_request")
                self.assertTrue(
                    selection.compiler,
                    f"{path} is a conformance_matrix_guard input, so it must select"
                    " the compiler jobs that run the guard",
                )

    def test_a_docs_only_matrix_edit_still_selects_the_compiler_jobs(self):
        # The real shape of the pull requests that motivated #591: the matrix
        # plus the roadmap paragraph citing it, and nothing else. `docs/ROADMAP.md`
        # on its own is EMPTY_SELECTION, and the union must not dilute the matrix
        # path's own selection.
        self.assertEqual(
            Selection(False, False, False),
            classify_paths(["docs/ROADMAP.md"], event_name="pull_request"),
        )
        self.assertTrue(
            classify_paths(
                ["docs/PYTHON_STANDARDS.md", "docs/ROADMAP.md"],
                event_name="pull_request",
            ).compiler
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
            Selection(True, False, True),
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

    def test_cross_category_rename_keeps_compiler_selected_when_git_disables_rename_detection(self):
        with tempfile.TemporaryDirectory() as temporary_directory:
            repository = Path(temporary_directory)
            subprocess.run(["git", "init", "-q", str(repository)], check=True)
            subprocess.run(
                ["git", "-C", str(repository), "config", "user.email", "test@example.test"],
                check=True,
            )
            subprocess.run(
                ["git", "-C", str(repository), "config", "user.name", "Classifier Test"],
                check=True,
            )
            (repository / "src").mkdir()
            (repository / "src" / "foo.rs").write_text("fn main() {}\n")
            subprocess.run(["git", "-C", str(repository), "add", "."], check=True)
            subprocess.run(
                ["git", "-C", str(repository), "commit", "-qm", "initial"], check=True
            )
            base = subprocess.run(
                ["git", "-C", str(repository), "rev-parse", "HEAD"],
                check=True,
                capture_output=True,
                text=True,
            ).stdout.strip()
            (repository / "docs").mkdir()
            subprocess.run(
                [
                    "git",
                    "-C",
                    str(repository),
                    "mv",
                    "src/foo.rs",
                    "docs/foo.md",
                ],
                check=True,
            )
            subprocess.run(
                ["git", "-C", str(repository), "commit", "-qm", "rename"], check=True
            )
            head = subprocess.run(
                ["git", "-C", str(repository), "rev-parse", "HEAD"],
                check=True,
                capture_output=True,
                text=True,
            ).stdout.strip()

            changed = subprocess.run(
                [
                    "git",
                    "-C",
                    str(repository),
                    "diff",
                    "--no-renames",
                    "--name-only",
                    "--diff-filter=ACDMRTUXB",
                    "-z",
                    base,
                    head,
                ],
                check=True,
                capture_output=True,
            ).stdout.rstrip(b"\x00").split(b"\x00")

            self.assertEqual(
                Selection(True, False, True),
                classify_paths(
                    [path.decode("utf-8") for path in changed], event_name="pull_request"
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

    def test_nul_delimited_compiler_change_selects_fresh_binary_agent_evals(self):
        with tempfile.TemporaryDirectory() as temporary_directory:
            output_path = Path(temporary_directory) / "github-output"
            result = self.run_cli(
                stream=b"src/main.rs\x00",
                event_name="pull_request",
                output_path=output_path,
            )

            self.assertEqual(0, result.returncode)
            self.assertEqual(
                "compiler=true\npages=false\nagent=true\n",
                output_path.read_text(),
            )
            self.assertNotIn(b"src/main.rs", result.stdout + result.stderr)

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

    def test_output_append_waits_for_an_exclusive_lock_and_writes_one_intact_block(self):
        with tempfile.TemporaryDirectory() as temporary_directory:
            output_path = Path(temporary_directory) / "github-output"
            descriptor = os.open(output_path, os.O_WRONLY | os.O_CREAT, 0o600)
            process = None
            try:
                os.write(descriptor, b"prior=true\n")
                fcntl.flock(descriptor, fcntl.LOCK_EX)
                process = subprocess.Popen(
                    [
                        sys.executable,
                        "-B",
                        str(self.SCRIPT),
                        "--event-name",
                        "pull_request",
                        "--github-output",
                        str(output_path),
                    ],
                    stdin=subprocess.PIPE,
                    stdout=subprocess.PIPE,
                    stderr=subprocess.PIPE,
                )
                assert process.stdin is not None
                process.stdin.write(b"site/index.html\x00")
                process.stdin.close()

                deadline = time.monotonic() + 2
                while time.monotonic() < deadline and process.poll() is None:
                    time.sleep(0.02)
                self.assertIsNone(process.poll(), "CLI did not wait for the output lock")

                fcntl.flock(descriptor, fcntl.LOCK_UN)
                process.wait(timeout=5)
                assert process.stdout is not None
                assert process.stderr is not None
                stdout = process.stdout.read()
                stderr = process.stderr.read()
                process.stdout.close()
                process.stderr.close()
                self.assertEqual(0, process.returncode, stderr.decode("utf-8"))
                self.assertEqual(b"", stdout)
                self.assertEqual(
                    "prior=true\ncompiler=false\npages=true\nagent=false\n",
                    output_path.read_text(),
                )
            finally:
                fcntl.flock(descriptor, fcntl.LOCK_UN)
                os.close(descriptor)
                if process is not None and process.poll() is None:
                    process.kill()
                    try:
                        process.wait(timeout=5)
                    except subprocess.TimeoutExpired as error:
                        raise AssertionError(
                            "CLI did not terminate within five seconds after cleanup kill"
                        ) from error

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
