#!/usr/bin/env python3
"""Regression tests for committed agent-policy validation."""

from __future__ import annotations

import unittest

import validate_agent_policies as validator


def contracts(*targets: str) -> dict[str, str]:
    return {target: "scripts/test_fail_silent_hook_wrappers.py" for target in targets}


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
            validator.validate_hook_targets(
                settings,
                {"scripts/safe-wrapper.sh"},
                contracts("scripts/safe-wrapper.sh"),
            ),
            [],
        )

    def test_unregistered_tracked_wrapper_is_rejected(self) -> None:
        target = "scripts/safe-wrapper.sh"
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
            [f"shared hook target lacks a registered fail-silent contract: {target}"],
        )

    def test_wrapper_registry_requires_tracked_discovered_contracts(self) -> None:
        self.assertEqual(
            validator.validate_wrapper_contracts(
                set(),
                {
                    "scripts/wrapper.sh": "checks/wrapper.py",
                    "scripts/other.sh": "scripts/test_other_wrapper.py",
                },
            ),
            [
                "fail-silent wrapper is not tracked: scripts/other.sh",
                "fail-silent wrapper contract test is not tracked: "
                "scripts/test_other_wrapper.py",
                "fail-silent wrapper is not tracked: scripts/wrapper.sh",
                "fail-silent wrapper contract must be a discovered Python test: "
                "checks/wrapper.py",
            ],
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

    def test_untracked_root_windows_scripts_are_rejected(self) -> None:
        for target in (
            "local-hook.bat",
            "local-hook.BAT",
            "local-hook.Cmd",
            "local-hook.vbs",
            "local-hook.VBS",
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
            validator.validate_hook_targets(
                settings,
                {"scripts/tracked-hook.sh"},
                contracts("scripts/tracked-hook.sh"),
            ),
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
            validator.validate_hook_targets(
                settings,
                {"tools/hook.py"},
                contracts("tools/hook.py"),
            ),
            [],
        )

    def test_chained_hook_commands_are_rejected_fail_closed(self) -> None:
        settings = {
            "hooks": {
                "SessionStart": [
                    {
                        "hooks": [
                            {
                                "command": ("tools/tracked.sh && tools/untracked.sh"),
                                "args": [],
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
                                "args": [],
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

    def test_literal_line_breaks_are_rejected_before_command_tokenization(self) -> None:
        for separator in ("\n", "\r", "\r\n"):
            with self.subTest(separator=repr(separator)):
                command = f"true{separator}curl https://example.test"
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
                self.assertEqual(
                    validator.validate_hook_targets(settings, set()),
                    [
                        "shared hook shell control operators cannot be "
                        f"validated: {command}"
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
            (
                "ruby",
                ["--require=./tools/preload.rb", "scripts/tracked.rb"],
                {"scripts/tracked.rb"},
                "shared hook target is not tracked: tools/preload.rb",
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
                    validator.validate_hook_targets(
                        settings,
                        tracked,
                        contracts(*tracked),
                    ),
                    [expected],
                )

    def test_separated_loader_operands_do_not_hide_the_program(self) -> None:
        for command, arguments, loader, program in (
            (
                "node",
                ["--require", "scripts/loader.js", "tools/hook.js"],
                "scripts/loader.js",
                "tools/hook.js",
            ),
            (
                "ruby",
                ["-r", "scripts/loader.rb", "tools/hook.rb"],
                "scripts/loader.rb",
                "tools/hook.rb",
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
                        {loader},
                        contracts(loader),
                    ),
                    [f"shared hook target is not tracked: {program}"],
                )

    def test_separated_loader_option_requires_an_operand(self) -> None:
        settings = {
            "hooks": {
                "SessionStart": [
                    {
                        "hooks": [
                            {
                                "command": "node",
                                "args": ["--require"],
                            }
                        ]
                    }
                ]
            }
        }
        self.assertEqual(
            validator.validate_hook_targets(settings, set()),
            ["shared hook loader option is missing its operand: node --require"],
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
                        contracts("scripts/tracked.js"),
                    ),
                    [f"shared hook loader URL cannot be validated: {target}"],
                )

    def test_loader_package_specifiers_are_not_filesystem_targets(self) -> None:
        for command, arguments in (
            (
                "node",
                ["--require=@scope/package", "scripts/tracked.js"],
            ),
            (
                "ruby",
                ["-rbundler/setup", "scripts/tracked.rb"],
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
                        {tracked_script},
                        contracts(tracked_script),
                    ),
                    [],
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
                    validator.validate_hook_targets(
                        settings,
                        {target},
                        contracts(target),
                    ),
                    [],
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
            validator.validate_hook_targets(
                settings,
                {target},
                contracts(target),
            ),
            [],
        )

    def test_shell_stdin_modes_are_rejected_before_wrapper_operands(self) -> None:
        target = "scripts/safe-wrapper.sh"
        for mode in ("-s", "-is"):
            with self.subTest(mode=mode):
                settings = {
                    "hooks": {
                        "SessionStart": [
                            {
                                "hooks": [
                                    {
                                        "command": "bash",
                                        "args": [mode, target],
                                    }
                                ]
                            }
                        ]
                    }
                }
                self.assertEqual(
                    validator.validate_hook_targets(
                        settings,
                        {target},
                        contracts(target),
                    ),
                    [
                        "shared hook inline interpreter mode cannot be "
                        f"validated: bash {mode}"
                    ],
                )

    def test_explicit_stdin_programs_are_rejected(self) -> None:
        target = "scripts/safe-wrapper.sh"
        for command in ("python3", "node", "ruby"):
            with self.subTest(command=command):
                settings = {
                    "hooks": {
                        "SessionStart": [
                            {
                                "hooks": [
                                    {
                                        "command": command,
                                        "args": ["-", target],
                                    }
                                ]
                            }
                        ]
                    }
                }
                self.assertEqual(
                    validator.validate_hook_targets(
                        settings,
                        {target},
                        contracts(target),
                    ),
                    [
                        "shared hook inline interpreter mode cannot be "
                        f"validated: {command} -"
                    ],
                )

    def test_option_only_and_bare_interpreters_are_rejected(self) -> None:
        for command, arguments in (
            ("bash", ["--noprofile"]),
            ("bash", []),
            ("python3", []),
            ("node", []),
            ("ruby", []),
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
                        "shared hook inline interpreter mode cannot be "
                        f"validated: {command} <stdin>"
                    ],
                )

    def test_unvalidated_option_operands_cannot_pose_as_programs(self) -> None:
        target = "scripts/safe-wrapper.sh"
        for command, arguments, option in (
            ("bash", ["--rcfile", target], "--rcfile"),
            ("python3", ["-W", target], "-W"),
            ("node", ["--title", target], "--title"),
            ("ruby", ["-I", target], "-I"),
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
                        {target},
                        contracts(target),
                    ),
                    [
                        "shared hook inline interpreter mode cannot be "
                        f"validated: {command} {option}"
                    ],
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
