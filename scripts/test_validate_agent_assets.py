#!/usr/bin/env python3
"""Regression tests for repository agent-asset validation."""

from __future__ import annotations

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
