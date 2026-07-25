#!/usr/bin/env python3
"""Mutation tests for the Python language-track consistency guard."""

from __future__ import annotations

import unittest

import check_language_tracks as checker


def repository_documents() -> dict[str, str]:
    return {
        path: (checker.ROOT / path).read_text(encoding="utf-8")
        for path in checker.DOCUMENTS
    }


def repository_workflow() -> str:
    return (checker.ROOT / checker.CI_WORKFLOW).read_text(encoding="utf-8")


class LanguageTrackConsistencyTests(unittest.TestCase):
    def test_repository_contract_is_consistent(self) -> None:
        self.assertEqual(
            checker.validate_language_tracks(
                repository_documents(),
                repository_workflow(),
            ),
            [],
        )

    def test_permanent_python_314_ceiling_is_rejected(self) -> None:
        documents = repository_documents()
        documents["docs/SPEC.md"] = documents["docs/SPEC.md"].replace(
            "1. **Standard Python in, native binary out.** pycc never adds its own "
            "dialect or\n   syntax. The v1.0 language level is exactly CPython 3.14; "
            "admitting a later\n   standard Python language level requires its "
            "versioned roadmap gate and a\n   superseding ADR.",
            "1. **Standard Python 3.14 in, native binary out.** No dialect, no new "
            "syntax — ever.",
        )

        failures = checker.validate_language_tracks(
            documents,
            repository_workflow(),
        )
        self.assertTrue(
            any("permanent Python 3.14 ceiling" in failure for failure in failures)
        )
        self.assertTrue(
            any(
                "docs/SPEC.md is missing required language-track claim" in failure
                for failure in failures
            )
        )

    def test_each_normative_document_must_keep_v1_0_scope(self) -> None:
        mutations = {
            "README.md": ("The v1.0 track uses", "The v1 track uses"),
            "docs/SPEC.md": (
                "The v1.0 language level is",
                "The v1 language level is",
            ),
            "docs/ROADMAP.md": ("D-012 fixes v1.0 to", "D-012 fixes v1 to"),
            "docs/PYTHON_STANDARDS.md": (
                "D-012 fixes v1.0 to exactly",
                "D-012 says v1 accepts exactly",
            ),
            "docs/DECISIONS.md": (
                "exactly CPython 3.14 in v1.0",
                "exactly CPython 3.14 in v1",
            ),
            "docs/TESTING.md": (
                "The\n  v1.0 Python 3.14 run covers",
                "The\n  v1 Python 3.14 run covers",
            ),
            "docs/CLI_SPEC.md": (
                "v1.0 language level",
                "language level; only 3.14 in v1",
            ),
            "docs/STDLIB_PLAN.md": (
                "The v1.0 surface remains CPython 3.14",
                "The v1 surface remains CPython 3.14",
            ),
            "docs/TYPE_SYSTEM.md": (
                "the v1.0 Python 3.14 contract",
                "the v1 Python 3.14 contract",
            ),
            "docs/DELIVERY_PLAN.md": (
                "Matches the v1.0 language line",
                "Matches the v1 language line",
            ),
            "site/architecture/index.html": (
                "The v1.0 language contract",
                "The v1 language contract",
            ),
            "site/python-aot-compilers/index.html": (
                "Python 3.14 is the v1.0 design target",
                "Python 3.14 is the v1 design target",
            ),
        }

        for path, (before, after) in mutations.items():
            with self.subTest(path=path):
                documents = repository_documents()
                self.assertIn(before, documents[path])
                documents[path] = documents[path].replace(before, after, 1)
                failures = checker.validate_language_tracks(
                    documents,
                    repository_workflow(),
                )
                self.assertTrue(
                    any(path in failure for failure in failures),
                    failures,
                )

        documents = repository_documents()
        config_line = (
            'python = "3.14"          # v1.0 language level; '
            "later v1.x needs its gate + ADR"
        )
        documents["docs/CLI_SPEC.md"] = documents["docs/CLI_SPEC.md"].replace(
            config_line,
            f'{config_line}\npython = "3.15"',
            1,
        )
        failures = checker.validate_language_tracks(
            documents,
            repository_workflow(),
        )
        self.assertTrue(
            any(
                failure.startswith(
                    "docs/CLI_SPEC.md is missing required language-track claim"
                )
                for failure in failures
            ),
            failures,
        )

        documents = repository_documents()
        documents["docs/CLI_SPEC.md"] = documents["docs/CLI_SPEC.md"].replace(
            config_line,
            'python = "3.15"\n'
            '# python = "3.14" # v1.0 language level; '
            "later v1.x needs its gate + ADR",
            1,
        )
        failures = checker.validate_language_tracks(
            documents,
            repository_workflow(),
        )
        self.assertTrue(
            any(
                failure.startswith(
                    "docs/CLI_SPEC.md is missing required language-track claim"
                )
                for failure in failures
            ),
            failures,
        )

        documents = repository_documents()
        documents["docs/CLI_SPEC.md"] = documents["docs/CLI_SPEC.md"].replace(
            config_line,
            'python = "3.15"',
            1,
        )
        documents["docs/CLI_SPEC.md"] += f"\n{config_line}\n"
        failures = checker.validate_language_tracks(
            documents,
            repository_workflow(),
        )
        self.assertTrue(
            any(
                failure.startswith(
                    "docs/CLI_SPEC.md is missing required language-track claim"
                )
                for failure in failures
            ),
            failures,
        )

    def test_supported_language_runs_must_remain_cumulative(self) -> None:
        mutations = {
            "README.md": (
                "runs its cumulative fixture set",
                "runs only that level's fixture set",
            ),
            "docs/PYTHON_STANDARDS.md": (
                "covers `py30/` through `py315/`",
                "covers only `py315/`",
            ),
            "docs/ROADMAP.md": (
                "the cumulative Python 3.0–3.15 fixture set",
                "only the Python 3.15 fixture set",
            ),
            "docs/TESTING.md": (
                "the Python 3.15 run covers `py30/`\n  through `py315/`",
                "the Python 3.15 run covers only `py315/`",
            ),
        }

        for path, (before, after) in mutations.items():
            with self.subTest(path=path):
                documents = repository_documents()
                self.assertIn(before, documents[path])
                documents[path] = documents[path].replace(before, after, 1)
                failures = checker.validate_language_tracks(
                    documents,
                    repository_workflow(),
                )
                self.assertTrue(
                    any(
                        failure.startswith(
                            f"{path} is missing required language-track claim"
                        )
                        for failure in failures
                    ),
                    failures,
                )

        current_track_mutations = {
            "docs/PYTHON_STANDARDS.md": (
                (
                    "covers\n  `py30/` through `py314/` against CPython 3.14.6",
                    "covers\n  `py31/` through `py314/` against CPython 3.14.6",
                ),
                (
                    "covers\n  `py30/` through `py314/` against CPython 3.14.6",
                    "covers\n  `py30/` through `py313/` against CPython 3.14.6",
                ),
                (
                    "covers\n  `py30/` through `py314/` against CPython 3.14.6",
                    "covers\n  `py30/` through `py314/` against CPython 3.14.5",
                ),
            ),
            "docs/TESTING.md": (
                (
                    "covers `py30/` through `py314/` against CPython 3.14.6",
                    "covers `py31/` through `py314/` against CPython 3.14.6",
                ),
                (
                    "covers `py30/` through `py314/` against CPython 3.14.6",
                    "covers `py30/` through `py313/` against CPython 3.14.6",
                ),
                (
                    "covers `py30/` through `py314/` against CPython 3.14.6",
                    "covers `py30/` through `py314/` against CPython 3.14.5",
                ),
            ),
        }

        for path, mutations_for_path in current_track_mutations.items():
            for before, after in mutations_for_path:
                with self.subTest(path=path, after=after):
                    documents = repository_documents()
                    self.assertIn(before, documents[path])
                    documents[path] = documents[path].replace(before, after, 1)
                    failures = checker.validate_language_tracks(
                        documents,
                        repository_workflow(),
                    )
                    self.assertTrue(
                        any(
                            failure.startswith(
                                f"{path} is missing required language-track claim"
                            )
                            for failure in failures
                        ),
                        failures,
                    )

    def test_preview_and_upgrade_gate_are_scoped_after_v1_0(self) -> None:
        path = "docs/PYTHON_STANDARDS.md"
        for before, after in (
            ("Post-v1.0 preview", "Post-v1 preview"),
            ("the post-v1.0 roadmap gate", "the post-v1 roadmap gate"),
        ):
            with self.subTest(after=after):
                documents = repository_documents()
                self.assertIn(before, documents[path])
                documents[path] = documents[path].replace(before, after, 1)
                failures = checker.validate_language_tracks(
                    documents,
                    repository_workflow(),
                )
                self.assertTrue(
                    any(
                        failure.startswith(
                            f"{path} is missing required language-track claim"
                        )
                        for failure in failures
                    ),
                    failures,
                )
                self.assertTrue(
                    any(
                        "major-version-wide post-v1 gate" in failure
                        for failure in failures
                    ),
                    failures,
                )

    def test_additive_major_version_ceiling_is_rejected(self) -> None:
        path = "README.md"
        contradictions = (
            "The v1 supports only CPython 3.14.",
            "All v1.x releases support only Python 3.14.",
            "Python 3.14 remains the language ceiling throughout v1.",
            "Python 3.15 support is deferred until v2.0.",
            "Every v1 release is limited to CPython 3.14.",
            "The v1 family is permanently pinned to Python 3.14.",
            "v1 will never support Python 3.15.",
            "The entire v1 line is restricted to Python 3.14.",
        )

        for contradiction in contradictions:
            with self.subTest(contradiction=contradiction):
                documents = repository_documents()
                documents[path] += f"\n{contradiction}\n"
                failures = checker.validate_language_tracks(
                    documents,
                    repository_workflow(),
                )
                self.assertTrue(
                    any(
                        "major-version-wide Python 3.14 ceiling" in failure
                        or "Python 3.15 deferred beyond v1.x" in failure
                        or "v1-wide rejection of Python 3.15" in failure
                        for failure in failures
                    ),
                    failures,
                )

    def test_compatibility_language_is_not_a_false_positive(self) -> None:
        documents = repository_documents()
        allowed = (
            "Python 3.14 is not the language ceiling throughout v1.x.",
            "The Python 3.14 oracle remains pinned for the v1.x compatibility run.",
            "The v1 track uses CPython 3.14 only as a compatibility oracle while "
            "Python 3.15 is supported.",
            "The independent Python 3.14 compatibility run is not optional.",
            "The independent Python 3.14 compatibility run will not be removed.",
            "The Python 3.15 configuration does not run only the py315 fixture.",
            "The Python 3.15 gate does not proceed without a superseding ADR.",
            "The v1.0 language level is not Python 3.15.",
            "Python 3.15 support is not deferred until v2.0.",
            "D-012 is not superseded; it remains active.",
        )
        documents["README.md"] += "\n" + "\n".join(allowed) + "\n"
        failures = checker.validate_language_tracks(
            documents,
            repository_workflow(),
        )
        self.assertFalse(
            any("contains forbidden" in failure for failure in failures),
            failures,
        )
        self.assertIn(
            "README.md has unregistered visible language-track policy changes",
            failures,
        )

    def test_visible_additive_contradictions_are_rejected(self) -> None:
        contradictions = (
            ("README.md", "The v1.0 language level is CPython 3.15."),
            (
                "docs/TESTING.md",
                "The Python 3.15 configuration runs only the py315 fixture directory.",
            ),
            (
                "docs/ROADMAP.md",
                "After 3.15 ships, the independent Python 3.14 configuration is "
                "optional.",
            ),
            (
                "docs/PYTHON_STANDARDS.md",
                "The Python 3.15 gate does not need a superseding ADR.",
            ),
            (
                "docs/TESTING.md",
                "The Python 3.14 run executes only py314/ against CPython 3.14.5.",
            ),
            (
                "docs/SPEC.md",
                "A pycc-only syntax extension is permitted in supported programs.",
            ),
            (
                "docs/PYTHON_STANDARDS.md",
                "The Python 3.15 preview is part of the v1.0 language surface.",
            ),
            ("docs/DECISIONS.md", "D-012 is superseded by D-021."),
            (
                "README.md",
                "The only language level supported throughout the v1 series is "
                "Python 3.14.",
            ),
            (
                "docs/SPEC.md",
                "The v1.x Python 3.15 track introduces pycc-only syntax.",
            ),
            (
                "docs/TESTING.md",
                "The Python 3.14 run excludes the py30 fixture directory.",
            ),
            (
                "docs/TESTING.md",
                "For Python 3.15, older fixtures are excluded and only py315 is "
                "executed.",
            ),
            (
                "docs/ROADMAP.md",
                "Once Python 3.15 ships, drop the separate Python 3.14 job.",
            ),
            (
                "docs/PYTHON_STANDARDS.md",
                "Later standard Python levels need no machine-checked "
                "support/cumulative-fixture/oracle mapping.",
            ),
            (
                "README.md",
                "Python 3.15 will never be supported in v1.x.",
            ),
            (
                "site/architecture/index.html",
                "Python 3.15 adoption has been cancelled; every v1.x release "
                "will use Python 3.14.",
            ),
            (
                "site/python-aot-compilers/index.html",
                "Python 3.15 adoption has been cancelled; every v1.x release "
                "will use Python 3.14.",
            ),
        )

        for path, contradiction in contradictions:
            with self.subTest(path=path):
                documents = repository_documents()
                if path in checker.SITE_PROJECTIONS:
                    documents[path] = documents[path].replace(
                        "</main>",
                        f"<p>{contradiction}</p>\n</main>",
                        1,
                    )
                else:
                    documents[path] += f"\n{contradiction}\n"
                failures = checker.validate_language_tracks(
                    documents,
                    repository_workflow(),
                )
                self.assertTrue(
                    any(
                        failure.startswith(f"{path} contains forbidden")
                        or failure
                        == f"{path} has unregistered visible language-track policy changes"
                        for failure in failures
                    ),
                    failures,
                )

    def test_hidden_or_nonnormative_text_cannot_satisfy_claim(self) -> None:
        required = checker.REQUIRED_CLAIMS["README.md"][0]
        wrappers = (
            f"<!-- {required} -->",
            f"<!-- {required}",
            f"> {required}",
            f"```\n{required}\n```",
            f"````text\n{required}\n````",
            f"   ~~~\n{required}\n   ~~~",
            f"   ```\n{required}\n   ```",
            f"> ```\n> {required}\n> ````",
            f"    {required}",
            f"- ```\n  {required}\n  ```",
            f"~~{required}~~",
        )
        for wrapped in wrappers:
            with self.subTest(wrapped=wrapped):
                documents = repository_documents()
                self.assertIn(required, documents["README.md"])
                documents["README.md"] = documents["README.md"].replace(
                    required,
                    "The v1.0 language level is CPython 3.15.",
                    1,
                )
                documents["README.md"] += f"\n{wrapped}\n"
                failures = checker.validate_language_tracks(
                    documents,
                    repository_workflow(),
                )
                self.assertTrue(
                    any(
                        failure.startswith(
                            "README.md is missing required language-track claim"
                        )
                        for failure in failures
                    ),
                    failures,
                )

        site_path = "site/architecture/index.html"
        site_required = checker.REQUIRED_CLAIMS[site_path][0]
        hidden_site_wrappers = (
            f"<script>{site_required}</script>",
            f"<p hidden>{site_required}</p>",
            f'<p aria-hidden="true">{site_required}</p>',
            f'<p style="display: none">{site_required}</p>',
            f'<p style="visibility: hidden">{site_required}</p>',
        )
        for wrapped in hidden_site_wrappers:
            with self.subTest(wrapped=wrapped):
                documents = repository_documents()
                self.assertIn(site_required, documents[site_path])
                documents[site_path] = documents[site_path].replace(
                    site_required,
                    "The v1.0 language contract is Python 3.15.",
                    1,
                )
                documents[site_path] = documents[site_path].replace(
                    "</body>",
                    f"{wrapped}\n</body>",
                    1,
                )
                failures = checker.validate_language_tracks(
                    documents,
                    repository_workflow(),
                )
                self.assertTrue(
                    any(
                        failure.startswith(
                            f"{site_path} is missing required language-track claim"
                        )
                        for failure in failures
                    ),
                    failures,
                )

    def test_policy_digest_requires_explicit_registration(self) -> None:
        self.assertEqual(
            set(checker.EXPECTED_POLICY_DIGESTS),
            set(checker.DOCUMENTS),
        )
        self.assertEqual(
            set(checker.REQUIRED_CLAIMS),
            set(checker.DOCUMENTS),
        )
        self.assertEqual(
            set(checker.POLICY_DOCUMENTS) | set(checker.SITE_PROJECTIONS),
            set(checker.DOCUMENTS),
        )
        self.assertTrue(
            set(checker.SITE_PROJECTIONS).issubset(
                checker.EXPECTED_POLICY_DIGESTS
            ),
        )
        for addition in (
            "Python 3.15 support enters only through the v1.x roadmap gate.",
            "Future upstream grammars are permanently rejected.",
        ):
            with self.subTest(addition=addition):
                documents = repository_documents()
                path = "README.md"
                documents[path] += f"\n{addition}\n"
                failure = (
                    f"{path} has unregistered visible language-track policy changes"
                )
                self.assertIn(
                    failure,
                    checker.validate_language_tracks(
                        documents,
                        repository_workflow(),
                    ),
                )

        for path in checker.SITE_PROJECTIONS:
            with self.subTest(path=path):
                documents = repository_documents()
                addition = (
                    "<p>Python 3.15 adoption has been cancelled; every v1.x "
                    "release will use Python 3.14.</p>"
                )
                documents[path] = documents[path].replace(
                    "</main>",
                    f"{addition}\n</main>",
                    1,
                )
                failure = (
                    f"{path} has unregistered visible language-track policy changes"
                )
                self.assertIn(
                    failure,
                    checker.validate_language_tracks(
                        documents,
                        repository_workflow(),
                    ),
                )

        documents = repository_documents()
        path = "README.md"
        documents[path] += (
            "\nPython 3.15 support enters only through the v1.x roadmap gate.\n"
        )
        failure = f"{path} has unregistered visible language-track policy changes"
        self.assertIn(
            failure,
            checker.validate_language_tracks(
                documents,
                repository_workflow(),
            ),
        )

        original = checker.EXPECTED_POLICY_DIGESTS[path]
        checker.EXPECTED_POLICY_DIGESTS[path] = checker.policy_digest(
            path,
            documents[path],
        )
        try:
            self.assertEqual(
                checker.validate_language_tracks(
                    documents,
                    repository_workflow(),
                ),
                [],
            )
        finally:
            checker.EXPECTED_POLICY_DIGESTS[path] = original

    def test_later_track_keeps_independent_python_314_compatibility(self) -> None:
        mutations = {
            "docs/PYTHON_STANDARDS.md": (
                "the independent 3.14 compatibility run remains\n  required",
                "the 3.14 compatibility run is removed",
            ),
            "docs/ROADMAP.md": (
                "the independent Python 3.14 configuration keeps\n"
                "the complete Python 3.0–3.14 suite green",
                "the Python 3.14 configuration is removed",
            ),
            "docs/TESTING.md": (
                "the separate\n  Python 3.14 compatibility run remains required",
                "the Python 3.14 compatibility run is removed",
            ),
        }

        for path, (before, after) in mutations.items():
            with self.subTest(path=path):
                documents = repository_documents()
                self.assertIn(before, documents[path])
                documents[path] = documents[path].replace(before, after, 1)
                failures = checker.validate_language_tracks(
                    documents,
                    repository_workflow(),
                )
                self.assertTrue(
                    any(
                        failure.startswith(
                            f"{path} is missing required language-track claim"
                        )
                        for failure in failures
                    ),
                    failures,
                )

    def test_upgrade_contract_keeps_superseding_adr(self) -> None:
        mutations = {
            "docs/SPEC.md": (
                "and a\n   superseding ADR",
                "without a superseding ADR",
            ),
            "docs/ROADMAP.md": (
                "a new ADR supersedes D-012",
                "D-012 needs no replacement",
            ),
            "docs/PYTHON_STANDARDS.md": (
                "and a new\nADR that supersedes D-012",
                "without a superseding ADR",
            ),
            "docs/DECISIONS.md": (
                "requires a superseding ADR plus",
                "requires no superseding ADR and",
            ),
        }

        for path, (before, after) in mutations.items():
            with self.subTest(path=path):
                documents = repository_documents()
                self.assertIn(before, documents[path])
                documents[path] = documents[path].replace(before, after, 1)
                failures = checker.validate_language_tracks(
                    documents,
                    repository_workflow(),
                )
                self.assertTrue(
                    any(
                        failure.startswith(
                            f"{path} is missing required language-track claim"
                        )
                        for failure in failures
                    ),
                    failures,
                )

    def test_active_d012_row_is_required(self) -> None:
        documents = repository_documents()
        original_row = next(
            line
            for line in documents["docs/DECISIONS.md"].splitlines()
            if line.startswith("| D-012 |")
        )
        documents["docs/DECISIONS.md"] = documents["docs/DECISIONS.md"].replace(
            "pycc-specific syntax remains forbidden | proposed |",
            "pycc-specific syntax remains forbidden | superseded by D-021 |",
            1,
        )
        documents["docs/DECISIONS.md"] += f"\n```\n{original_row}\n```\n"
        self.assertIn(
            "docs/DECISIONS.md must contain exactly one active proposed/accepted "
            "D-012 language-track row",
            checker.validate_language_tracks(
                documents,
                repository_workflow(),
            ),
        )

    def test_guard_and_self_tests_are_required_in_ci(self) -> None:
        workflow = repository_workflow()
        for command in checker.REQUIRED_CI_COMMANDS:
            with self.subTest(command=command):
                self.assertIn(command, workflow)
                failures = checker.validate_language_tracks(
                    repository_documents(),
                    workflow.replace(command, "", 1),
                )
                self.assertIn(
                    f"CI must run the language-track guard: {command}",
                    failures,
                )

    def test_commented_ci_commands_do_not_count_as_execution(self) -> None:
        workflow = repository_workflow()
        for command in checker.REQUIRED_CI_COMMANDS:
            with self.subTest(command=command):
                executable_line = f"          {command}"
                self.assertIn(executable_line, workflow)
                commented_workflow = workflow.replace(
                    executable_line,
                    f"          # {command}",
                    1,
                )
                self.assertIn(
                    f"CI must run the language-track guard: {command}",
                    checker.validate_language_tracks(
                        repository_documents(),
                        commented_workflow,
                    ),
                )

    def test_language_track_ci_job_and_step_must_be_unconditional(self) -> None:
        workflow = repository_workflow()
        mutations = (
            workflow.replace(
                f"  {checker.CI_JOB_NAME}:",
                f"  {checker.CI_JOB_NAME}:\n    if: ${{{{ false }}}}",
                1,
            ),
            workflow.replace(
                f"      - name: {checker.CI_STEP_NAME}",
                f"      - name: {checker.CI_STEP_NAME}\n        if: ${{{{ false }}}}",
                1,
            ),
            workflow.replace(
                f"      - name: {checker.CI_STEP_NAME}",
                f"      - name: {checker.CI_STEP_NAME}\n"
                "        continue-on-error: true",
                1,
            ),
            workflow.replace(
                f"  {checker.CI_JOB_NAME}:",
                f'  {checker.CI_JOB_NAME}:\n    "if": ${{{{ false }}}}',
                1,
            ),
            workflow.replace(
                f"  {checker.CI_JOB_NAME}:",
                f"  {checker.CI_JOB_NAME}:\n    if : ${{{{ false }}}}",
                1,
            ),
            workflow.replace(
                f"      - name: {checker.CI_STEP_NAME}",
                f"      - name: {checker.CI_STEP_NAME}\n"
                '        "continue-on-error": true',
                1,
            ),
            workflow.replace(
                f"      - name: {checker.CI_STEP_NAME}",
                f'      - name: {checker.CI_STEP_NAME}\n        "shell": python',
                1,
            ),
            workflow.replace(
                f"      - name: {checker.CI_STEP_NAME}",
                f"      - name: {checker.CI_STEP_NAME}\n"
                "        working-directory: bypass",
                1,
            ),
            workflow.replace(
                "    runs-on: ubuntu-latest",
                "    runs-on: ubuntu-latest\n    runs-on: ubuntu-latest",
                1,
            ),
            workflow.replace(
                '          PYTHONPATH: ""\n        run: |',
                '          PYTHONPATH: ""\n        "run": |\n        run: |',
                1,
            ),
        )

        for mutated_workflow in mutations:
            with self.subTest(mutation=mutated_workflow):
                failures = checker.validate_language_tracks(
                    repository_documents(),
                    mutated_workflow,
                )
                for command in checker.REQUIRED_CI_COMMANDS:
                    self.assertIn(
                        f"CI must run the language-track guard: {command}",
                        failures,
                    )

        timeout_workflow = (
            workflow.replace(
                "    runs-on: ubuntu-latest",
                "    runs-on: ubuntu-latest\n    timeout-minutes: 60",
                1,
            )
            .replace(
                f"      - name: {checker.CI_STEP_NAME}",
                f"      - name: {checker.CI_STEP_NAME}\n        timeout-minutes: 5",
                1,
            )
        )
        self.assertEqual(
            checker.validate_language_tracks(
                repository_documents(),
                timeout_workflow,
            ),
            [],
        )

    def test_semantic_yaml_quoting_does_not_break_ci_guard(self) -> None:
        workflow = repository_workflow()
        mutations = (
            workflow.replace(
                f"  {checker.CI_JOB_NAME}:",
                f'  "{checker.CI_JOB_NAME}":',
                1,
            ),
            workflow.replace(
                f"  {checker.CI_JOB_NAME}:",
                f"  {checker.CI_JOB_NAME} :",
                1,
            ),
            workflow.replace(
                f"      - name: {checker.CI_STEP_NAME}",
                f'      - name: "{checker.CI_STEP_NAME}"',
                1,
            ),
            workflow.replace(
                '          PYTHONPATH: ""\n        run: |',
                '          PYTHONPATH: ""\n        "run": |',
                1,
            ),
        )
        for mutated_workflow in mutations:
            with self.subTest(mutation=mutated_workflow):
                self.assertEqual(
                    checker.validate_language_tracks(
                        repository_documents(),
                        mutated_workflow,
                    ),
                    [],
                )

        equivalent_empty_scalars = workflow.replace(
            '          BASH_ENV: ""',
            "          \"BASH_ENV\" : ''",
            1,
        )
        self.assertEqual(
            checker.validate_language_tracks(
                repository_documents(),
                equivalent_empty_scalars,
            ),
            [],
        )

    def test_ci_trigger_and_default_shell_cannot_bypass_guard(self) -> None:
        workflow = repository_workflow()
        mutations = (
            (
                workflow.replace("  pull_request:", "  workflow_dispatch:", 1),
                "CI language-track guard requires an unfiltered pull_request trigger",
            ),
            (
                workflow.replace(
                    "  pull_request:",
                    '  pull_request:\n    paths-ignore: ["**/*.md"]',
                    1,
                ),
                "CI language-track guard requires an unfiltered pull_request trigger",
            ),
            (
                workflow.replace(
                    "permissions:",
                    "defaults:\n  run:\n    shell: true {0}\n\npermissions:",
                    1,
                ),
                "CI language-track guard forbids workflow-level run defaults",
            ),
            (
                workflow.replace(
                    "env:",
                    "env:\n  BASH_ENV: bypass/exit-zero.sh",
                    1,
                ),
                "CI language-track guard forbids inherited execution environment",
            ),
            (
                workflow.replace(
                    "env:",
                    "env:\n  SHELLOPTS: noexec",
                    1,
                ),
                "CI language-track guard forbids inherited execution environment",
            ),
        )

        for mutated_workflow, expected in mutations:
            with self.subTest(expected=expected):
                self.assertIn(
                    expected,
                    checker.validate_language_tracks(
                        repository_documents(),
                        mutated_workflow,
                    ),
                )

    def test_language_track_ci_commands_run_directly_and_in_order(self) -> None:
        workflow = repository_workflow()
        first, second = checker.REQUIRED_CI_COMMANDS
        direct_block = f"          {first}\n          {second}"
        mutations = (
            workflow.replace(
                direct_block,
                f"          {second}\n          {first}",
                1,
            ),
            workflow.replace(
                direct_block,
                f"          if false; then\n"
                f"            {first}\n"
                f"            {second}\n"
                f"          fi",
                1,
            ),
        )

        for mutated_workflow in mutations:
            with self.subTest(mutation=mutated_workflow):
                self.assertIn(
                    "CI language-track step must contain exactly the repository guard "
                    "and self-tests, in that order",
                    checker.validate_language_tracks(
                        repository_documents(),
                        mutated_workflow,
                    ),
                )

    def test_required_self_test_inventory_is_enforced(self) -> None:
        source = (checker.ROOT / checker.SELF_TEST_PATH).read_text(encoding="utf-8")
        renamed = source.replace(
            "def test_permanent_python_314_ceiling_is_rejected",
            "def removed_permanent_python_314_ceiling_mutation",
            1,
        )
        self.assertIn(
            f"{checker.SELF_TEST_PATH} is missing required mutation test: "
            "test_permanent_python_314_ceiling_is_rejected",
            checker.validate_self_test_inventory(renamed),
        )
        hollow_start = source.index(
            "    def test_permanent_python_314_ceiling_is_rejected"
        )
        hollow_end = source.index(
            "\n    def test_each_normative_document_must_keep_v1_0_scope",
            hollow_start,
        )
        hollow = (
            source[:hollow_start]
            + "    def test_permanent_python_314_ceiling_is_rejected"
            "(self) -> None:\n"
            "        pass\n"
            + source[hollow_end:]
        )
        self.assertIn(
            f"{checker.SELF_TEST_PATH} has unregistered mutation-test behavior",
            checker.validate_self_test_inventory(hollow),
        )
        duplicated = source.replace(
            "class LanguageTrackConsistencyTests(unittest.TestCase):",
            "class LanguageTrackConsistencyTests(unittest.TestCase):\n"
            "    def test_permanent_python_314_ceiling_is_rejected"
            "(self) -> None:\n"
            "        pass",
            1,
        )
        self.assertIn(
            f"{checker.SELF_TEST_PATH} must define each required mutation test "
            "exactly once: test_permanent_python_314_ceiling_is_rejected",
            checker.validate_self_test_inventory(duplicated),
        )
        plain_class = source.replace(
            "class LanguageTrackConsistencyTests(unittest.TestCase):",
            "class LanguageTrackConsistencyTests:",
            1,
        )
        self.assertIn(
            f"{checker.SELF_TEST_PATH} must define exactly one discoverable "
            "LanguageTrackConsistencyTests(unittest.TestCase)",
            checker.validate_self_test_inventory(plain_class),
        )
        module_loader_override = source.replace(
            "class LanguageTrackConsistencyTests(unittest.TestCase):",
            "unittest.TestLoader.getTestCaseNames = lambda *_args: []\n\n"
            "class LanguageTrackConsistencyTests(unittest.TestCase):",
            1,
        )
        self.assertIn(
            f"{checker.SELF_TEST_PATH} has unregistered mutation-test behavior",
            checker.validate_self_test_inventory(module_loader_override),
        )
        fabricated_loader_override = source.replace(
            "class LanguageTrackConsistencyTests(unittest.TestCase):",
            "unittest.TestLoader.loadTestsFromTestCase = (\n"
            "    lambda *_args: unittest.TestSuite([\n"
            "        unittest.FunctionTestCase(lambda: None)\n"
            "    ])\n"
            ")\n\n"
            "class LanguageTrackConsistencyTests(unittest.TestCase):",
            1,
        )
        self.assertIn(
            f"{checker.SELF_TEST_PATH} has unregistered mutation-test behavior",
            checker.validate_self_test_inventory(fabricated_loader_override),
        )
        no_runner = source.replace(
            'if __name__ == "__main__":\n    unittest.main()',
            "",
            1,
        )
        self.assertIn(
            f"{checker.SELF_TEST_PATH} must execute unittest.main() as a script",
            checker.validate_self_test_inventory(no_runner),
        )
        skipped = source.replace(
            "class LanguageTrackConsistencyTests(unittest.TestCase):",
            "class LanguageTrackConsistencyTests(unittest.TestCase):\n"
            "    __unittest_skip__ = True",
            1,
        )
        self.assertIn(
            f"{checker.SELF_TEST_PATH} must not skip or decorate required "
            "mutation tests",
            checker.validate_self_test_inventory(skipped),
        )
        annotated_skipped = source.replace(
            "class LanguageTrackConsistencyTests(unittest.TestCase):",
            "class LanguageTrackConsistencyTests(unittest.TestCase):\n"
            "    __unittest_skip__: bool = True",
            1,
        )
        self.assertIn(
            f"{checker.SELF_TEST_PATH} must not skip or decorate required "
            "mutation tests",
            checker.validate_self_test_inventory(annotated_skipped),
        )
        walrus_skipped = source.replace(
            "class LanguageTrackConsistencyTests(unittest.TestCase):",
            "class LanguageTrackConsistencyTests(unittest.TestCase):\n"
            "    (__unittest_skip__ := True)",
            1,
        )
        self.assertIn(
            f"{checker.SELF_TEST_PATH} must not override unittest discovery or "
            "lifecycle",
            checker.validate_self_test_inventory(walrus_skipped),
        )
        locals_skipped = source.replace(
            "class LanguageTrackConsistencyTests(unittest.TestCase):",
            "class LanguageTrackConsistencyTests(unittest.TestCase):\n"
            '    locals()["__unittest_skip__"] = True',
            1,
        )
        self.assertIn(
            f"{checker.SELF_TEST_PATH} must not override unittest discovery or "
            "lifecycle",
            checker.validate_self_test_inventory(locals_skipped),
        )
        asynchronous = source.replace(
            "def test_permanent_python_314_ceiling_is_rejected",
            "async def test_permanent_python_314_ceiling_is_rejected",
            1,
        )
        self.assertIn(
            f"{checker.SELF_TEST_PATH} is missing required mutation test: "
            "test_permanent_python_314_ceiling_is_rejected",
            checker.validate_self_test_inventory(asynchronous),
        )
        skipped_setup = source.replace(
            "class LanguageTrackConsistencyTests(unittest.TestCase):",
            "class LanguageTrackConsistencyTests(unittest.TestCase):\n"
            "    def setUp(self):\n"
            '        raise unittest.SkipTest("disabled")',
            1,
        )
        self.assertIn(
            f"{checker.SELF_TEST_PATH} must not override unittest discovery or "
            "lifecycle",
            checker.validate_self_test_inventory(skipped_setup),
        )
        footer = '\n\nif __name__ == "__main__":\n    unittest.main()\n'
        self.assertTrue(source.endswith(footer))
        empty_loader = (
            source[: -len(footer)] + "\n\ndef load_tests(*_args):\n"
            "    return unittest.TestSuite()\n" + footer
        )
        self.assertIn(
            f"{checker.SELF_TEST_PATH} must not override unittest discovery or "
            "lifecycle",
            checker.validate_self_test_inventory(empty_loader),
        )
        assigned_loader = (
            source[: -len(footer)]
            + "\n\nload_tests = lambda *_args: unittest.TestSuite()\n"
            + footer
        )
        self.assertIn(
            f"{checker.SELF_TEST_PATH} must not override unittest discovery or "
            "lifecycle",
            checker.validate_self_test_inventory(assigned_loader),
        )
        rebound_test = source.replace(
            footer,
            "\n\nLanguageTrackConsistencyTests."
            "test_permanent_python_314_ceiling_is_rejected = "
            'unittest.skip("disabled")('
            "LanguageTrackConsistencyTests."
            "test_permanent_python_314_ceiling_is_rejected)" + footer,
            1,
        )
        self.assertIn(
            f"{checker.SELF_TEST_PATH} must not override unittest discovery or "
            "lifecycle",
            checker.validate_self_test_inventory(rebound_test),
        )
        self.assertEqual(checker.validate_self_test_inventory(source), [])

    def test_missing_document_fails_closed(self) -> None:
        documents = repository_documents()
        missing = checker.DOCUMENTS[-1]
        del documents[missing]
        self.assertIn(
            f"language-track document is unavailable: {missing}",
            checker.validate_language_tracks(
                documents,
                repository_workflow(),
            ),
        )


if __name__ == "__main__":
    unittest.main()
