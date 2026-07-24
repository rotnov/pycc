#!/usr/bin/env python3
"""Regression tests for committed agent-policy validation."""

from __future__ import annotations

import unittest

import validate_agent_policies as validator


class AgentPolicyValidationTests(unittest.TestCase):
    def test_untracked_shared_hook_target_is_rejected(self) -> None:
        settings = {
            "hooks": {
                "UserPromptSubmit": [
                    {
                        "hooks": [
                            {
                                "type": "command",
                                "command": "sh",
                                "args": [".ievo/hooks/scripts/capture.sh"],
                            }
                        ]
                    }
                ]
            }
        }
        self.assertEqual(
            validator.validate_hook_targets(settings, set()),
            ["shared hook target is not tracked: .ievo/hooks/scripts/capture.sh"],
        )

    def test_tracked_shared_wrapper_is_accepted(self) -> None:
        settings = {
            "hooks": {
                "SessionStart": [
                    {
                        "hooks": [
                            {
                                "command": "sh",
                                "args": ["scripts/safe-wrapper.sh"],
                            }
                        ]
                    }
                ]
            }
        }
        self.assertEqual(
            validator.validate_hook_targets(settings, {"scripts/safe-wrapper.sh"}),
            [],
        )

    def test_shell_form_untracked_target_is_rejected(self) -> None:
        settings = {
            "hooks": {
                "SessionStart": [
                    {
                        "hooks": [
                            {
                                "command": ("sh .ievo/hooks/scripts/capture.sh"),
                                "args": [],
                            }
                        ]
                    }
                ]
            }
        }
        self.assertEqual(
            validator.validate_hook_targets(settings, set()),
            ["shared hook target is not tracked: .ievo/hooks/scripts/capture.sh"],
        )

    def test_unlisted_repository_relative_target_is_rejected(self) -> None:
        settings = {
            "hooks": {
                "SessionStart": [
                    {
                        "hooks": [
                            {
                                "command": "sh",
                                "args": ["tools/local-hook.sh"],
                            }
                        ]
                    }
                ]
            }
        }
        self.assertEqual(
            validator.validate_hook_targets(settings, set()),
            ["shared hook target is not tracked: tools/local-hook.sh"],
        )

    def test_project_dir_target_outside_allowlist_is_rejected(self) -> None:
        settings = {
            "hooks": {
                "SessionStart": [
                    {
                        "hooks": [
                            {
                                "command": "sh",
                                "args": ["${CLAUDE_PROJECT_DIR}/bin/local-hook"],
                            }
                        ]
                    }
                ]
            }
        }
        self.assertEqual(
            validator.validate_hook_targets(settings, set()),
            ["shared hook target is not tracked: bin/local-hook"],
        )

    def test_root_script_without_extension_is_rejected(self) -> None:
        settings = {
            "hooks": {
                "SessionStart": [
                    {
                        "hooks": [
                            {
                                "command": "sh",
                                "args": ["local-hook"],
                            }
                        ]
                    }
                ]
            }
        }
        self.assertEqual(
            validator.validate_hook_targets(settings, set()),
            ["shared hook target is not tracked: local-hook"],
        )

    def test_dot_relative_command_is_rejected(self) -> None:
        settings = {
            "hooks": {
                "SessionStart": [
                    {
                        "hooks": [
                            {
                                "command": "./local-hook",
                                "args": [],
                            }
                        ]
                    }
                ]
            }
        }
        self.assertEqual(
            validator.validate_hook_targets(settings, set()),
            ["shared hook target is not tracked: local-hook"],
        )

    def test_string_args_are_rejected(self) -> None:
        settings = {
            "hooks": {
                "SessionStart": [
                    {
                        "hooks": [
                            {
                                "command": "sh",
                                "args": ".ievo/hooks/scripts/capture.sh",
                            }
                        ]
                    }
                ]
            }
        }
        self.assertEqual(
            validator.validate_hook_schema(settings),
            ["hooks.SessionStart[0].hooks[0].args must be a list of strings"],
        )

    def test_flag_parser_preserves_policy_values(self) -> None:
        self.assertEqual(
            validator.parse_flag(
                "enabled: true\nsignal: corrections-only\n"
                "auto_write_scope: project-wide-only\n"
            ),
            {
                "enabled": "true",
                "signal": "corrections-only",
                "auto_write_scope": "project-wide-only",
            },
        )


if __name__ == "__main__":
    unittest.main()
