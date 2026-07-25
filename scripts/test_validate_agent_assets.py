#!/usr/bin/env python3
"""Regression tests for repository agent-asset validation."""

from __future__ import annotations

import hashlib
import json
import shutil
import tempfile
import unittest
from pathlib import Path

import validate_agent_assets as validator


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

    def test_inline_claude_marketplace_with_exact_plugin_sha_is_accepted(
        self,
    ) -> None:
        self.assertEqual(self.validate_claude_settings(self.claude_settings()), [])

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


if __name__ == "__main__":
    unittest.main()
