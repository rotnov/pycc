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
            [
                "shared hook target must remain machine-local: "
                ".ievo/hooks/scripts/capture.sh"
            ],
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

    def test_tracked_ievo_hook_target_is_still_rejected(self) -> None:
        target = ".ievo/hooks/scripts/capture.sh"
        settings = {
            "hooks": {
                "UserPromptSubmit": [
                    {
                        "hooks": [
                            {
                                "command": "sh",
                                "args": [target],
                            }
                        ]
                    }
                ]
            }
        }
        self.assertEqual(
            validator.validate_hook_targets(settings, {target}),
            [f"shared hook target must remain machine-local: {target}"],
        )

    def test_force_added_ievo_hook_file_is_rejected(self) -> None:
        target = ".ievo/hooks/scripts/capture.sh"
        self.assertEqual(
            validator.validate_machine_local_files({target}),
            [f"machine-local iEvo hook must not be tracked: {target}"],
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
            [
                "shared hook target must remain machine-local: "
                ".ievo/hooks/scripts/capture.sh"
            ],
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

    def test_absolute_interpreter_still_validates_relative_script(self) -> None:
        settings = {
            "hooks": {
                "SessionStart": [
                    {
                        "hooks": [
                            {
                                "command": "/bin/sh",
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

    def test_absolute_machine_local_script_is_rejected(self) -> None:
        settings = {
            "hooks": {
                "SessionStart": [
                    {
                        "hooks": [
                            {
                                "command": (
                                    "/home/alice/project/.ievo/hooks/capture.sh"
                                ),
                                "args": [],
                            }
                        ]
                    }
                ]
            }
        }
        self.assertEqual(
            validator.validate_hook_targets(settings, set()),
            [
                "shared hook target must not be absolute: "
                "/home/alice/project/.ievo/hooks/capture.sh"
            ],
        )

    def test_absolute_script_argument_is_rejected_but_interpreter_is_allowed(
        self,
    ) -> None:
        settings = {
            "hooks": {
                "SessionStart": [
                    {
                        "hooks": [
                            {
                                "command": "/bin/sh",
                                "args": ["/home/alice/project/tools/hook.sh"],
                            }
                        ]
                    }
                ]
            }
        }
        self.assertEqual(
            validator.validate_hook_targets(settings, set()),
            [
                "shared hook target must not be absolute: "
                "/home/alice/project/tools/hook.sh"
            ],
        )

    def test_env_launcher_still_validates_relative_script(self) -> None:
        settings = {
            "hooks": {
                "SessionStart": [
                    {
                        "hooks": [
                            {
                                "command": "env",
                                "args": ["sh", "tools/local-hook.sh"],
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

    def test_env_launcher_skips_assignments_before_the_command(self) -> None:
        settings = {
            "hooks": {
                "SessionStart": [
                    {
                        "hooks": [
                            {
                                "command": "/usr/bin/env",
                                "args": [
                                    "HOOK_MODE=shared",
                                    "sh",
                                    "scripts/tracked-hook.sh",
                                ],
                            }
                        ]
                    }
                ]
            }
        }
        self.assertEqual(
            validator.validate_hook_targets(settings, {"scripts/tracked-hook.sh"}),
            [],
        )

    def test_unknown_launcher_validates_path_like_operands(self) -> None:
        for command, arguments, expected in [
            ("uv", ["run", "tools/hook.py"], "tools/hook.py"),
            ("ruby", ["hook.rb"], "hook.rb"),
            ("custom-runner", ["--quiet", "./bin/hook"], "bin/hook"),
        ]:
            with self.subTest(command=command):
                settings = {
                    "hooks": {
                        "SessionStart": [
                            {
                                "hooks": [
                                    {
                                        "command": command,
                                        "args": arguments,
                                    }
                                ]
                            }
                        ]
                    }
                }
                self.assertEqual(
                    validator.validate_hook_targets(settings, set()),
                    [f"shared hook target is not tracked: {expected}"],
                )

    def test_unknown_launcher_accepts_tracked_path_like_operands(self) -> None:
        settings = {
            "hooks": {
                "SessionStart": [
                    {
                        "hooks": [
                            {
                                "command": "uv",
                                "args": ["run", "tools/hook.py"],
                            }
                        ]
                    }
                ]
            }
        }
        self.assertEqual(
            validator.validate_hook_targets(settings, {"tools/hook.py"}),
            [],
        )

    def test_opaque_launcher_options_are_rejected_fail_closed(self) -> None:
        settings = {
            "hooks": {
                "SessionStart": [
                    {
                        "hooks": [
                            {
                                "command": "env",
                                "args": ["-S", "sh tools/local-hook.sh"],
                            }
                        ]
                    }
                ]
            }
        }
        self.assertEqual(
            validator.validate_hook_targets(settings, set()),
            ["shared hook command launcher cannot be validated: env"],
        )

    def test_inline_shell_command_is_rejected_fail_closed(self) -> None:
        settings = {
            "hooks": {
                "SessionStart": [
                    {
                        "hooks": [
                            {
                                "command": (
                                    "sh -c 'exec .ievo/hooks/scripts/capture.sh'"
                                ),
                                "args": [],
                            }
                        ]
                    }
                ]
            }
        }
        self.assertEqual(
            validator.validate_hook_targets(settings, set()),
            ["shared hook inline interpreter mode cannot be validated: sh -c"],
        )

    def test_inline_shell_argument_is_rejected_fail_closed(self) -> None:
        settings = {
            "hooks": {
                "SessionStart": [
                    {
                        "hooks": [
                            {
                                "command": "/bin/bash",
                                "args": [
                                    "-lc",
                                    ("exec $CLAUDE_PROJECT_DIR/tools/local-hook.sh"),
                                ],
                            }
                        ]
                    }
                ]
            }
        }
        self.assertEqual(
            validator.validate_hook_targets(settings, set()),
            ["shared hook inline interpreter mode cannot be validated: /bin/bash -lc"],
        )

    def test_inline_python_and_node_commands_are_rejected(self) -> None:
        for command, arguments, expected in [
            (
                "/usr/bin/python3.14",
                ["-c", "exec(open('tools/hook.py').read())"],
                "/usr/bin/python3.14 -c",
            ),
            (
                "python3",
                ["-cprint('inline')"],
                "python3 -cprint('inline')",
            ),
            (
                "node",
                ["--eval=require('./tools/hook.js')"],
                "node --eval=require('./tools/hook.js')",
            ),
        ]:
            with self.subTest(command=command):
                settings = {
                    "hooks": {
                        "SessionStart": [
                            {
                                "hooks": [
                                    {
                                        "command": command,
                                        "args": arguments,
                                    }
                                ]
                            }
                        ]
                    }
                }
                self.assertEqual(
                    validator.validate_hook_targets(settings, set()),
                    [
                        "shared hook inline interpreter mode cannot be "
                        f"validated: {expected}"
                    ],
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

    def test_malformed_shell_command_is_rejected_fail_closed(self) -> None:
        settings = {
            "hooks": {
                "SessionStart": [
                    {
                        "hooks": [
                            {
                                "command": "sh -c 'unterminated",
                                "args": [],
                            }
                        ]
                    }
                ]
            }
        }
        failures = validator.validate_hook_schema(settings)
        self.assertEqual(len(failures), 1)
        self.assertIn("command is not valid shell syntax", failures[0])

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
