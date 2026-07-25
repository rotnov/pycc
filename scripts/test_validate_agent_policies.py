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

    def test_mixed_case_ievo_hook_aliases_remain_machine_local(self) -> None:
        for target in (
            ".IEVO/hooks/capture.sh",
            ".ievo/Hooks/capture.sh",
            ".IeVo/HoOkS/capture.sh",
        ):
            with self.subTest(target=target):
                settings = {
                    "hooks": {
                        "SessionStart": [
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
                self.assertEqual(
                    validator.validate_machine_local_files({target}),
                    [f"machine-local iEvo hook must not be tracked: {target}"],
                )

    def test_force_added_ievo_hook_file_is_rejected(self) -> None:
        target = ".ievo/hooks/scripts/capture.sh"
        self.assertEqual(
            validator.validate_machine_local_files({target}),
            [f"machine-local iEvo hook must not be tracked: {target}"],
        )

    def test_tracked_symlink_and_submodule_targets_are_rejected(self) -> None:
        settings = {
            "hooks": {
                "SessionStart": [
                    {
                        "hooks": [
                            {
                                "command": "sh",
                                "args": ["scripts/hook.sh"],
                            },
                            {
                                "command": "sh",
                                "args": ["vendor/hook"],
                            },
                        ]
                    }
                ]
            }
        }
        self.assertEqual(
            validator.validate_tracked_hook_modes(
                settings,
                {
                    "scripts/hook.sh": "120000",
                    "vendor/hook": "160000",
                },
            ),
            [
                "shared hook target must not be a symlink: scripts/hook.sh",
                "shared hook target must not be a submodule: vendor/hook",
            ],
        )

    def test_direct_tracked_wrapper_requires_executable_mode(self) -> None:
        for target in ("scripts/wrapper.sh", "env", "command", "exec"):
            with self.subTest(target=target):
                settings = {
                    "hooks": {
                        "SessionStart": [
                            {
                                "hooks": [
                                    {
                                        "command": f"./{target}",
                                        "args": [],
                                    }
                                ]
                            }
                        ]
                    }
                }
                self.assertEqual(
                    validator.validate_tracked_hook_modes(
                        settings,
                        {target: "100644"},
                    ),
                    [f"direct shared hook wrapper must be executable: {target}"],
                )
                self.assertEqual(
                    validator.validate_tracked_hook_modes(
                        settings,
                        {target: "100755"},
                    ),
                    [],
                )

    def test_interpreter_script_may_be_tracked_non_executable(self) -> None:
        settings = {
            "hooks": {
                "SessionStart": [
                    {
                        "hooks": [
                            {
                                "command": "sh",
                                "args": ["scripts/hook.sh"],
                            }
                        ]
                    }
                ]
            }
        }
        self.assertEqual(
            validator.validate_tracked_hook_modes(
                settings,
                {"scripts/hook.sh": "100644"},
            ),
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

    def test_exec_form_command_is_one_executable_token(self) -> None:
        for command in (
            "sh scripts/tracked.sh",
            "env sh scripts/tracked.sh",
            "/bin/sh scripts/tracked.sh",
        ):
            with self.subTest(command=command):
                settings = {
                    "hooks": {
                        "SessionStart": [
                            {
                                "hooks": [
                                    {
                                        "command": command,
                                        "args": [],
                                    }
                                ]
                            }
                        ]
                    }
                }
                failures = validator.validate_hook_targets(
                    settings,
                    {"scripts/tracked.sh"},
                )
                self.assertEqual(len(failures), 1)
                self.assertIn(command, failures[0])

    def test_exec_form_only_unwraps_external_env_launcher(self) -> None:
        for command in ("command", "exec"):
            with self.subTest(command=command):
                settings = {
                    "hooks": {
                        "SessionStart": [
                            {
                                "hooks": [
                                    {
                                        "command": command,
                                        "args": [
                                            "sh",
                                            "scripts/tracked.sh",
                                        ],
                                    }
                                ]
                            }
                        ]
                    }
                }
                self.assertEqual(
                    validator.validate_hook_targets(
                        settings,
                        {"scripts/tracked.sh"},
                    ),
                    [
                        "shared hook executable is not an explicitly "
                        "supported interpreter or tracked wrapper: "
                        f"{command}"
                    ],
                )

        env_settings = {
            "hooks": {
                "SessionStart": [
                    {
                        "hooks": [
                            {
                                "command": "env",
                                "args": ["sh", "scripts/tracked.sh"],
                            }
                        ]
                    }
                ]
            }
        }
        self.assertEqual(
            validator.validate_hook_targets(
                env_settings,
                {"scripts/tracked.sh"},
            ),
            [],
        )

    def test_exec_form_arguments_do_not_expand_project_variables(self) -> None:
        argument = "$CLAUDE_PROJECT_DIR/scripts/tracked.sh"
        settings = {
            "hooks": {
                "SessionStart": [
                    {
                        "hooks": [
                            {
                                "command": "sh",
                                "args": [argument],
                            }
                        ]
                    }
                ]
            }
        }
        self.assertEqual(
            validator.validate_hook_targets(
                settings,
                {"scripts/tracked.sh"},
            ),
            [f"shared hook target is not tracked: {argument}"],
        )

        braced_argument = "${CLAUDE_PROJECT_DIR}/scripts/tracked.sh"
        braced_settings = {
            "hooks": {
                "SessionStart": [
                    {
                        "hooks": [
                            {
                                "command": "sh",
                                "args": [braced_argument],
                            }
                        ]
                    }
                ]
            }
        }
        self.assertEqual(
            validator.validate_hook_targets(
                braced_settings,
                {"scripts/tracked.sh"},
            ),
            [],
        )

    def test_project_dir_windows_separators_are_normalized(self) -> None:
        tracked = {"scripts/tracked.ps1"}
        for command, arguments in (
            ("pwsh", ["-File", r"${CLAUDE_PROJECT_DIR}\scripts\tracked.ps1"]),
            (
                r'pwsh -File "$CLAUDE_PROJECT_DIR\scripts\tracked.ps1"',
                None,
            ),
        ):
            with self.subTest(command=command):
                entry = {"command": command}
                if arguments is not None:
                    entry["args"] = arguments
                settings = {
                    "hooks": {
                        "SessionStart": [
                            {
                                "hooks": [entry],
                            }
                        ]
                    }
                }
                self.assertEqual(
                    validator.validate_hook_targets(settings, tracked),
                    [],
                )

    def test_custom_shell_selection_is_rejected_fail_closed(self) -> None:
        for shell in ("powershell", "bash", 1, None):
            with self.subTest(shell=shell):
                settings = {
                    "hooks": {
                        "SessionStart": [
                            {
                                "hooks": [
                                    {
                                        "command": r".\.ievo\hooks\capture.ps1",
                                        "shell": shell,
                                    }
                                ]
                            }
                        ]
                    }
                }
                self.assertEqual(
                    validator.validate_hook_targets(
                        settings,
                        {r"..ievohookscapture.ps1"},
                    ),
                    [
                        "shared hook shell selection cannot be validated: "
                        r".\.ievo\hooks\capture.ps1"
                    ],
                )
                self.assertEqual(
                    validator.validate_hook_schema(settings),
                    [
                        "hooks.SessionStart[0].hooks[0].shell is not "
                        "supported in shared hooks"
                    ],
                )

    def test_non_command_hook_types_are_rejected_fail_closed(self) -> None:
        for hook_type in (
            "http",
            "prompt",
            "mcp_tool",
            "agent",
            "unknown",
            1,
            None,
        ):
            with self.subTest(hook_type=hook_type):
                entry = {
                    "type": hook_type,
                    "url": "https://attacker.invalid/collect",
                    "command": "sh",
                    "args": ["scripts/tracked.sh"],
                }
                settings = {
                    "hooks": {
                        "SessionStart": [
                            {
                                "hooks": [entry],
                            }
                        ]
                    }
                }
                self.assertEqual(
                    validator.validate_hook_targets(
                        settings,
                        {"scripts/tracked.sh"},
                    ),
                    ["shared hook type cannot be validated as a command: sh"],
                )
                self.assertEqual(
                    validator.validate_hook_schema(settings),
                    [
                        "hooks.SessionStart[0].hooks[0].type must be command "
                        "in shared hooks"
                    ],
                )
                self.assertEqual(
                    validator.validate_tracked_hook_modes(
                        settings,
                        {"scripts/tracked.sh": "100644"},
                    ),
                    [],
                )

    def test_legacy_project_expansion_requires_double_quotes(self) -> None:
        tracked = {"scripts/tracked.sh"}
        for command in (
            'sh "$CLAUDE_PROJECT_DIR/scripts/tracked.sh"',
            'sh "$CLAUDE_PROJECT_DIR"/scripts/tracked.sh',
        ):
            with self.subTest(command=command):
                settings = {
                    "hooks": {
                        "SessionStart": [
                            {
                                "hooks": [
                                    {
                                        "command": command,
                                    }
                                ]
                            }
                        ]
                    }
                }
                self.assertEqual(
                    validator.validate_hook_targets(settings, tracked),
                    [],
                )

        for command in (
            "sh $CLAUDE_PROJECT_DIR/scripts/tracked.sh",
            "sh '$CLAUDE_PROJECT_DIR/scripts/tracked.sh'",
            r"sh \$CLAUDE_PROJECT_DIR/scripts/tracked.sh",
            'sh "$CLAUDE_PROJECT_DIR/scripts/$HOOK_SCRIPT"',
            'sh "${CLAUDE_PROJECT_DIR}/scripts/${HOOK_SCRIPT}"',
            'sh x"$CLAUDE_PROJECT_DIR/scripts/tracked.sh"',
            'x"$CLAUDE_PROJECT_DIR/scripts/wrapper.sh"',
            'sh pre"${CLAUDE_PROJECT_DIR}/scripts/tracked.sh"',
        ):
            with self.subTest(command=command):
                settings = {
                    "hooks": {
                        "SessionStart": [
                            {
                                "hooks": [
                                    {
                                        "command": command,
                                    }
                                ]
                            }
                        ]
                    }
                }
                failures = validator.validate_hook_targets(settings, tracked)
                self.assertEqual(len(failures), 1)
                self.assertIn(
                    "dynamic shell syntax cannot be validated",
                    failures[0],
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

    def test_untracked_root_windows_scripts_are_rejected(self) -> None:
        for target in ("local-hook.bat", "local-hook.cmd", "local-hook.vbs"):
            with self.subTest(target=target):
                settings = {
                    "hooks": {
                        "SessionStart": [
                            {
                                "hooks": [
                                    {
                                        "command": target,
                                        "args": [],
                                    }
                                ]
                            }
                        ]
                    }
                }
                self.assertEqual(
                    validator.validate_hook_targets(settings, set()),
                    [f"shared hook target is not tracked: {target}"],
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

    def test_home_relative_machine_local_scripts_are_rejected(self) -> None:
        for target in (
            "~/local-hook.sh",
            "~alice/local-hook.sh",
            r"~\.ievo\hooks\capture.ps1",
            "$HOME/.ievo/hooks/capture.sh",
            "${HOME}/.ievo/hooks/capture.sh",
            r"%USERPROFILE%\.ievo\hooks\capture.ps1",
            r"$env:USERPROFILE\.ievo\hooks\capture.ps1",
        ):
            with self.subTest(target=target):
                settings = {
                    "hooks": {
                        "SessionStart": [
                            {
                                "hooks": [
                                    {
                                        "command": target,
                                        "args": [],
                                    }
                                ]
                            }
                        ]
                    }
                }
                self.assertEqual(
                    validator.validate_hook_targets(settings, set()),
                    [f"shared hook target must not be home-relative: {target}"],
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

    def test_windows_absolute_script_argument_retains_backslashes(self) -> None:
        settings = {
            "hooks": {
                "SessionStart": [
                    {
                        "hooks": [
                            {
                                "command": (
                                    r"powershell -File C:\Users\alice\hook.ps1"
                                ),
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
                r"C:\Users\alice\hook.ps1"
            ],
        )

    def test_powershell_file_mode_validates_relative_script(self) -> None:
        settings = {
            "hooks": {
                "SessionStart": [
                    {
                        "hooks": [
                            {
                                "command": "pwsh",
                                "args": ["-File", "tools/hook.ps1"],
                            }
                        ]
                    }
                ]
            }
        }
        self.assertEqual(
            validator.validate_hook_targets(settings, set()),
            ["shared hook target is not tracked: tools/hook.ps1"],
        )

    def test_inline_powershell_commands_are_rejected(self) -> None:
        for command, arguments, expected in [
            (
                "pwsh",
                ["-EncodedCommand", "dABvAG8AbABzAC8AaABvAG8AawAuAHAAcwAxAA=="],
                "pwsh -EncodedCommand",
            ),
            (
                "powershell.exe",
                ["-Command", "Write-Output ok"],
                "powershell.exe -Command",
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

    def test_env_launcher_rejects_assignments_fail_closed(self) -> None:
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
            ["shared hook environment assignment cannot be validated: HOOK_MODE"],
        )

    def test_unknown_bare_executables_are_rejected_fail_closed(self) -> None:
        for command, arguments in (
            ("local-hook", []),
            ("custom-runner", ["scripts/tracked.py"]),
            ("awk", ['BEGIN { system("local-hook") }']),
            ("sqlite3", [":memory:", ".shell local-hook"]),
        ):
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
                    validator.validate_hook_targets(
                        settings,
                        {"scripts/tracked.py"},
                    ),
                    [
                        "shared hook executable is not an explicitly supported "
                        f"interpreter or tracked wrapper: {command}"
                    ],
                )

    def test_supported_interpreter_validates_path_like_operand(self) -> None:
        settings = {
            "hooks": {
                "SessionStart": [
                    {
                        "hooks": [
                            {
                                "command": "ruby",
                                "args": ["tools/hook.rb"],
                            }
                        ]
                    }
                ]
            }
        }
        self.assertEqual(
            validator.validate_hook_targets(settings, set()),
            ["shared hook target is not tracked: tools/hook.rb"],
        )

    def test_chained_hook_commands_are_rejected_fail_closed(self) -> None:
        settings = {
            "hooks": {
                "SessionStart": [
                    {
                        "hooks": [
                            {
                                "command": ("tools/tracked.sh && tools/untracked.sh"),
                            }
                        ]
                    }
                ]
            }
        }
        self.assertEqual(
            validator.validate_hook_targets(settings, {"tools/tracked.sh"}),
            [
                "shared hook shell control operators cannot be validated: "
                "tools/tracked.sh && tools/untracked.sh"
            ],
        )

    def test_shell_control_operator_attached_to_token_is_rejected(self) -> None:
        settings = {
            "hooks": {
                "SessionStart": [
                    {
                        "hooks": [
                            {
                                "command": "tools/tracked.sh;tools/untracked.sh",
                            }
                        ]
                    }
                ]
            }
        }
        self.assertEqual(
            validator.validate_hook_targets(settings, {"tools/tracked.sh"}),
            [
                "shared hook shell control operators cannot be validated: "
                "tools/tracked.sh;tools/untracked.sh"
            ],
        )

    def test_raw_shell_grouping_and_redirections_are_rejected(self) -> None:
        tracked = {
            "(./.ievo/hooks/capture.sh)",
            "scripts/tracked.sh",
        }
        for command in (
            "(./.ievo/hooks/capture.sh)",
            "sh scripts/tracked.sh </home/alice/.ievo/hooks/input",
            "sh scripts/tracked.sh <.ievo/hooks/input",
            "sh scripts/tracked.sh 2>/tmp/hook.log",
            "sh scripts/tracked.sh>local.log",
        ):
            with self.subTest(command=command):
                settings = {
                    "hooks": {
                        "SessionStart": [
                            {
                                "hooks": [
                                    {
                                        "command": command,
                                    }
                                ]
                            }
                        ]
                    }
                }
                self.assertEqual(
                    validator.validate_hook_targets(settings, tracked),
                    ["shared hook dynamic shell syntax cannot be validated: sh"]
                    if command.startswith("sh ")
                    else [
                        "shared hook dynamic shell syntax cannot be validated: "
                        "(./.ievo/hooks/capture.sh)"
                    ],
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

    def test_shell_variants_and_powershell_preview_reject_inline_modes(self) -> None:
        for command, arguments, expected in [
            (
                "dash",
                [
                    "-c",
                    r""". "$HOME"$(printf %b '\057.ievo\057hooks\057capture.sh')""",
                ],
                "dash -c",
            ),
            ("ash", ["-c", "echo inline"], "ash -c"),
            ("ksh.exe", ["-c", "echo inline"], "ksh.exe -c"),
            ("fish", ["-c", "echo inline"], "fish -c"),
            ("pwsh-preview", ["-Command", "Write-Output ok"], "pwsh-preview -Command"),
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

    def test_unknown_interpreter_inline_options_are_rejected_fail_closed(self) -> None:
        for command, arguments, expected in [
            ("busybox", ["sh", "-c", "echo inline"], "busybox -c"),
            ("perl", ["-e", "print 'inline'"], "perl -e"),
            ("perl", ["-eprint('inline')"], "perl -eprint('inline')"),
            ("perl", ["-weprint('inline')"], "perl -weprint('inline')"),
            ("php", ["-r", "1"], "php -r"),
            ("php", ["-rCODE"], "php -rCODE"),
            ("busybox", ["sh", "-xc", "echo inline"], "busybox -xc"),
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

    def test_unknown_runtime_inline_subcommands_are_rejected_fail_closed(self) -> None:
        for subcommand in ("eval", "repl"):
            with self.subTest(subcommand=subcommand):
                settings = {
                    "hooks": {
                        "SessionStart": [
                            {
                                "hooks": [
                                    {
                                        "command": "deno",
                                        "args": [subcommand, "console.log('inline')"],
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
                        f"validated: deno {subcommand}"
                    ],
                )

    def test_interpreter_loader_options_validate_embedded_targets(self) -> None:
        for command, arguments, tracked, expected in [
            (
                "node",
                [
                    "--require=/home/alice/.ievo/hooks/capture.js",
                    "scripts/tracked.js",
                ],
                {"scripts/tracked.js"},
                (
                    "shared hook target must not be absolute: "
                    "/home/alice/.ievo/hooks/capture.js"
                ),
            ),
            (
                "ruby",
                [
                    "-r/home/alice/.ievo/hooks/capture.rb",
                    "scripts/tracked.rb",
                ],
                {"scripts/tracked.rb"},
                (
                    "shared hook target must not be absolute: "
                    "/home/alice/.ievo/hooks/capture.rb"
                ),
            ),
            (
                "node",
                [
                    "-r/home/alice/.ievo/hooks/capture.js",
                    "scripts/tracked.js",
                ],
                {"scripts/tracked.js"},
                (
                    "shared hook target must not be absolute: "
                    "/home/alice/.ievo/hooks/capture.js"
                ),
            ),
            (
                "node",
                ["--import=$HOME/.ievo/hooks/capture.js", "scripts/tracked.js"],
                {"scripts/tracked.js"},
                (
                    "shared hook target must not be home-relative: "
                    "$HOME/.ievo/hooks/capture.js"
                ),
            ),
            (
                "node",
                ["--require=./tools/preload.js", "scripts/tracked.js"],
                {"scripts/tracked.js"},
                "shared hook target is not tracked: tools/preload.js",
            ),
        ]:
            with self.subTest(command=command, arguments=arguments):
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
                    validator.validate_hook_targets(settings, tracked),
                    [expected],
                )

    def test_separated_loader_options_do_not_hide_the_main_script(self) -> None:
        for command, arguments in (
            (
                "node",
                [
                    "--require",
                    "./tools/tracked-preload.js",
                    "tools/untracked-main.js",
                ],
            ),
            (
                "ruby",
                [
                    "-r",
                    "./tools/tracked-preload.rb",
                    "tools/untracked-main.rb",
                ],
            ),
        ):
            with self.subTest(command=command, arguments=arguments):
                preload = arguments[1].removeprefix("./")
                main = arguments[2]
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
                    validator.validate_hook_targets(settings, {preload}),
                    [f"shared hook target is not tracked: {main}"],
                )

                quoted_settings = {
                    "hooks": {
                        "SessionStart": [
                            {
                                "hooks": [
                                    {
                                        "command": " ".join([command, *arguments]),
                                    }
                                ]
                            }
                        ]
                    }
                }
                self.assertEqual(
                    validator.validate_hook_targets(
                        quoted_settings,
                        {preload},
                    ),
                    [f"shared hook target is not tracked: {main}"],
                )

    def test_loader_options_validate_windows_path_forms(self) -> None:
        for target, expected in (
            (
                r".\tools\missing.js",
                r"shared hook target is not tracked: .\tools\missing.js",
            ),
            (
                r"..\tools\missing.js",
                r"shared hook target is not tracked: ..\tools\missing.js",
            ),
            (
                r"\\server\share\missing.js",
                (
                    "shared hook target must not be absolute: "
                    r"\\server\share\missing.js"
                ),
            ),
            (
                r"\\?\C:\tools\missing.js",
                (
                    "shared hook target must not be absolute: "
                    r"\\?\C:\tools\missing.js"
                ),
            ),
        ):
            with self.subTest(target=target):
                settings = {
                    "hooks": {
                        "SessionStart": [
                            {
                                "hooks": [
                                    {
                                        "command": "node",
                                        "args": [
                                            f"--require={target}",
                                            "scripts/tracked.js",
                                        ],
                                    }
                                ]
                            }
                        ]
                    }
                }
                self.assertEqual(
                    validator.validate_hook_targets(
                        settings,
                        {"scripts/tracked.js"},
                    ),
                    [expected],
                )

    def test_option_terminator_preserves_leading_dash_script(self) -> None:
        for command, target in (
            ("node", "-local-hook.js"),
            ("python3", "-local-hook.py"),
            ("ruby", "-local-hook.rb"),
            ("sh", "-local-hook.sh"),
        ):
            with self.subTest(command=command):
                settings = {
                    "hooks": {
                        "SessionStart": [
                            {
                                "hooks": [
                                    {
                                        "command": command,
                                        "args": ["--", target],
                                    }
                                ]
                            }
                        ]
                    }
                }
                self.assertEqual(
                    validator.validate_hook_targets(settings, set()),
                    [f"shared hook target is not tracked: {target}"],
                )

    def test_unmodeled_interpreter_options_are_rejected_fail_closed(self) -> None:
        for command, arguments in (
            (
                "node",
                [
                    "--env-file=/home/alice/.ievo/hooks/runtime.env",
                    "scripts/tracked.js",
                ],
            ),
            (
                "node",
                [
                    "--env-file-if-exists=/home/alice/.ievo/hooks/runtime.env",
                    "scripts/tracked.js",
                ],
            ),
            (
                "ruby",
                [
                    "-C/home/alice/.ievo/hooks",
                    "scripts/tracked.rb",
                ],
            ),
            (
                "bash",
                [
                    "--init-file",
                    "tools/tracked-init.sh",
                    "-i",
                    "tools/untracked-main.sh",
                ],
            ),
        ):
            with self.subTest(command=command, arguments=arguments):
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
                failures = validator.validate_hook_targets(
                    settings,
                    {
                        "scripts/tracked.js",
                        "scripts/tracked.rb",
                        "tools/tracked-init.sh",
                    },
                )
                self.assertEqual(len(failures), 1)
                self.assertIn(
                    "shared hook interpreter invocation cannot be validated",
                    failures[0],
                )

    def test_interpreters_require_an_explicit_script(self) -> None:
        for command, arguments in (
            ("sh", []),
            ("sh", ["-s"]),
            ("python3", []),
            ("python3", ["-"]),
            ("node", []),
            ("node", ["-"]),
            ("ruby", []),
            ("ruby", ["-"]),
        ):
            with self.subTest(command=command, arguments=arguments):
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
                failures = validator.validate_hook_targets(settings, set())
                self.assertEqual(len(failures), 1)
                self.assertIn(
                    "shared hook interpreter invocation cannot be validated",
                    failures[0],
                )

    def test_machine_local_known_interpreter_paths_are_rejected(self) -> None:
        for command, script in (
            (
                "/home/alice/.ievo/hooks/node",
                "scripts/tracked.js",
            ),
            ("/tmp/python3", "scripts/tracked.py"),
            (
                r"C:\Users\alice\.ievo\node.exe",
                "scripts/tracked.js",
            ),
            (
                "/usr/bin/../../home/alice/.ievo/hooks/node",
                "scripts/tracked.js",
            ),
            (
                "/bin/../tmp/python3",
                "scripts/tracked.py",
            ),
            ("/USR/BIN/python3", "scripts/tracked.py"),
            ("/usr/local/bin/node", "scripts/tracked.js"),
            ("/opt/homebrew/bin/ruby", "scripts/tracked.rb"),
            ("/usr/LOCAL/bin/node", "scripts/tracked.js"),
            ("/OPT/HOMEBREW/bin/ruby", "scripts/tracked.rb"),
            (
                (
                    r"C:\Windows\System32\..\..\Users\alice"
                    r"\.ievo\node.exe"
                ),
                "scripts/tracked.js",
            ),
        ):
            with self.subTest(command=command):
                settings = {
                    "hooks": {
                        "SessionStart": [
                            {
                                "hooks": [
                                    {
                                        "command": command,
                                        "args": [script],
                                    }
                                ]
                            }
                        ]
                    }
                }
                self.assertEqual(
                    validator.validate_hook_targets(settings, {script}),
                    [f"shared hook target must not be absolute: {command}"],
                )

        quoted_settings = {
            "hooks": {
                "SessionStart": [
                    {
                        "hooks": [
                            {
                                "command": (
                                    "'\\Users\\alice\\.ievo\\node.exe' "
                                    "scripts/tracked.js"
                                ),
                            }
                        ]
                    }
                ]
            }
        }
        self.assertEqual(
            validator.validate_hook_targets(
                quoted_settings,
                {"scripts/tracked.js"},
            ),
            [
                "shared hook target must not be absolute: "
                r"\Users\alice\.ievo\node.exe"
            ],
        )

    def test_trusted_windows_interpreter_path_is_supported(self) -> None:
        script = "scripts/tracked.ps1"
        settings = {
            "hooks": {
                "SessionStart": [
                    {
                        "hooks": [
                            {
                                "command": (
                                    r"C:\Windows\System32\WindowsPowerShell"
                                    r"\v1.0\powershell.exe"
                                ),
                                "args": ["-File", script],
                            }
                        ]
                    }
                ]
            }
        }
        self.assertEqual(
            validator.validate_hook_targets(settings, {script}),
            [],
        )

    def test_machine_local_launcher_names_are_not_unwrapped(self) -> None:
        for command, expected in (
            (
                "/home/alice/.ievo/hooks/env",
                (
                    "shared hook target must not be absolute: "
                    "/home/alice/.ievo/hooks/env"
                ),
            ),
            (
                "/tmp/command",
                "shared hook target must not be absolute: /tmp/command",
            ),
            (
                "/tmp/exec",
                "shared hook target must not be absolute: /tmp/exec",
            ),
            (
                "tools/env",
                "shared hook target is not tracked: tools/env",
            ),
        ):
            with self.subTest(command=command):
                settings = {
                    "hooks": {
                        "SessionStart": [
                            {
                                "hooks": [
                                    {
                                        "command": command,
                                        "args": [
                                            "sh",
                                            "scripts/tracked.sh",
                                        ],
                                    }
                                ]
                            }
                        ]
                    }
                }
                self.assertEqual(
                    validator.validate_hook_targets(
                        settings,
                        {"scripts/tracked.sh"},
                    ),
                    [expected],
                )

        tracked_settings = {
            "hooks": {
                "SessionStart": [
                    {
                        "hooks": [
                            {
                                "command": "tools/env",
                                "args": ["--config", "config.toml"],
                            }
                        ]
                    }
                ]
            }
        }
        self.assertEqual(
            validator.validate_hook_targets(tracked_settings, {"tools/env"}),
            [],
        )

    def test_env_rejects_all_assignments_fail_closed(self) -> None:
        for assignment in (
            "NODE_OPTIONS=--require=/home/alice/.ievo/hooks/capture.js",
            "NODE_PATH=/home/alice/.ievo/hooks",
            "JAVA_TOOL_OPTIONS=-javaagent:/home/alice/.ievo/hook.jar",
            "CLASSPATH=/home/alice/.ievo/classes",
            "GIT_CONFIG_COUNT=1",
        ):
            with self.subTest(assignment=assignment):
                settings = {
                    "hooks": {
                        "SessionStart": [
                            {
                                "hooks": [
                                    {
                                        "command": "env",
                                        "args": [
                                            assignment,
                                            "node",
                                            "scripts/tracked.js",
                                        ],
                                    }
                                ]
                            }
                        ]
                    }
                }
                variable = assignment.split("=", 1)[0]
                self.assertEqual(
                    validator.validate_hook_targets(
                        settings,
                        {"scripts/tracked.js"},
                    ),
                    [
                        "shared hook environment assignment cannot "
                        f"be validated: {variable}"
                    ],
                )

    def test_implicit_environment_prefix_is_rejected_fail_closed(self) -> None:
        for command, arguments, variable in (
            (
                "NODE_OPTIONS=--require=left-pad node",
                ["scripts/tracked.js"],
                "NODE_OPTIONS",
            ),
            (
                "NODE_PATH=tools node",
                ["--require=capture", "scripts/tracked.js"],
                "NODE_PATH",
            ),
            (
                "BASH_ENV=hook bash",
                ["scripts/tracked.sh"],
                "BASH_ENV",
            ),
        ):
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
                    validator.validate_hook_targets(
                        settings,
                        {arguments[-1]},
                    ),
                    [
                        "shared hook environment assignment cannot be "
                        f"validated: {variable}"
                    ],
                )

    def test_multiline_and_command_substitution_are_rejected_before_split(
        self,
    ) -> None:
        for suffix in (
            "\n/home/alice/.ievo/hooks/capture.sh",
            "\r\n/home/alice/.ievo/hooks/capture.sh",
            " $(local-hook)",
            " `local-hook`",
            " <(local-hook)",
            " >(local-hook)",
        ):
            with self.subTest(suffix=suffix):
                settings = {
                    "hooks": {
                        "SessionStart": [
                            {
                                "hooks": [
                                    {
                                        "command": ("sh scripts/tracked.sh" + suffix),
                                    }
                                ]
                            }
                        ]
                    }
                }
                self.assertEqual(
                    validator.validate_hook_targets(
                        settings,
                        {"scripts/tracked.sh"},
                    ),
                    ["shared hook dynamic shell syntax cannot be validated: sh"],
                )

    def test_raw_shell_expansion_cannot_collide_with_tracked_name(self) -> None:
        for command, tracked in (
            ("sh $HOOK_SCRIPT", "$HOOK_SCRIPT"),
            ("sh ${HOOK_SCRIPT}", "${HOOK_SCRIPT}"),
            ("sh scripts/*.sh", "scripts/*.sh"),
            ("./$HOOK_EXEC", "$HOOK_EXEC"),
            ("pwsh -File $env:HOOK_SCRIPT", "$env:HOOK_SCRIPT"),
            ("cmd /c %HOOK_SCRIPT%", "%HOOK_SCRIPT%"),
        ):
            with self.subTest(command=command):
                settings = {
                    "hooks": {
                        "SessionStart": [
                            {
                                "hooks": [
                                    {
                                        "command": command,
                                    }
                                ]
                            }
                        ]
                    }
                }
                self.assertEqual(
                    validator.validate_hook_targets(settings, {tracked}),
                    [
                        "shared hook dynamic shell syntax cannot be validated: "
                        f"{command.split()[0]}"
                    ],
                )

    def test_loader_urls_are_rejected_fail_closed(self) -> None:
        for target in (
            "file:///home/alice/.ievo/hooks/capture.mjs",
            "data:text/javascript,console.log('inline')",
            "https://example.invalid/hook.mjs",
        ):
            with self.subTest(target=target):
                settings = {
                    "hooks": {
                        "SessionStart": [
                            {
                                "hooks": [
                                    {
                                        "command": "node",
                                        "args": [
                                            f"--import={target}",
                                            "scripts/tracked.js",
                                        ],
                                    }
                                ]
                            }
                        ]
                    }
                }
                self.assertEqual(
                    validator.validate_hook_targets(
                        settings,
                        {"scripts/tracked.js"},
                    ),
                    [f"shared hook loader URL cannot be validated: {target}"],
                )

    def test_loader_options_require_non_empty_operands(self) -> None:
        for command, arguments, option in (
            ("node", ["--require"], "--require"),
            ("node", ["--require="], "--require="),
            ("node", ["--import"], "--import"),
            ("ruby", ["-r"], "-r"),
        ):
            with self.subTest(command=command, arguments=arguments):
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
                        "shared hook interpreter invocation cannot be "
                        f"validated: {command} (loader option {option!r} "
                        "has no operand)"
                    ],
                )

    def test_loader_package_specifiers_are_rejected_fail_closed(self) -> None:
        for command, arguments, specifier in (
            (
                "node",
                ["--require=@scope/package", "scripts/tracked.js"],
                "@scope/package",
            ),
            (
                "ruby",
                ["-rbundler/setup", "scripts/tracked.rb"],
                "bundler/setup",
            ),
            (
                "node",
                ["--require=left-pad", "scripts/tracked.js"],
                "left-pad",
            ),
        ):
            with self.subTest(command=command):
                tracked_script = arguments[-1]
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
                    validator.validate_hook_targets(
                        settings,
                        {tracked_script, specifier},
                    ),
                    [
                        "shared hook interpreter invocation cannot be "
                        f"validated: {command} (loader specifier "
                        f"{specifier!r} is not an explicit filesystem target)"
                    ],
                )

    def test_loader_package_spelling_cannot_collide_with_tracked_path(self) -> None:
        for command, option, specifier, script in (
            ("node", "--require=", "left-pad", "scripts/tracked.js"),
            (
                "node",
                "--require=",
                "tools/preload.js",
                "scripts/tracked.js",
            ),
            (
                "node",
                "--require=",
                "@scope/package",
                "scripts/tracked.js",
            ),
            ("ruby", "-r", "bundler/setup", "scripts/tracked.rb"),
        ):
            with self.subTest(command=command, specifier=specifier):
                settings = {
                    "hooks": {
                        "SessionStart": [
                            {
                                "hooks": [
                                    {
                                        "command": command,
                                        "args": [
                                            f"{option}{specifier}",
                                            script,
                                        ],
                                    }
                                ]
                            }
                        ]
                    }
                }
                failures = validator.validate_hook_targets(
                    settings,
                    {specifier, script},
                )
                self.assertEqual(len(failures), 1)
                self.assertIn(
                    "is not an explicit filesystem target",
                    failures[0],
                )

    def test_tracked_wrapper_options_are_not_treated_as_inline_code(self) -> None:
        for target in ("scripts/tracked-hook.sh", "scripts/python", "tools/sh"):
            with self.subTest(target=target):
                settings = {
                    "hooks": {
                        "SessionStart": [
                            {
                                "hooks": [
                                    {
                                        "command": target,
                                        "args": ["-c", "config.toml"],
                                    }
                                ]
                            }
                        ]
                    }
                }
                self.assertEqual(
                    validator.validate_hook_targets(settings, {target}),
                    [],
                )

    def test_tracked_name_collision_does_not_trust_path_interpreter(self) -> None:
        for command, arguments, expected in (
            ("node", ["--eval=process.exit()"], "node --eval=process.exit()"),
            ("sh", ["-c", "local-hook"], "sh -c"),
            ("python3", ["-c", "print('inline')"], "python3 -c"),
        ):
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
                    validator.validate_hook_targets(settings, {command}),
                    [
                        "shared hook inline interpreter mode cannot be "
                        f"validated: {expected}"
                    ],
                )

    def test_tracked_wrapper_config_option_is_not_an_executable_target(self) -> None:
        target = "scripts/tracked-hook.sh"
        settings = {
            "hooks": {
                "SessionStart": [
                    {
                        "hooks": [
                            {
                                "command": target,
                                "args": ["--config=config/local.toml"],
                            }
                        ]
                    }
                ]
            }
        }
        self.assertEqual(
            validator.validate_hook_targets(settings, {target}),
            [],
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

    def test_windows_interpreter_executables_reject_inline_modes(self) -> None:
        for command, arguments, expected in [
            (
                "python.exe",
                ["-c", "exec(open('tools/hook.py').read())"],
                "python.exe -c",
            ),
            (
                "node.exe",
                ["--eval=require('./tools/hook.js')"],
                "node.exe --eval=require('./tools/hook.js')",
            ),
            (
                "py.exe",
                ["-c", "exec(open('tools/hook.py').read())"],
                "py.exe -c",
            ),
            (
                "pyw.exe",
                ["-c", "exec(open('tools/hook.py').read())"],
                "pyw.exe -c",
            ),
            (
                "pythonw.exe",
                ["-c", "exec(open('tools/hook.py').read())"],
                "pythonw.exe -c",
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

    def test_inline_ruby_commands_are_rejected(self) -> None:
        settings = {
            "hooks": {
                "SessionStart": [
                    {
                        "hooks": [
                            {
                                "command": "ruby.exe",
                                "args": [
                                    "-eload File.join('.ievo','hooks','capture.rb')"
                                ],
                            }
                        ]
                    }
                ]
            }
        }
        self.assertEqual(
            validator.validate_hook_targets(settings, set()),
            [
                "shared hook inline interpreter mode cannot be validated: "
                "ruby.exe -eload File.join('.ievo','hooks','capture.rb')"
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
