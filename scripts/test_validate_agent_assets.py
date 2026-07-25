#!/usr/bin/env python3
"""Regression tests for repository agent-asset validation."""

from __future__ import annotations

import codecs
import hashlib
import json
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path

import validate_agent_assets as validator


FEATURE_DEV = "feature" + "-dev"
CLAUDE_PLUGIN_MARKETPLACE = "claude-plugins" + "-official"
TDD_WORKFLOWS = "tdd" + "-workflows"
CLAUDE_WORKFLOW_MARKETPLACE = "claude-code" + "-workflows"
MUTABLE_HELPER = "mutable" + "-helper"
CODE_REVIEW = "code" + "-review"
PR_REVIEW_TOOLKIT = "pr-review" + "-toolkit"


class AgentAssetValidationTests(unittest.TestCase):
    def fence_result(self, contents: str) -> str | None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "fixture.md"
            path.write_text(contents, encoding="utf-8")
            return validator.fence_error(path)

    def test_balanced_fence_is_accepted(self) -> None:
        self.assertIsNone(self.fence_result("```markdown\n# Example\n```\n"))

    def test_unclosed_fence_is_rejected(self) -> None:
        self.assertIn(
            "unclosed",
            self.fence_result("```markdown\n# Example\n") or "",
        )

    def test_nested_fence_is_rejected(self) -> None:
        self.assertIn(
            "nested",
            self.fence_result("```markdown\n```json\n{}\n```\n") or "",
        )

    def test_absolute_repository_output_is_detected(self) -> None:
        line = "The ADR must be saved in the `/docs/adr/` directory."
        match = validator.ABSOLUTE_OUTPUT.search(line)
        self.assertIsNotNone(match)
        self.assertEqual(match.group(1), "/docs/adr/")

    def test_slash_skill_dependency_is_detected(self) -> None:
        self.assertEqual(
            validator.SLASH_SKILL.findall(
                "Run a `/grilling` session using the `/domain-modeling` skill."
            ),
            ["grilling", "domain-modeling"],
        )

    def test_relative_link_target_drops_fragment(self) -> None:
        self.assertEqual(
            validator.link_target("../skill/SKILL.md#workflow"),
            "../skill/SKILL.md",
        )

    def test_absolute_link_target_stays_absolute_for_rejection(self) -> None:
        self.assertEqual(validator.link_target("/spec/"), "/spec/")

    def test_immutable_pin_requires_a_full_commit_sha(self) -> None:
        self.assertIsNotNone(
            validator.IMMUTABLE_SHA.fullmatch(
                "7d5f3e12d0556cb6c5df2974e2babe0433674186"
            )
        )
        self.assertIsNone(validator.IMMUTABLE_SHA.fullmatch("v0.58.1"))

    def test_skill_folder_hash_matches_skills_cli_ordering(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "agents").mkdir()
            (root / "agents" / "openai.yaml").write_bytes(b"agent")
            (root / "SKILL.md").write_bytes(b"skill")

            expected = hashlib.sha256()
            expected.update(b"agents/openai.yaml")
            expected.update(b"agent")
            expected.update(b"SKILL.md")
            expected.update(b"skill")
            self.assertEqual(
                validator.compute_skill_folder_hash(root),
                expected.hexdigest(),
            )

    def skill_lock_failures(
        self,
        *,
        entry_overrides: dict[str, object] | None = None,
        canonical_present: bool = True,
    ) -> list[str]:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "docs").mkdir()
            (root / "docs" / "AGENT_TOOLING.md").write_text(
                (validator.ROOT / "docs" / "AGENT_TOOLING.md").read_text(
                    encoding="utf-8"
                ),
                encoding="utf-8",
            )
            lock = json.loads(
                (validator.ROOT / "skills-lock.json").read_text(encoding="utf-8")
            )
            if entry_overrides:
                lock["skills"]["i-have-an-issue"].update(entry_overrides)
            (root / "skills-lock.json").write_text(
                json.dumps(lock),
                encoding="utf-8",
            )
            skills_root = validator.SKILLS_ROOT
            if not canonical_present:
                skills_root = root / "empty-skills"
                skills_root.mkdir()
            failures: list[str] = []
            validator.validate_skill_lock(
                failures,
                root=root,
                skills_root=skills_root,
            )
            return failures

    def test_skill_lock_binds_reviewed_provenance_and_hash(self) -> None:
        self.assertEqual(self.skill_lock_failures(), [])

    def test_skill_lock_rejects_mismatched_content_hash(self) -> None:
        failures = self.skill_lock_failures(
            entry_overrides={"computedHash": "0" * 64}
        )
        self.assertTrue(any(".computedHash must be" in item for item in failures))

    def test_skill_lock_rejects_changed_upstream_source(self) -> None:
        failures = self.skill_lock_failures(
            entry_overrides={"source": "attacker/skills"}
        )
        self.assertTrue(any(".source must be" in item for item in failures))

    def test_skill_lock_requires_canonical_skill(self) -> None:
        failures = self.skill_lock_failures(canonical_present=False)
        self.assertTrue(any("has no canonical" in item for item in failures))

    def test_alpha_promotion_requires_both_authenticated_client_evals(
        self,
    ) -> None:
        failures: list[str] = []
        validator.validate_alpha_promotion_gate(
            {"pycc": {"source": "future"}},
            failures,
        )
        self.assertEqual(len(failures), 1)
        self.assertIn(
            "authenticated Codex and Claude model-eval evidence",
            failures[0],
        )

    def test_alpha_promotion_accepts_complete_authenticated_evidence(
        self,
    ) -> None:
        original = validator.AUTHENTICATED_MODEL_EVAL_EVIDENCE
        try:
            validator.AUTHENTICATED_MODEL_EVAL_EVIDENCE = {
                "pycc": {
                    "codex": "https://example.test/codex-eval",
                    "claude": "https://example.test/claude-eval",
                }
            }
            failures: list[str] = []
            validator.validate_alpha_promotion_gate(
                {"pycc": {"source": "future"}},
                failures,
            )
            self.assertEqual(failures, [])
        finally:
            validator.AUTHENTICATED_MODEL_EVAL_EVIDENCE = original

    def alpha_contract_failures(
        self,
        *,
        remove_feedback_text: str | None = None,
        pycc_eval_count: int | None = None,
        remove_pycc_runner: bool = False,
    ) -> list[str]:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for name in ("pycc", "pycc-feedback"):
                shutil.copytree(validator.SKILLS_ROOT / name, root / name)
            if remove_feedback_text is not None:
                feedback_path = root / "pycc-feedback" / "SKILL.md"
                feedback_path.write_text(
                    feedback_path.read_text(encoding="utf-8").replace(
                        remove_feedback_text,
                        "",
                    ),
                    encoding="utf-8",
                )
            if pycc_eval_count is not None:
                evals_path = root / "pycc" / "evals" / "evals.json"
                evals = json.loads(evals_path.read_text(encoding="utf-8"))
                evals["evals"] = evals["evals"][:pycc_eval_count]
                evals_path.write_text(json.dumps(evals), encoding="utf-8")
            if remove_pycc_runner:
                evals_path = root / "pycc" / "evals" / "evals.json"
                evals = json.loads(evals_path.read_text(encoding="utf-8"))
                evals["evals"][0].pop("runner")
                evals_path.write_text(json.dumps(evals), encoding="utf-8")
            failures: list[str] = []
            validator.validate_alpha_skill_contracts(
                root,
                failures,
                root=root,
            )
            return failures

    def test_feedback_skill_requires_outbound_query_sanitization(self) -> None:
        failures = self.alpha_contract_failures(
            remove_feedback_text="sanitize every outbound query"
        )
        self.assertTrue(
            any("sanitize every outbound query" in item for item in failures)
        )

    def test_alpha_skill_requires_multiple_evals(self) -> None:
        failures = self.alpha_contract_failures(pycc_eval_count=1)
        self.assertTrue(any("at least two evals" in item for item in failures))

    def test_alpha_skill_requires_executable_eval_runners(self) -> None:
        failures = self.alpha_contract_failures(remove_pycc_runner=True)
        self.assertTrue(
            any("complete executable runner set" in item for item in failures)
        )

    def write_skill(
        self,
        root: Path,
        name: str,
        description: str,
        body: str,
        *,
        extra_frontmatter: str = "",
    ) -> Path:
        skill_root = root / name
        skill_root.mkdir(parents=True)
        path = skill_root / "SKILL.md"
        path.write_text(
            f"---\nname: {name}\ndescription: {description}\n"
            f"{extra_frontmatter}---\n\n{body}\n",
            encoding="utf-8",
        )
        return path

    def parity_failures(
        self,
        wrapper_body: str,
        *,
        wrapper_name: str = "example",
        wrapper_description: str = "Example workflow.",
    ) -> list[str]:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            canonical_root = root / "canonical"
            codex_root = root / "codex"
            self.write_skill(
                canonical_root,
                "example",
                "Example workflow.",
                "# Canonical",
            )
            self.write_skill(
                codex_root,
                wrapper_name,
                wrapper_description,
                wrapper_body,
            )
            failures: list[str] = []
            validator.validate_skill_parity(
                canonical_root,
                codex_root,
                failures,
            )
            return failures

    def test_codex_wrapper_loads_the_complete_canonical_skill(self) -> None:
        self.assertEqual(
            self.parity_failures(
                "Read `.claude/skills/example/SKILL.md` completely as the "
                "canonical workflow."
            ),
            [],
        )

    def test_codex_wrapper_with_wrong_canonical_target_is_rejected(self) -> None:
        failures = self.parity_failures(
            "Read `.claude/skills/other/SKILL.md` completely as the canonical workflow."
        )
        self.assertTrue(
            any("must reference exactly" in failure for failure in failures)
        )

    def test_codex_and_claude_skill_descriptions_must_match(self) -> None:
        failures = self.parity_failures(
            "Read `.claude/skills/example/SKILL.md` completely as the "
            "canonical workflow.",
            wrapper_description="Different triggers.",
        )
        self.assertTrue(
            any("description must match" in failure for failure in failures)
        )

    def test_codex_wrapper_set_must_match_canonical_skill_set(self) -> None:
        failures = self.parity_failures(
            "Read `.claude/skills/other/SKILL.md` completely as the "
            "canonical workflow.",
            wrapper_name="other",
        )
        self.assertTrue(any("missing canonical" in failure for failure in failures))
        self.assertTrue(any("without canonical" in failure for failure in failures))

    def test_explicit_only_canonical_skill_requires_a_codex_guard(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            canonical_root = root / "canonical"
            codex_root = root / "codex"
            self.write_skill(
                canonical_root,
                "example",
                "Explicit invocation only; never select this skill implicitly. "
                "Example workflow.",
                "# Canonical",
                extra_frontmatter="disable-model-invocation: true\n",
            )
            self.write_skill(
                codex_root,
                "example",
                "Explicit invocation only; never select this skill implicitly. "
                "Example workflow.",
                "Read `.claude/skills/example/SKILL.md` completely as the "
                "canonical workflow.",
            )
            failures: list[str] = []
            validator.validate_skill_parity(
                canonical_root,
                codex_root,
                failures,
            )
            self.assertTrue(
                any("explicit-only invocation gate" in failure for failure in failures)
            )

    def test_explicit_only_codex_guard_is_accepted(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            canonical_root = root / "canonical"
            codex_root = root / "codex"
            self.write_skill(
                canonical_root,
                "example",
                "Explicit invocation only; never select this skill implicitly. "
                "Example workflow.",
                "# Canonical",
                extra_frontmatter="disable-model-invocation: true\n",
            )
            self.write_skill(
                codex_root,
                "example",
                "Explicit invocation only; never select this skill implicitly. "
                "Example workflow.",
                "The canonical workflow is explicit-only. Continue only when "
                "the user names `$example`. If selected implicitly, stop without "
                "writing files. Read `.claude/skills/example/SKILL.md` completely "
                "as the canonical workflow.",
            )
            failures: list[str] = []
            validator.validate_skill_parity(
                canonical_root,
                codex_root,
                failures,
            )
            self.assertEqual(failures, [])

    def claude_settings(
        self,
        *,
        sha: str = "7d5f3e12d0556cb6c5df2974e2babe0433674186",
    ) -> dict:
        return {
            "enabledPlugins": {
                "ievo@ievo-skills": True,
            },
            "extraKnownMarketplaces": {
                "ievo-skills": {
                    "source": {
                        "source": "settings",
                        "name": "ievo-skills",
                        "plugins": [
                            {
                                "name": "ievo",
                                "source": {
                                    "source": "git-subdir",
                                    "url": validator.IEVO_REPOSITORY_URL,
                                    "path": validator.IEVO_PLUGIN_PATH,
                                    "sha": sha,
                                },
                            }
                        ],
                    },
                    "autoUpdate": False,
                }
            }
        }

    def validate_claude_settings(
        self,
        settings: dict,
        codex_ref: str = "7d5f3e12d0556cb6c5df2974e2babe0433674186",
    ) -> list[str]:
        failures: list[str] = []
        validator.validate_claude_ievo_marketplace(
            settings,
            codex_ref,
            failures,
        )
        return failures

    def optional_boundary_failures(
        self,
        settings: dict,
        root: Path,
        repository_files: list[tuple[Path, str]] | None = None,
    ) -> list[str]:
        if repository_files is None:
            repository_files = [
                (path, "100644")
                for path in root.rglob("*")
                if path.is_file()
            ]
        failures: list[str] = []
        validator.validate_optional_plugin_boundary(
            settings,
            failures,
            root,
            repository_files,
        )
        return failures

    def test_inline_claude_marketplace_with_exact_plugin_sha_is_accepted(
        self,
    ) -> None:
        self.assertEqual(self.validate_claude_settings(self.claude_settings()), [])

    def test_inline_claude_marketplace_rejects_every_sibling_entry(self) -> None:
        settings = self.claude_settings()
        plugins = settings["extraKnownMarketplaces"]["ievo-skills"]["source"][
            "plugins"
        ]
        plugins.append(
            {
                "name": "sibling",
                "source": {
                    "source": "git",
                    "url": "https://example.com/sibling.git",
                },
            }
        )

        failures = self.validate_claude_settings(settings)

        self.assertTrue(
            any(
                "must contain only the pinned ievo plugin" in failure
                for failure in failures
            )
        )

    def test_inline_claude_marketplace_rejects_duplicate_ievo_entries(
        self,
    ) -> None:
        settings = self.claude_settings()
        plugins = settings["extraKnownMarketplaces"]["ievo-skills"]["source"][
            "plugins"
        ]
        plugins.append(json.loads(json.dumps(plugins[0])))

        failures = self.validate_claude_settings(settings)

        self.assertTrue(
            any(
                "must contain only the pinned ievo plugin" in failure
                for failure in failures
            )
        )

    def test_inline_claude_marketplace_rejects_a_non_ievo_only_entry(
        self,
    ) -> None:
        invalid_entries = (
            "not-an-ievo-plugin",
            {
                "name": "sibling",
                "source": {
                    "source": "git",
                    "url": "https://example.com/sibling.git",
                },
            },
        )
        for invalid_entry in invalid_entries:
            with self.subTest(entry=invalid_entry):
                settings = self.claude_settings()
                settings["extraKnownMarketplaces"]["ievo-skills"]["source"][
                    "plugins"
                ] = [invalid_entry]

                failures = self.validate_claude_settings(settings)

                self.assertTrue(
                    any(
                        "plugin must be the pinned ievo plugin" in failure
                        for failure in failures
                    )
                )

    def test_claude_marketplace_ref_is_rejected(self) -> None:
        settings = self.claude_settings()
        marketplace = settings["extraKnownMarketplaces"]["ievo-skills"]
        marketplace["source"] = {
            "source": "github",
            "repo": "ievo-ai/skills",
            "ref": "7d5f3e12d0556cb6c5df2974e2babe0433674186",
        }
        failures = self.validate_claude_settings(settings)
        self.assertTrue(
            any("inline settings marketplace" in failure for failure in failures)
        )

    def test_claude_plugin_pin_must_be_an_exact_sha(self) -> None:
        failures = self.validate_claude_settings(
            self.claude_settings(sha="v0.58.1"),
        )
        self.assertTrue(
            any("full immutable commit SHA" in failure for failure in failures)
        )

    def test_claude_and_codex_pins_must_match(self) -> None:
        failures = self.validate_claude_settings(
            self.claude_settings(),
            codex_ref="aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        )
        self.assertTrue(any("must match" in failure for failure in failures))

    def test_claude_ievo_plugin_must_be_enabled(self) -> None:
        settings = self.claude_settings()
        settings["enabledPlugins"]["ievo@ievo-skills"] = False
        failures = self.validate_claude_settings(settings)
        self.assertTrue(any("must enable ievo@ievo-skills" in failure for failure in failures))

    def test_required_asset_cannot_reference_an_optional_claude_plugin(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            skill = root / ".claude" / "skills" / "example" / "SKILL.md"
            skill.parent.mkdir(parents=True)
            skill.write_text(
                f"Run `/{FEATURE_DEV}` before implementing the task.\n",
                encoding="utf-8",
            )
            settings = {
                "enabledPlugins": {
                    f"{FEATURE_DEV}@{CLAUDE_PLUGIN_MARKETPLACE}": True,
                    "ievo@ievo-skills": True,
                }
            }

            failures = self.optional_boundary_failures(
                settings,
                root,
            )

            self.assertEqual(len(failures), 1)
            self.assertIn(
                f"{FEATURE_DEV}@{CLAUDE_PLUGIN_MARKETPLACE}",
                failures[0],
            )

    def test_required_workflow_cannot_reference_an_optional_marketplace(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            workflow = root / ".github" / "workflows" / "required.yml"
            workflow.parent.mkdir(parents=True)
            workflow.write_text(
                f"# Requires {CLAUDE_WORKFLOW_MARKETPLACE} at runtime.\n",
                encoding="utf-8",
            )
            settings = {
                "enabledPlugins": {
                    f"{TDD_WORKFLOWS}@{CLAUDE_WORKFLOW_MARKETPLACE}": True,
                }
            }

            failures = self.optional_boundary_failures(
                settings,
                root,
            )

            self.assertEqual(len(failures), 1)
            self.assertIn(
                f"marketplace {CLAUDE_WORKFLOW_MARKETPLACE}",
                failures[0],
            )

    def test_declared_marketplace_is_optional_before_a_plugin_is_enabled(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            agents = root / "AGENTS.md"
            marketplace = "new" + "-market"
            agents.write_text(
                f"Use `helper@{marketplace}` for every task.\n",
                encoding="utf-8",
            )
            settings = {
                "enabledPlugins": {
                    "ievo@ievo-skills": True,
                },
                "extraKnownMarketplaces": {
                    "ievo-skills": {},
                    marketplace: {},
                },
            }

            failures = self.optional_boundary_failures(
                settings,
                root,
            )

            self.assertEqual(len(failures), 1)
            self.assertIn(f"marketplace {marketplace}", failures[0])

    def test_declared_marketplace_repository_source_is_optional(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            agents = root / "AGENTS.md"
            marketplace = "new" + "-market"
            source_repo = "wshobson" + "/agents"
            agents.write_text(
                f"Use workflows from `{source_repo}` for every task.\n",
                encoding="utf-8",
            )
            settings = self.claude_settings()
            settings["extraKnownMarketplaces"][marketplace] = {
                "source": {
                    "source": "github",
                    "repo": source_repo,
                }
            }

            failures = self.optional_boundary_failures(
                settings,
                root,
            )

            self.assertEqual(len(failures), 1)
            self.assertIn(f"marketplace source {source_repo}", failures[0])
            self.assertIn(f"({marketplace})", failures[0])

    def test_declared_marketplace_url_source_coordinates_are_optional(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            agents = root / "AGENTS.md"
            marketplace = "url" + "-market"
            source_repo = "example" + "/agent-tools"
            source_url = "https://github.com/" + source_repo + ".git"
            normalized_source = "github.com/" + source_repo
            agents.write_text(
                f"Use workflows from `{normalized_source}` for every task.\n",
                encoding="utf-8",
            )
            settings = self.claude_settings()
            settings["extraKnownMarketplaces"][marketplace] = {
                "source": {
                    "source": "url",
                    "url": source_url,
                }
            }

            failures = self.optional_boundary_failures(
                settings,
                root,
            )

            self.assertEqual(len(failures), 1)
            self.assertIn(f"marketplace source {normalized_source}", failures[0])
            self.assertIn(f"({marketplace})", failures[0])

    def test_marketplace_source_coordinates_are_canonicalized(self) -> None:
        cases = (
            (
                "repo",
                "example/agent-tools" + ".git/",
                "example/agent-tools",
            ),
            (
                "url",
                "https://github.com/example/agent%2Dtools.git",
                "example/agent-tools",
            ),
            (
                "url",
                "git@github.com:example/agent-tools.git",
                "example/agent-tools",
            ),
        )
        for source_key, source_value, required_reference in cases:
            with self.subTest(source=source_value):
                with tempfile.TemporaryDirectory() as directory:
                    root = Path(directory)
                    agents = root / "AGENTS.md"
                    marketplace = "normalized" + "-market"
                    agents.write_text(
                        f"Use `{required_reference}` for every task.\n",
                        encoding="utf-8",
                    )
                    settings = self.claude_settings()
                    settings["extraKnownMarketplaces"][marketplace] = {
                        "source": {
                            "source": source_key,
                            source_key: source_value,
                        }
                    }

                    failures = self.optional_boundary_failures(
                        settings,
                        root,
                    )

                    self.assertEqual(len(failures), 1)
                    self.assertIn(
                        f"marketplace source {required_reference}",
                        failures[0],
                    )

    def test_single_segment_url_path_does_not_create_a_generic_token(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            agents = root / "AGENTS.md"
            agents.write_text(
                "All agents must preserve user changes.\n",
                encoding="utf-8",
            )
            marketplace = "single" + "-segment"
            settings = self.claude_settings()
            settings["extraKnownMarketplaces"][marketplace] = {
                "source": {
                    "source": "url",
                    "url": "https://example.com/agents",
                }
            }

            failures = self.optional_boundary_failures(
                settings,
                root,
            )

            self.assertEqual(failures, [])

    def test_url_scheme_and_host_are_normalized_but_path_stays_case_sensitive(
        self,
    ) -> None:
        source_url = "https://example.com/agents"
        cases = (
            ("HTTPS://EXAMPLE.COM/agents", True),
            ("https://example.com/Agents", False),
        )
        for required_reference, rejected in cases:
            with self.subTest(reference=required_reference):
                with tempfile.TemporaryDirectory() as directory:
                    root = Path(directory)
                    agents = root / "AGENTS.md"
                    agents.write_text(
                        f"Use `{required_reference}` for every task.\n",
                        encoding="utf-8",
                    )
                    marketplace = "case" + "-market"
                    settings = self.claude_settings()
                    settings["extraKnownMarketplaces"][marketplace] = {
                        "source": {
                            "source": "url",
                            "url": source_url,
                        }
                    }

                    failures = self.optional_boundary_failures(
                        settings,
                        root,
                    )

                    if rejected:
                        self.assertEqual(len(failures), 1)
                        self.assertIn(
                            "marketplace source https://example.com/agents",
                            failures[0],
                        )
                    else:
                        self.assertEqual(failures, [])

    def test_url_identity_normalization_handles_authority_edges(self) -> None:
        cases = (
            (
                "HTTPS://EXAMPLE.COM:8443/Path",
                "https://example.com:8443/Path",
            ),
            (
                "HTTPS://[2001:DB8::1]:8443/Path",
                "https://[2001:db8::1]:8443/Path",
            ),
            (
                "HTTPS://user@EXAMPLE.COM/Path",
                "https://example.com/Path",
            ),
            (
                "https://[invalid/Path",
                "https://[invalid/Path",
            ),
            (
                "https://EXAMPLE.COM:notaport/Path",
                "https://EXAMPLE.COM:notaport/Path",
            ),
            (
                "https://:443/Path",
                "https://:443/Path",
            ),
            (
                "All agents preserve path Case.",
                "All agents preserve path Case.",
            ),
        )
        for source, expected in cases:
            with self.subTest(source=source):
                self.assertEqual(
                    validator.normalize_url_identity_components(source),
                    expected,
                )

    def test_scp_and_host_path_hosts_are_normalized_but_paths_are_not(
        self,
    ) -> None:
        source_url = "git@EXAMPLE.COM:agents.git"
        cases = (
            ("git@example.com:agents.git", True),
            ("EXAMPLE.COM/agents", True),
            ("git@example.com:Agents.git", False),
        )
        for required_reference, rejected in cases:
            with self.subTest(reference=required_reference):
                with tempfile.TemporaryDirectory() as directory:
                    root = Path(directory)
                    agents = root / "AGENTS.md"
                    agents.write_text(
                        f"Use `{required_reference}` for every task.\n",
                        encoding="utf-8",
                    )
                    marketplace = "scp-case" + "-market"
                    settings = self.claude_settings()
                    settings["extraKnownMarketplaces"][marketplace] = {
                        "source": {
                            "source": "url",
                            "url": source_url,
                        }
                    }

                    failures = self.optional_boundary_failures(
                        settings,
                        root,
                    )

                    if rejected:
                        self.assertEqual(len(failures), 1)
                        self.assertIn(
                            "marketplace source example.com/agents",
                            failures[0],
                        )
                    else:
                        self.assertEqual(failures, [])

    def test_percent_encoded_paths_compose_with_host_normalization(self) -> None:
        cases = (
            (
                "HTTPS://EXAMPLE.COM/agent%2Dtools.git",
                "https://example.com/agent%2Dtools.git",
            ),
            (
                "git@EXAMPLE.COM:agent%2Dtools.git",
                "git@example.com:agent%2Dtools.git",
            ),
        )
        for source_url, required_reference in cases:
            with self.subTest(source=source_url):
                with tempfile.TemporaryDirectory() as directory:
                    root = Path(directory)
                    agents = root / "AGENTS.md"
                    agents.write_text(
                        f"Use `{required_reference}` for every task.\n",
                        encoding="utf-8",
                    )
                    marketplace = "encoded-case" + "-market"
                    settings = self.claude_settings()
                    settings["extraKnownMarketplaces"][marketplace] = {
                        "source": {
                            "source": "url",
                            "url": source_url,
                        }
                    }

                    failures = self.optional_boundary_failures(
                        settings,
                        root,
                    )

                    self.assertEqual(len(failures), 1)
                    self.assertIn(
                        "marketplace source",
                        failures[0],
                    )
                    self.assertIn(f"({marketplace})", failures[0])

    def test_single_label_scp_marketplace_sources_are_rejected(self) -> None:
        for host in ("github", "GITHUB"):
            with self.subTest(host=host):
                with tempfile.TemporaryDirectory() as directory:
                    root = Path(directory)
                    agents = root / "AGENTS.md"
                    agents.write_text(
                        "Use repository-owned tools.\n",
                        encoding="utf-8",
                    )
                    marketplace = "single-label" + "-market"
                    settings = self.claude_settings()
                    settings["extraKnownMarketplaces"][marketplace] = {
                        "source": {
                            "source": "url",
                            "url": f"git@{host}:agents.git",
                        }
                    }

                    failures = self.optional_boundary_failures(
                        settings,
                        root,
                    )

                    self.assertEqual(len(failures), 1)
                    self.assertIn(marketplace, failures[0])
                    self.assertIn(
                        "fully qualified dotted SCP host",
                        failures[0],
                    )
                    self.assertNotIn(host, failures[0])

    def test_malformed_marketplace_url_reports_a_controlled_failure(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            agents = root / "AGENTS.md"
            agents.write_text("Use repository-owned tools.\n", encoding="utf-8")
            marketplace = "malformed" + "-market"
            settings = self.claude_settings()
            settings["extraKnownMarketplaces"][marketplace] = {
                "source": {
                    "source": "url",
                    "url": "https://[invalid/repo.git",
                }
            }

            failures = self.optional_boundary_failures(
                settings,
                root,
            )

            self.assertEqual(len(failures), 1)
            self.assertIn(marketplace, failures[0])
            self.assertIn("source.url", failures[0])
            self.assertNotIn("[invalid", failures[0])

    def test_pinned_marketplace_source_is_not_treated_as_optional(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            agents = root / "AGENTS.md"
            source_repo = "example" + "/pinned-tools"
            agents.write_text(
                f"Use the reviewed `{source_repo}` baseline.\n",
                encoding="utf-8",
            )
            settings = {
                "enabledPlugins": {
                    "ievo@ievo-skills": True,
                },
                "extraKnownMarketplaces": {
                    "ievo-skills": {
                        "source": {
                            "source": "github",
                            "repo": source_repo,
                        }
                    }
                },
            }

            failures = self.optional_boundary_failures(
                settings,
                root,
            )

            self.assertEqual(failures, [])

    def test_unvalidated_sibling_in_pinned_marketplace_is_optional(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            agents = root / "AGENTS.md"
            agents.write_text(
                f"Use `{MUTABLE_HELPER}@ievo-skills` for every change.\n",
                encoding="utf-8",
            )
            settings = {
                "enabledPlugins": {
                    "ievo@ievo-skills": True,
                    f"{MUTABLE_HELPER}@ievo-skills": True,
                }
            }

            failures = self.optional_boundary_failures(
                settings,
                root,
            )

            self.assertEqual(len(failures), 1)
            self.assertIn(f"{MUTABLE_HELPER}@ievo-skills", failures[0])

    def test_unconfigured_sibling_in_pinned_marketplace_is_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            agents = root / "AGENTS.md"
            agents.write_text(
                f"Use `{MUTABLE_HELPER}@ievo-skills` for every change.\n",
                encoding="utf-8",
            )

            failures = self.optional_boundary_failures(
                {
                    "enabledPlugins": {
                        "ievo@ievo-skills": True,
                    }
                },
                root,
            )

            self.assertEqual(len(failures), 1)
            self.assertIn(f"{MUTABLE_HELPER}@ievo-skills", failures[0])

    def test_scoped_instruction_files_reject_optional_plugin_references(self) -> None:
        for filename in ("AGENTS.md", "CLAUDE.md"):
            with self.subTest(filename=filename):
                with tempfile.TemporaryDirectory() as directory:
                    root = Path(directory)
                    instructions = root / "nested" / "subdir" / filename
                    instructions.parent.mkdir(parents=True)
                    instructions.write_text(
                        f"Use `/{FEATURE_DEV}` for every task.\n",
                        encoding="utf-8",
                    )
                    settings = {
                        "enabledPlugins": {
                            f"{FEATURE_DEV}@{CLAUDE_PLUGIN_MARKETPLACE}": True,
                            "ievo@ievo-skills": True,
                        }
                    }

                    failures = self.optional_boundary_failures(
                        settings,
                        root,
                    )

                    self.assertEqual(len(failures), 1)
                    self.assertIn(f"nested/subdir/{filename}", failures[0])

    def test_all_tracked_agent_asset_formats_are_scanned(self) -> None:
        settings = {
            "enabledPlugins": {
                f"{FEATURE_DEV}@{CLAUDE_PLUGIN_MARKETPLACE}": True,
                "ievo@ievo-skills": True,
            }
        }
        fixtures = (
            (".claude/skills/demo/scripts/run.sh", "100644"),
            (".claude/skills/demo/agents/openai.yaml", "100644"),
            (".ievo/evolution/project.md", "100644"),
            (".github/actions/demo/action.yml", "100644"),
            ("ci/check/action.yml", "100644"),
            ("ci/check/action.yaml", "100644"),
            ("tests/agent_smoke.py", "100644"),
            ("scripts/check_ci_permissions.rb", "100644"),
            ("crates/demo/src/lib.rs", "100644"),
            ("tools/agent-adapter", "100755"),
        )

        for relative, mode in fixtures:
            with self.subTest(relative=relative):
                with tempfile.TemporaryDirectory() as directory:
                    root = Path(directory)
                    asset = root / relative
                    asset.parent.mkdir(parents=True)
                    asset.write_text(
                        f"Run /{FEATURE_DEV} before continuing.\n",
                        encoding="utf-8",
                    )

                    failures = self.optional_boundary_failures(
                        settings,
                        root,
                        [(asset, mode)],
                    )

                    self.assertEqual(len(failures), 1)
                    self.assertIn(relative, failures[0])

    def test_claude_behavioral_settings_reject_optional_plugin_references(
        self,
    ) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            settings_path = root / ".claude" / "settings.json"
            settings_path.parent.mkdir(parents=True)
            settings = self.claude_settings()
            settings["enabledPlugins"][
                f"{FEATURE_DEV}@{CLAUDE_PLUGIN_MARKETPLACE}"
            ] = True
            settings["hooks"] = {
                "PreToolUse": [
                    {
                        "hooks": [
                            {
                                "type": "command",
                                "command": f"/{FEATURE_DEV}",
                            }
                        ]
                    }
                ]
            }
            settings_path.write_text(
                json.dumps(settings),
                encoding="utf-8",
            )

            failures = self.optional_boundary_failures(
                settings,
                root,
            )

            self.assertEqual(len(failures), 1)
            self.assertIn(".claude/settings.json", failures[0])
            self.assertIn(
                f"{FEATURE_DEV}@{CLAUDE_PLUGIN_MARKETPLACE}",
                failures[0],
            )

    def test_claude_marketplace_declarations_are_not_required_consumers(
        self,
    ) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            settings_path = root / ".claude" / "settings.json"
            settings_path.parent.mkdir(parents=True)
            settings = self.claude_settings()
            settings["enabledPlugins"][
                f"{FEATURE_DEV}@{CLAUDE_PLUGIN_MARKETPLACE}"
            ] = True
            settings["extraKnownMarketplaces"][
                CLAUDE_PLUGIN_MARKETPLACE
            ] = {
                "source": {
                    "source": "github",
                    "repo": "example" + "/optional-tools",
                }
            }
            settings_path.write_text(
                json.dumps(settings),
                encoding="utf-8",
            )

            failures = self.optional_boundary_failures(
                settings,
                root,
            )

            self.assertEqual(failures, [])

    def test_invalid_claude_settings_shapes_are_scanned_as_raw_text(self) -> None:
        settings = {
            "enabledPlugins": {
                f"{FEATURE_DEV}@{CLAUDE_PLUGIN_MARKETPLACE}": True,
                "ievo@ievo-skills": True,
            }
        }
        cases = (
            f'{{"hooks": ["/{FEATURE_DEV}"]',
            json.dumps([f"/{FEATURE_DEV}"]),
        )
        for contents in cases:
            with self.subTest(contents=contents):
                with tempfile.TemporaryDirectory() as directory:
                    root = Path(directory)
                    settings_path = root / ".claude" / "settings.json"
                    settings_path.parent.mkdir(parents=True)
                    settings_path.write_text(contents, encoding="utf-8")

                    self.assertEqual(
                        validator.required_asset_body(
                            Path(".claude/settings.json"),
                            contents,
                        ),
                        contents,
                    )
                    failures = self.optional_boundary_failures(
                        settings,
                        root,
                    )

                    self.assertEqual(len(failures), 1)
                    self.assertIn(".claude/settings.json", failures[0])
                    self.assertIn(
                        f"{FEATURE_DEV}@{CLAUDE_PLUGIN_MARKETPLACE}",
                        failures[0],
                    )

    def test_ignored_instruction_file_is_not_a_repository_asset(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            tracked = root / "AGENTS.md"
            tracked.write_text("Use repository-owned tools.\n", encoding="utf-8")
            ignored = root / "cache" / "AGENTS.md"
            ignored.parent.mkdir()
            ignored.write_text(
                f"Use /{FEATURE_DEV}.\n",
                encoding="utf-8",
            )
            (root / ".gitignore").write_text("cache/\n", encoding="utf-8")
            subprocess.run(
                ["git", "init", "--quiet", str(root)],
                check=True,
            )
            subprocess.run(
                ["git", "-C", str(root), "add", ".gitignore", "AGENTS.md"],
                check=True,
            )

            repository_files = validator.tracked_repository_files(root)
            failures = self.optional_boundary_failures(
                {
                    "enabledPlugins": {
                        f"{FEATURE_DEV}@{CLAUDE_PLUGIN_MARKETPLACE}": True,
                        "ievo@ievo-skills": True,
                    }
                },
                root,
                repository_files,
            )

            self.assertEqual(failures, [])
            self.assertNotIn(
                ignored,
                [path for path, _ in repository_files],
            )

    def test_ievo_overlay_provenance_is_not_a_required_dependency(self) -> None:
        for newline in ("\n", "\r\n"):
            with self.subTest(newline=repr(newline)):
                with tempfile.TemporaryDirectory() as directory:
                    root = Path(directory)
                    overlay = root / ".ievo" / "evolution" / "skills" / "demo.md"
                    overlay.parent.mkdir(parents=True)
                    provenance = newline.join(
                        (
                            "---",
                            "source:",
                            f"  path: plugins/{FEATURE_DEV}/skills/demo",
                            "---",
                            "",
                        )
                    )
                    overlay.write_text(
                        provenance + "# Local vendored overlay" + newline,
                        encoding="utf-8",
                    )
                    settings = {
                        "enabledPlugins": {
                            f"{FEATURE_DEV}@{CLAUDE_PLUGIN_MARKETPLACE}": True,
                            "ievo@ievo-skills": True,
                        }
                    }

                    failures = self.optional_boundary_failures(
                        settings,
                        root,
                        [(overlay, "100644")],
                    )

                    self.assertEqual(failures, [])
                    overlay.write_text(
                        provenance
                        + f"Run /{FEATURE_DEV} before continuing."
                        + newline,
                        encoding="utf-8",
                    )

                    failures = self.optional_boundary_failures(
                        settings,
                        root,
                        [(overlay, "100644")],
                    )

                    self.assertEqual(len(failures), 1)
                    self.assertIn(
                        overlay.relative_to(root).as_posix(),
                        failures[0],
                    )

    def test_dated_ievo_vendoring_heading_is_provenance_only(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            overlay = root / ".ievo" / "evolution" / "skills" / "demo.md"
            overlay.parent.mkdir(parents=True)
            source_repo = "example" + "/agent-tools"
            heading = f"## 2026-07-24 — Vendored from {source_repo}\n"
            provenance = (
                "---\n"
                "source:\n"
                f"  repo: {source_repo}\n"
                "  commit_sha: aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\n"
                "---\n\n"
            )
            overlay.write_text(
                provenance + "# demo\n\n" + heading + "Initial copy.\n",
                encoding="utf-8",
            )
            marketplace = "overlay" + "-source"
            settings = self.claude_settings()
            settings["extraKnownMarketplaces"][marketplace] = {
                "source": {
                    "source": "github",
                    "repo": source_repo,
                }
            }

            failures = self.optional_boundary_failures(
                settings,
                root,
                [(overlay, "100644")],
            )

            self.assertEqual(failures, [])

            overlay.write_text(
                provenance
                + "# demo\n\n"
                + heading
                + f"Use {source_repo} at runtime.\n",
                encoding="utf-8",
            )
            failures = self.optional_boundary_failures(
                settings,
                root,
                [(overlay, "100644")],
            )

            self.assertEqual(len(failures), 1)
            self.assertIn(f"marketplace source {source_repo}", failures[0])

    def test_nested_ievo_repo_metadata_does_not_authorize_provenance(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            overlay = root / ".ievo" / "evolution" / "skills" / "demo.md"
            overlay.parent.mkdir(parents=True)
            source_repo = "example" + "/agent-tools"
            overlay.write_text(
                "---\n"
                "source:\n"
                "  metadata:\n"
                f"    repo: {source_repo}\n"
                "---\n\n"
                "# demo\n\n"
                f"## 2026-07-24 — Vendored from {source_repo}\n"
                "Initial copy.\n",
                encoding="utf-8",
            )
            marketplace = "nested" + "-source"
            settings = self.claude_settings()
            settings["extraKnownMarketplaces"][marketplace] = {
                "source": {
                    "source": "github",
                    "repo": source_repo,
                }
            }

            failures = self.optional_boundary_failures(
                settings,
                root,
                [(overlay, "100644")],
            )

            self.assertEqual(len(failures), 1)
            self.assertIn(f"marketplace source {source_repo}", failures[0])

    def test_required_agent_asset_symlink_is_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            subprocess.run(
                ["git", "init", "--quiet", str(root)],
                check=True,
            )
            object_id = subprocess.run(
                ["git", "-C", str(root), "hash-object", "-w", "--stdin"],
                check=True,
                input=b"target.md",
                stdout=subprocess.PIPE,
            ).stdout.decode("ascii").strip()
            subprocess.run(
                [
                    "git",
                    "-C",
                    str(root),
                    "update-index",
                    "--add",
                    "--cacheinfo",
                    f"120000,{object_id},AGENTS.md",
                ],
                check=True,
            )
            failures: list[str] = []
            validator.validate_optional_plugin_boundary(
                {
                    "enabledPlugins": {
                        f"{FEATURE_DEV}@{CLAUDE_PLUGIN_MARKETPLACE}": True,
                        "ievo@ievo-skills": True,
                    }
                },
                failures,
                root,
            )

            self.assertEqual(len(failures), 1)
            self.assertIn("must not be symlinks", failures[0])

    def test_unreadable_required_asset_reports_a_controlled_failure(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            missing = root / "AGENTS.md"

            failures = self.optional_boundary_failures(
                {
                    "enabledPlugins": {
                        f"{FEATURE_DEV}@{CLAUDE_PLUGIN_MARKETPLACE}": True,
                        "ievo@ievo-skills": True,
                    }
                },
                root,
                [(missing, "100644")],
            )

            self.assertEqual(len(failures), 1)
            self.assertIn("unable to read tracked required agent asset", failures[0])

    def test_utf16_powershell_asset_is_scanned(self) -> None:
        source = f"Run /{FEATURE_DEV} before continuing.\n"
        payloads = (
            codecs.BOM_UTF16_LE + source.encode("utf-16-le"),
            codecs.BOM_UTF16_BE + source.encode("utf-16-be"),
        )
        for payload in payloads:
            with self.subTest(bom=payload[:2]):
                with tempfile.TemporaryDirectory() as directory:
                    root = Path(directory)
                    script = root / "scripts" / "agent-check.ps1"
                    script.parent.mkdir()
                    script.write_bytes(payload)

                    failures = self.optional_boundary_failures(
                        {
                            "enabledPlugins": {
                                f"{FEATURE_DEV}@{CLAUDE_PLUGIN_MARKETPLACE}": True,
                                "ievo@ievo-skills": True,
                            }
                        },
                        root,
                        [(script, "100644")],
                    )

                    self.assertEqual(len(failures), 1)
                    self.assertIn(script.relative_to(root).as_posix(), failures[0])

    def test_unknown_required_asset_encoding_fails_closed(self) -> None:
        source = f"Run /{FEATURE_DEV} before continuing.\n"
        payloads = (
            b"\xffinvalid",
            source.encode("utf-16-le"),
            source.encode("utf-16-be"),
            codecs.BOM_UTF32_LE + source.encode("utf-32-le"),
            codecs.BOM_UTF32_BE + source.encode("utf-32-be"),
        )
        for payload in payloads:
            with self.subTest(prefix=payload[:4]):
                with tempfile.TemporaryDirectory() as directory:
                    root = Path(directory)
                    script = root / "scripts" / "agent-check.ps1"
                    script.parent.mkdir()
                    script.write_bytes(payload)

                    failures = self.optional_boundary_failures(
                        {
                            "enabledPlugins": {
                                f"{FEATURE_DEV}@{CLAUDE_PLUGIN_MARKETPLACE}": True,
                                "ievo@ievo-skills": True,
                            }
                        },
                        root,
                        [(script, "100644")],
                    )

                    self.assertEqual(len(failures), 1)
                    self.assertIn("must be UTF-8 or BOM-tagged UTF-16", failures[0])

    def test_exact_identity_reports_the_matching_shared_marketplace_plugin(
        self,
    ) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            agents = root / "AGENTS.md"
            agents.write_text(
                f"Use `{CODE_REVIEW}@{CLAUDE_PLUGIN_MARKETPLACE}`.\n",
                encoding="utf-8",
            )
            settings = {
                "enabledPlugins": {
                    f"{FEATURE_DEV}@{CLAUDE_PLUGIN_MARKETPLACE}": True,
                    f"{CODE_REVIEW}@{CLAUDE_PLUGIN_MARKETPLACE}": True,
                    f"{PR_REVIEW_TOOLKIT}@{CLAUDE_PLUGIN_MARKETPLACE}": True,
                }
            }

            failures = self.optional_boundary_failures(
                settings,
                root,
            )

            self.assertEqual(len(failures), 1)
            self.assertIn(
                f"{CODE_REVIEW}@{CLAUDE_PLUGIN_MARKETPLACE}",
                failures[0],
            )
            self.assertNotIn(PR_REVIEW_TOOLKIT, failures[0])

    def test_pinned_baseline_plugin_is_not_treated_as_optional(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            agents = root / "AGENTS.md"
            agents.write_text("Use `ievo@ievo-skills`.\n", encoding="utf-8")
            settings = {
                "enabledPlugins": {
                    "ievo@ievo-skills": True,
                }
            }

            failures = self.optional_boundary_failures(
                settings,
                root,
            )

            self.assertEqual(failures, [])

    def test_pinned_identity_stays_exempt_with_optional_marketplace_sibling(
        self,
    ) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            agents = root / "AGENTS.md"
            agents.write_text("Use `ievo@ievo-skills`.\n", encoding="utf-8")
            settings = {
                "enabledPlugins": {
                    "ievo@ievo-skills": True,
                    f"{MUTABLE_HELPER}@ievo-skills": True,
                }
            }

            failures = self.optional_boundary_failures(
                settings,
                root,
            )

            self.assertEqual(failures, [])

    def test_disabled_or_missing_pinned_identity_is_not_exempt(self) -> None:
        for enabled_plugins in (
            {"ievo@ievo-skills": False},
            {},
        ):
            with self.subTest(enabled_plugins=enabled_plugins):
                with tempfile.TemporaryDirectory() as directory:
                    root = Path(directory)
                    agents = root / "AGENTS.md"
                    agents.write_text(
                        "Use `ievo@ievo-skills`.\n",
                        encoding="utf-8",
                    )

                    failures = self.optional_boundary_failures(
                        {"enabledPlugins": enabled_plugins},
                        root,
                    )

                    self.assertEqual(len(failures), 1)
                    self.assertIn("ievo@ievo-skills", failures[0])


if __name__ == "__main__":
    unittest.main()
