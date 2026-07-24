#!/usr/bin/env python3
"""Regression tests for repository agent-asset validation."""

from __future__ import annotations

import copy
import json
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
            },
            "enabledPlugins": {"ievo@ievo-skills": True},
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

    def validate_enabled_pins(self, settings: dict) -> list[str]:
        failures: list[str] = []
        validator.validate_enabled_claude_plugin_pins(settings, failures)
        return failures

    def production_claude_settings(self) -> dict:
        return json.loads(
            (validator.ROOT / ".claude/settings.json").read_text(encoding="utf-8")
        )

    def production_plugin_source(
        self,
        settings: dict,
        coordinate: str = "feature-dev@pycc-official-pinned",
    ) -> dict:
        plugin_name, marketplace_name = coordinate.split("@")
        plugins = settings["extraKnownMarketplaces"][marketplace_name]["source"][
            "plugins"
        ]
        return next(
            plugin["source"] for plugin in plugins if plugin["name"] == plugin_name
        )

    def test_every_production_enabled_claude_plugin_is_exactly_pinned(self) -> None:
        settings = self.production_claude_settings()
        self.assertEqual(len(settings["extraKnownMarketplaces"]), 3)
        self.assertEqual(len(settings["enabledPlugins"]), 12)
        self.assertEqual(self.validate_enabled_pins(settings), [])

    def test_enabled_claude_plugin_rejects_a_movable_marketplace(self) -> None:
        settings = self.claude_settings()
        settings["extraKnownMarketplaces"]["ievo-skills"] = {
            "source": {
                "source": "github",
                "repo": "ievo-ai/skills",
            },
            "autoUpdate": False,
        }
        failures = self.validate_enabled_pins(settings)
        self.assertTrue(
            any("inline settings marketplace" in failure for failure in failures)
        )

    def test_enabled_claude_plugin_rejects_automatic_updates(self) -> None:
        settings = self.claude_settings()
        settings["extraKnownMarketplaces"]["ievo-skills"]["autoUpdate"] = True
        failures = self.validate_enabled_pins(settings)
        self.assertTrue(
            any("autoUpdate must be false" in failure for failure in failures)
        )

    def test_enabled_claude_plugin_rejects_a_movable_plugin_source(self) -> None:
        settings = self.claude_settings(sha="v0.58.1")
        failures = self.validate_enabled_pins(settings)
        self.assertTrue(
            any("full immutable commit SHA" in failure for failure in failures)
        )

    def test_enabled_claude_plugin_rejects_a_non_git_subdir_source(self) -> None:
        settings = self.production_claude_settings()
        source = self.production_plugin_source(settings)
        source["source"] = "github"
        failures = self.validate_enabled_pins(settings)
        self.assertTrue(any("must use git-subdir" in failure for failure in failures))

    def test_enabled_claude_plugin_rejects_an_unapproved_url(self) -> None:
        settings = self.production_claude_settings()
        source = self.production_plugin_source(settings)
        source["url"] = "https://github.com/example/other.git"
        failures = self.validate_enabled_pins(settings)
        self.assertTrue(any("URL must be" in failure for failure in failures))

    def test_enabled_claude_plugin_rejects_the_wrong_subdirectory(self) -> None:
        settings = self.production_claude_settings()
        source = self.production_plugin_source(settings)
        source["path"] = "./plugins/not-feature-dev"
        failures = self.validate_enabled_pins(settings)
        self.assertTrue(any("path must be" in failure for failure in failures))

    def test_enabled_claude_plugin_rejects_a_missing_source(self) -> None:
        settings = self.production_claude_settings()
        marketplace = settings["extraKnownMarketplaces"]["pycc-official-pinned"]
        del marketplace["source"]["plugins"][0]["source"]
        failures = self.validate_enabled_pins(settings)
        self.assertTrue(
            any("source must be an object" in failure for failure in failures)
        )

    def test_enabled_claude_plugin_rejects_duplicate_entries(self) -> None:
        settings = self.production_claude_settings()
        marketplace = settings["extraKnownMarketplaces"]["pycc-official-pinned"]
        marketplace["source"]["plugins"].append(
            copy.deepcopy(marketplace["source"]["plugins"][0])
        )
        failures = self.validate_enabled_pins(settings)
        self.assertTrue(
            any(
                "exactly one inline marketplace entry" in failure
                for failure in failures
            )
        )

    def test_enabled_claude_plugin_rejects_a_missing_marketplace(self) -> None:
        settings = self.production_claude_settings()
        del settings["extraKnownMarketplaces"]["pycc-official-pinned"]
        failures = self.validate_enabled_pins(settings)
        self.assertTrue(
            any("has no declared marketplace" in failure for failure in failures)
        )

    def test_enabled_claude_plugin_rejects_an_invalid_coordinate(self) -> None:
        settings = self.production_claude_settings()
        settings["enabledPlugins"]["feature-dev"] = settings["enabledPlugins"].pop(
            "feature-dev@pycc-official-pinned"
        )
        failures = self.validate_enabled_pins(settings)
        self.assertTrue(
            any(
                "must have the form plugin@marketplace" in failure
                for failure in failures
            )
        )

    def test_enabled_claude_plugin_rejects_ref_even_with_a_sha(self) -> None:
        settings = self.production_claude_settings()
        source = self.production_plugin_source(settings)
        source["ref"] = "main"
        failures = self.validate_enabled_pins(settings)
        self.assertTrue(any("must use sha, not ref" in failure for failure in failures))


if __name__ == "__main__":
    unittest.main()
