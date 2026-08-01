#!/usr/bin/env python3
"""Lifecycle tests for the project-local iEvo hook manager."""

from __future__ import annotations

import collections
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

if os.name == "nt":
    import ctypes

import manage_ievo_hooks as manager


class IevoHookLifecycleTests(unittest.TestCase):
    def write_json(self, root: Path, relative: Path, value: object) -> None:
        path = root / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(json.dumps(value, indent=2) + "\n", encoding="utf-8")

    def read_json(self, root: Path, relative: Path) -> dict[str, object]:
        value = json.loads((root / relative).read_text(encoding="utf-8"))
        self.assertIsInstance(value, dict)
        return value

    def command_entry(
        self, target: Path, *, shell_form: bool = False
    ) -> dict[str, object]:
        if shell_form:
            return {
                "type": "command",
                "command": f"sh {target.as_posix()}",
            }
        return {
            "type": "command",
            "command": "sh",
            "args": [target.as_posix()],
        }

    def group(
        self,
        entry: dict[str, object],
        *,
        matcher: str | None = None,
    ) -> dict[str, object]:
        result: dict[str, object] = {"hooks": [entry]}
        if matcher is not None:
            result["matcher"] = matcher
        return result

    def records(self, settings: dict[str, object]) -> set[tuple[str, str]]:
        _, records = manager.strip_ievo_entries(settings)
        return {(event, target.as_posix()) for event, target, _ in records}

    def create_generated_files(self, root: Path) -> None:
        for target in manager.SCRIPT_TARGETS.values():
            path = root / target
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text("#!/bin/sh\nexit 0\n", encoding="utf-8")
            local = root / target.with_name(f"{target.stem}.local.sh")
            local.write_text("#!/bin/sh\nexit 0\n", encoding="utf-8")
        vendor = root / manager.VENDOR_DIRECTORY
        vendor.mkdir(parents=True)
        (vendor / "capture.mjs").write_text("// fixture\n", encoding="utf-8")
        flag = root / manager.FLAG
        flag.parent.mkdir(parents=True, exist_ok=True)
        flag.write_text(
            "enabled: true\nsignal: corrections-only\n"
            "auto_write_scope: project-wide-only\n",
            encoding="utf-8",
        )

    def create_gitignore(self, root: Path, *, upstream_shims: bool) -> None:
        subprocess.run(["git", "init", "--quiet", str(root)], check=True)
        lines = [
            "# iEvo local-only artifacts",
            ".claude/settings.local.json",
            ".codex/hooks.json",
        ]
        if upstream_shims:
            lines.extend(sorted(manager.UPSTREAM_TRACKED_SHIM_LINES))
        else:
            lines.append(".ievo/hooks/")
        (root / manager.GITIGNORE).write_text(
            "\n".join(lines) + "\n",
            encoding="utf-8",
        )

    def commit_tracked_baseline(self, root: Path) -> None:
        subprocess.run(
            [
                "git",
                "-C",
                str(root),
                "add",
                manager.GITIGNORE.as_posix(),
                manager.CLAUDE_SHARED.as_posix(),
                manager.FLAG.as_posix(),
            ],
            check=True,
        )
        subprocess.run(
            [
                "git",
                "-C",
                str(root),
                "-c",
                "user.name=iEvo lifecycle test",
                "-c",
                "user.email=ievo-lifecycle@example.invalid",
                "commit",
                "--quiet",
                "-m",
                "tracked baseline",
            ],
            check=True,
        )

    def run_manager(
        self, root: Path, *arguments: str
    ) -> subprocess.CompletedProcess[str]:
        # #169 follow-up: resolve before handing to the CLI. main() now
        # rejects any symlink/reparse component anywhere in the raw --root
        # argument, and this machine's own tempdir prefix (e.g. macOS's
        # /var -> /private/var) is exactly such a component -- resolving
        # here matches what a real, non-test caller's --root looks like
        # (this helper only ever asserts success below, so a legitimate,
        # already-resolved root is always the correct input).
        result = subprocess.run(
            [
                sys.executable,
                str(Path(manager.__file__).resolve()),
                "--root",
                str(root.resolve()),
                *arguments,
            ],
            check=False,
            capture_output=True,
            text=True,
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        return result

    def test_full_lifecycle_is_symmetric_idempotent_and_preserves_other_hooks(
        self,
    ) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            unrelated_entry = {
                "type": "command",
                "command": "sh",
                "args": ["scripts/unrelated.sh"],
            }
            shared_baseline = {
                "permissions": {"allow": ["Bash(gh api*)"]},
                "hooks": {
                    "UserPromptSubmit": [
                        {
                            "matcher": "project",
                            "hooks": [unrelated_entry],
                        }
                    ],
                    "Stop": [{"matcher": "empty", "hooks": []}],
                    "Notification": [],
                },
            }
            shared = {
                "permissions": {"allow": ["Bash(gh api*)"]},
                "hooks": {
                    "UserPromptSubmit": [
                        {
                            "matcher": "project",
                            "hooks": [
                                self.command_entry(
                                    manager.SCRIPT_TARGETS["correction-capture"]
                                ),
                                unrelated_entry,
                            ],
                        }
                    ],
                    "SessionStart": [
                        self.group(
                            self.command_entry(
                                manager.SCRIPT_TARGETS["evo-analysis-nudge"]
                            ),
                            matcher="startup",
                        )
                    ],
                    "PostToolUseFailure": [
                        self.group(
                            self.command_entry(
                                manager.SCRIPT_TARGETS["failure-capture"]
                            )
                        )
                    ],
                    "PermissionDenied": [
                        self.group(
                            self.command_entry(
                                manager.SCRIPT_TARGETS["failure-capture"]
                            )
                        )
                    ],
                    "Stop": [{"matcher": "empty", "hooks": []}],
                    "Notification": [],
                },
            }
            local = {
                "env": {"LOCAL_ONLY": "true"},
                "hooks": {
                    "UserPromptSubmit": [
                        self.group(
                            self.command_entry(
                                manager.SCRIPT_TARGETS["correction-capture"]
                            )
                        )
                    ]
                },
            }
            codex = {
                "hooks": {
                    "UserPromptSubmit": [
                        self.group(
                            self.command_entry(
                                manager.SCRIPT_TARGETS["correction-capture"],
                                shell_form=True,
                            )
                        )
                    ],
                    "SessionStart": [
                        self.group(
                            self.command_entry(
                                manager.SCRIPT_TARGETS["evo-analysis-nudge"],
                                shell_form=True,
                            ),
                            matcher="startup",
                        )
                    ],
                    "PermissionRequest": [
                        self.group(
                            self.command_entry(
                                manager.SCRIPT_TARGETS["failure-capture"],
                                shell_form=True,
                            )
                        )
                    ],
                    "Stop": [self.group(unrelated_entry)],
                }
            }

            self.write_json(root, manager.CLAUDE_SHARED, shared_baseline)
            self.create_generated_files(root)
            self.create_gitignore(root, upstream_shims=False)
            self.commit_tracked_baseline(root)
            shutil.rmtree(root / manager.SCRIPT_DIRECTORY)

            self.assertEqual(
                subprocess.run(
                    ["git", "-C", str(root), "status", "--short"],
                    check=True,
                    capture_output=True,
                    text=True,
                ).stdout,
                "",
            )

            self.write_json(root, manager.CLAUDE_SHARED, shared)
            self.write_json(root, manager.CLAUDE_LOCAL, local)
            self.write_json(root, manager.CODEX_LOCAL, codex)
            self.create_generated_files(root)
            self.create_gitignore(root, upstream_shims=True)

            self.run_manager(root, "localize")
            self.run_manager(root, "check", "--smoke")

            gitignore = (root / manager.GITIGNORE).read_text(encoding="utf-8")
            self.assertIn(".ievo/hooks/\n", gitignore)
            for line in manager.UPSTREAM_TRACKED_SHIM_LINES:
                self.assertNotIn(f"{line}\n", gitignore)

            rewritten_shared = self.read_json(root, manager.CLAUDE_SHARED)
            rewritten_local = self.read_json(root, manager.CLAUDE_LOCAL)
            self.assertEqual(self.records(rewritten_shared), set())
            self.assertEqual(
                rewritten_shared["permissions"],
                {"allow": ["Bash(gh api*)"]},
            )
            self.assertEqual(
                rewritten_shared["hooks"]["UserPromptSubmit"][0]["hooks"],
                [unrelated_entry],
            )
            self.assertEqual(
                rewritten_shared["hooks"]["Stop"],
                [{"matcher": "empty", "hooks": []}],
            )
            self.assertEqual(rewritten_shared["hooks"]["Notification"], [])
            self.assertEqual(rewritten_local["env"], {"LOCAL_ONLY": "true"})
            self.assertEqual(
                rewritten_local["hooks"]["UserPromptSubmit"][0]["matcher"],
                "project",
            )
            self.assertEqual(
                self.records(rewritten_local),
                {
                    (
                        "UserPromptSubmit",
                        manager.SCRIPT_TARGETS["correction-capture"].as_posix(),
                    ),
                    (
                        "SessionStart",
                        manager.SCRIPT_TARGETS["evo-analysis-nudge"].as_posix(),
                    ),
                    (
                        "PostToolUseFailure",
                        manager.SCRIPT_TARGETS["failure-capture"].as_posix(),
                    ),
                    (
                        "PermissionDenied",
                        manager.SCRIPT_TARGETS["failure-capture"].as_posix(),
                    ),
                },
            )

            localized_snapshot = {
                relative: (root / relative).read_text(encoding="utf-8")
                for relative in (
                    manager.CLAUDE_SHARED,
                    manager.CLAUDE_LOCAL,
                    manager.CODEX_LOCAL,
                )
            }
            self.run_manager(root, "localize")
            self.assertEqual(
                localized_snapshot,
                {
                    relative: (root / relative).read_text(encoding="utf-8")
                    for relative in localized_snapshot
                },
            )

            self.run_manager(root, "disable")
            for relative in (
                manager.CLAUDE_SHARED,
                manager.CLAUDE_LOCAL,
                manager.CODEX_LOCAL,
            ):
                self.assertEqual(self.records(self.read_json(root, relative)), set())
            self.assertEqual(
                self.read_json(root, manager.CLAUDE_SHARED)["hooks"][
                    "UserPromptSubmit"
                ][0]["hooks"],
                [unrelated_entry],
            )
            self.assertEqual(
                self.read_json(root, manager.CODEX_LOCAL)["hooks"]["Stop"][0]["hooks"],
                [unrelated_entry],
            )
            self.assertTrue((root / manager.FLAG).is_file())
            manager.ensure_corrections_only_intent(root)
            for target in (
                *manager.SCRIPT_TARGETS.values(),
                *manager.LOCAL_COMPANIONS,
                manager.VENDOR_DIRECTORY,
            ):
                self.assertFalse((root / target).exists())
            self.assertEqual(
                subprocess.run(
                    ["git", "-C", str(root), "status", "--short"],
                    check=True,
                    capture_output=True,
                    text=True,
                ).stdout,
                "",
            )

            disabled_snapshot = {
                relative: (root / relative).read_text(encoding="utf-8")
                for relative in (
                    manager.CLAUDE_SHARED,
                    manager.CLAUDE_LOCAL,
                    manager.CODEX_LOCAL,
                )
            }
            self.run_manager(root, "disable")
            self.assertEqual(
                disabled_snapshot,
                {
                    relative: (root / relative).read_text(encoding="utf-8")
                    for relative in disabled_snapshot
                },
            )

    def test_localize_fails_before_mutation_when_a_target_is_missing(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            shared = {
                "hooks": {
                    "UserPromptSubmit": [
                        self.group(
                            self.command_entry(
                                manager.SCRIPT_TARGETS["correction-capture"]
                            )
                        )
                    ]
                }
            }
            self.write_json(root, manager.CLAUDE_SHARED, shared)
            self.create_gitignore(root, upstream_shims=False)
            flag = root / manager.FLAG
            flag.parent.mkdir(parents=True, exist_ok=True)
            flag.write_text("enabled: true\n", encoding="utf-8")
            before = (root / manager.CLAUDE_SHARED).read_text(encoding="utf-8")

            with self.assertRaisesRegex(
                manager.HookLifecycleError,
                "hook target is missing",
            ):
                manager.localize(root)

            self.assertEqual(
                (root / manager.CLAUDE_SHARED).read_text(encoding="utf-8"),
                before,
            )
            self.assertFalse((root / manager.CLAUDE_LOCAL).exists())

    def test_localize_checks_effective_ignore_policy_before_config_writes(
        self,
    ) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            target = manager.SCRIPT_TARGETS["correction-capture"]
            shared = {
                "hooks": {"UserPromptSubmit": [self.group(self.command_entry(target))]}
            }
            self.write_json(root, manager.CLAUDE_SHARED, shared)
            self.create_generated_files(root)
            self.create_gitignore(root, upstream_shims=False)
            gitignore = root / manager.GITIGNORE
            gitignore.write_text(
                gitignore.read_text(encoding="utf-8")
                + "!.claude/settings.local.json\n",
                encoding="utf-8",
            )
            shared_before = (root / manager.CLAUDE_SHARED).read_text(encoding="utf-8")

            with self.assertRaisesRegex(
                manager.HookLifecycleError,
                "machine-local iEvo paths are not ignored",
            ):
                manager.localize(root)

            self.assertEqual(
                (root / manager.CLAUDE_SHARED).read_text(encoding="utf-8"),
                shared_before,
            )
            self.assertFalse((root / manager.CLAUDE_LOCAL).exists())

    def test_localize_requires_the_complete_corrections_only_intent(self) -> None:
        invalid_flags = {
            "missing signal": ("enabled: true\nauto_write_scope: project-wide-only\n"),
            "broadened signal": (
                "enabled: true\nsignal: all-feedback\n"
                "auto_write_scope: project-wide-only\n"
            ),
            "missing scope": "enabled: true\nsignal: corrections-only\n",
            "broadened scope": (
                "enabled: true\nsignal: corrections-only\n"
                "auto_write_scope: every-file\n"
            ),
            "conflicting duplicate": (
                "enabled: true\nsignal: corrections-only\n"
                "signal: all-feedback\nauto_write_scope: project-wide-only\n"
            ),
        }
        for label, contents in invalid_flags.items():
            with self.subTest(label=label), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                target = manager.SCRIPT_TARGETS["correction-capture"]
                shared = {
                    "hooks": {
                        "UserPromptSubmit": [self.group(self.command_entry(target))]
                    }
                }
                self.write_json(root, manager.CLAUDE_SHARED, shared)
                self.create_generated_files(root)
                self.create_gitignore(root, upstream_shims=False)
                (root / manager.FLAG).write_text(contents, encoding="utf-8")
                shared_before = (root / manager.CLAUDE_SHARED).read_text(
                    encoding="utf-8"
                )

                with self.assertRaisesRegex(
                    manager.HookLifecycleError,
                    "evo-auto.flag",
                ):
                    manager.localize(root)

                self.assertEqual(
                    (root / manager.CLAUDE_SHARED).read_text(encoding="utf-8"),
                    shared_before,
                )
                self.assertFalse((root / manager.CLAUDE_LOCAL).exists())

    def test_check_requires_the_complete_corrections_only_intent(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            target = manager.SCRIPT_TARGETS["correction-capture"]
            self.write_json(root, manager.CLAUDE_SHARED, {"hooks": {}})
            self.write_json(
                root,
                manager.CLAUDE_LOCAL,
                {
                    "hooks": {
                        "UserPromptSubmit": [self.group(self.command_entry(target))]
                    }
                },
            )
            self.create_generated_files(root)
            self.create_gitignore(root, upstream_shims=False)
            (root / manager.FLAG).write_text(
                "enabled: true\nsignal: every-prompt\n"
                "auto_write_scope: project-wide-only\n",
                encoding="utf-8",
            )

            with self.assertRaisesRegex(
                manager.HookLifecycleError,
                "corrections-only intent",
            ):
                manager.check(root, smoke=False)

    def test_disable_requires_the_complete_corrections_only_intent(self) -> None:
        invalid_flags = {
            "missing signal": ("enabled: true\nauto_write_scope: project-wide-only\n"),
            "conflicting duplicate": (
                "enabled: true\nsignal: corrections-only\n"
                "signal: all-feedback\nauto_write_scope: project-wide-only\n"
            ),
        }
        for label, contents in invalid_flags.items():
            with self.subTest(label=label), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                target = manager.SCRIPT_TARGETS["correction-capture"]
                shared = {
                    "hooks": {
                        "UserPromptSubmit": [self.group(self.command_entry(target))]
                    }
                }
                self.write_json(root, manager.CLAUDE_SHARED, shared)
                self.create_generated_files(root)
                self.create_gitignore(root, upstream_shims=False)
                (root / manager.FLAG).write_text(contents, encoding="utf-8")
                shared_before = (root / manager.CLAUDE_SHARED).read_bytes()
                target_before = (root / target).read_bytes()

                with self.assertRaisesRegex(
                    manager.HookLifecycleError,
                    "evo-auto.flag",
                ):
                    manager.disable(root)

                self.assertEqual(
                    (root / manager.CLAUDE_SHARED).read_bytes(), shared_before
                )
                self.assertEqual((root / target).read_bytes(), target_before)

    def test_lifecycle_accepts_matching_duplicate_intent_fields(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            target = manager.SCRIPT_TARGETS["correction-capture"]
            self.write_json(
                root,
                manager.CLAUDE_SHARED,
                {
                    "hooks": {
                        "UserPromptSubmit": [self.group(self.command_entry(target))]
                    }
                },
            )
            self.create_generated_files(root)
            self.create_gitignore(root, upstream_shims=False)
            (root / manager.FLAG).write_text(
                "enabled: true\nenabled: true\n"
                "signal: corrections-only\nsignal: corrections-only\n"
                "auto_write_scope: project-wide-only\n"
                "auto_write_scope: project-wide-only\n",
                encoding="utf-8",
            )

            manager.localize(root)
            manager.check(root, smoke=False)
            manager.disable(root)
            self.assertFalse((root / target).exists())

    def test_localize_writes_the_local_copy_before_removing_shared_entries(
        self,
    ) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            target = manager.SCRIPT_TARGETS["correction-capture"]
            shared = {
                "hooks": {"UserPromptSubmit": [self.group(self.command_entry(target))]}
            }
            self.write_json(root, manager.CLAUDE_SHARED, shared)
            self.create_generated_files(root)
            self.create_gitignore(root, upstream_shims=False)

            write_json = manager.atomic_write_json

            def fail_shared_write(
                write_root: Path,
                relative: Path,
                value: dict[str, object],
            ) -> None:
                if relative == manager.CLAUDE_SHARED:
                    raise OSError("injected shared-settings write failure")
                write_json(write_root, relative, value)

            with mock.patch.object(
                manager,
                "atomic_write_json",
                side_effect=fail_shared_write,
            ):
                with self.assertRaisesRegex(
                    OSError,
                    "injected shared-settings write failure",
                ):
                    manager.localize(root)

            self.assertEqual(
                self.records(self.read_json(root, manager.CLAUDE_SHARED)),
                {("UserPromptSubmit", target.as_posix())},
            )
            self.assertEqual(
                self.records(self.read_json(root, manager.CLAUDE_LOCAL)),
                {("UserPromptSubmit", target.as_posix())},
            )

    def test_codex_only_localize_validates_without_creating_claude_local(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            self.write_json(root, manager.CLAUDE_SHARED, {"permissions": {}})
            self.write_json(
                root,
                manager.CODEX_LOCAL,
                {
                    "hooks": {
                        "UserPromptSubmit": [
                            self.group(
                                self.command_entry(
                                    manager.SCRIPT_TARGETS["correction-capture"],
                                    shell_form=True,
                                )
                            )
                        ]
                    }
                },
            )
            self.create_generated_files(root)
            self.create_gitignore(root, upstream_shims=True)

            manager.localize(root)
            manager.check(root, smoke=True)

            self.assertFalse((root / manager.CLAUDE_LOCAL).exists())
            self.assertEqual(
                self.read_json(root, manager.CLAUDE_SHARED),
                {"permissions": {}},
            )

    def test_localize_fails_before_mutation_when_codex_config_is_invalid(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            shared = {
                "hooks": {
                    "UserPromptSubmit": [
                        self.group(
                            self.command_entry(
                                manager.SCRIPT_TARGETS["correction-capture"]
                            )
                        )
                    ]
                }
            }
            self.write_json(root, manager.CLAUDE_SHARED, shared)
            codex_path = root / manager.CODEX_LOCAL
            codex_path.parent.mkdir(parents=True, exist_ok=True)
            codex_path.write_text("{not-json\n", encoding="utf-8")
            self.create_generated_files(root)
            self.create_gitignore(root, upstream_shims=False)
            before = (root / manager.CLAUDE_SHARED).read_text(encoding="utf-8")

            with self.assertRaisesRegex(
                manager.HookLifecycleError,
                "cannot read valid JSON",
            ):
                manager.localize(root)

            self.assertEqual(
                (root / manager.CLAUDE_SHARED).read_text(encoding="utf-8"),
                before,
            )
            self.assertFalse((root / manager.CLAUDE_LOCAL).exists())

    def test_disable_fails_before_mutation_when_any_config_is_invalid(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            shared = {
                "hooks": {
                    "UserPromptSubmit": [
                        self.group(
                            self.command_entry(
                                manager.SCRIPT_TARGETS["correction-capture"]
                            )
                        )
                    ]
                }
            }
            self.write_json(root, manager.CLAUDE_SHARED, shared)
            local_path = root / manager.CLAUDE_LOCAL
            local_path.parent.mkdir(parents=True, exist_ok=True)
            local_path.write_text("{not-json\n", encoding="utf-8")
            self.create_generated_files(root)

            with self.assertRaisesRegex(
                manager.HookLifecycleError,
                "cannot read valid JSON",
            ):
                manager.disable(root)

            self.assertTrue((root / manager.FLAG).exists())
            self.assertNotEqual(
                self.records(self.read_json(root, manager.CLAUDE_SHARED)),
                set(),
            )

    def test_disable_rejects_an_unsupported_reference_before_mutation(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            target = manager.SCRIPT_TARGETS["correction-capture"]
            companion = manager.LOCAL_COMPANIONS[0]
            shared = {
                "hooks": {
                    "UserPromptSubmit": [self.group(self.command_entry(target))],
                    "Stop": [self.group(self.command_entry(target, shell_form=True))],
                    "DirectHandler": [self.command_entry(companion, shell_form=True)],
                }
            }
            self.write_json(root, manager.CLAUDE_SHARED, shared)
            self.create_generated_files(root)
            before = (root / manager.CLAUDE_SHARED).read_text(encoding="utf-8")

            with self.assertRaisesRegex(
                manager.HookLifecycleError,
                "unsupported iEvo hook reference",
            ):
                manager.disable(root)

            self.assertEqual(
                (root / manager.CLAUDE_SHARED).read_text(encoding="utf-8"),
                before,
            )
            self.assertTrue((root / manager.FLAG).is_file())
            self.assertTrue((root / target).is_file())
            self.assertTrue((root / companion).is_file())

    def test_disable_rejects_lexical_managed_aliases_before_mutation(self) -> None:
        aliases = (
            ".ievo/./hooks/scripts/correction-capture.sh",
            ".ievo//hooks///scripts/correction-capture.sh",
            r".ievo\hooks\scripts\correction-capture.sh",
            r".ievo/ho\oks/scripts/correction-capture.sh",
            r".ie\vo/hooks/scripts/correction-capture.sh",
            ".IEVO/HOOKS/SCRIPTS/correction-capture.sh",
            ".ievo/other/../hooks/scripts/correction-capture.sh",
            '.ievo/ho"oks"/scripts/correction-capture.sh',
            '.ie"vo"/hooks/scripts/correction-capture.sh',
            ".ievo/ho\\\noks/scripts/correction-capture.sh",
            ".ievo/hoo${EMPTY}ks/scripts/correction-capture.sh",
            ".ie${EMPTY}vo/hooks/scripts/correction-capture.sh",
            ".ievo/${HOOKS}/scripts/correction-capture.sh",
            "${IEVO}/hooks/scripts/correction-capture.sh",
            "${IEVO}/${HOOKS}/scripts/correction-capture.sh",
            ".ievo/hoo$(printf '')ks/scripts/correction-capture.sh",
            ".ievo/hoo`printf ''`ks/scripts/correction-capture.sh",
            ".ievo/hoo%EMPTY%ks/scripts/correction-capture.sh",
            ".ievo/hoo!EMPTY!ks/scripts/correction-capture.sh",
            ".ievo/hoo$env:EMPTYks/scripts/correction-capture.sh",
            "${IEVO_HOOKS}/scripts/failure-capture.sh",
            r"%IEVO_HOOKS%\scripts\failure-capture.sh",
            ".ievo/ho?ks/scripts/failure-capture.sh",
            ".ievo/ho[o]ks/scripts/failure-capture.sh",
            ".ievo/[[:alpha:]]ooks/scripts/failure-capture.sh",
            ".ievo/[[.h.]]ooks/scripts/failure-capture.sh",
            ".ievo/[[=h=]]ooks/scripts/failure-capture.sh",
            ".ievo/ho`oks/scripts/failure-capture.sh",
            r".ievo\ho^oks\scripts\failure-capture.sh",
            ".ievo/ho`\noks/scripts/failure-capture.sh",
            ".ievo/ho`\r\noks/scripts/failure-capture.sh",
            ".ievo/ho^\noks/scripts/failure-capture.sh",
            ".ievo/ho^\r\noks/scripts/failure-capture.sh",
            ".ievo/ho`printf o\n`ks/scripts/failure-capture.sh",
            ".ievo/ho{ok,xx}s/scripts/failure-capture.sh",
            ".ievo/ho{o..o}ks/scripts/failure-capture.sh",
            ".ievo/ho@(ok|xx)s/scripts/failure-capture.sh",
            ".ievo/$'hoo\\x6bs'/scripts/failure-capture.sh",
            "sh ('.ie'+'vo/ho'+'oks/scripts/failure-capture.sh')",
            "sh ('.ie','vo/hooks/scripts/failure-capture.sh' -join '')",
            "sh ('{0}{1}' -f '.ie','vo/hooks/scripts/failure-capture.sh')",
            "[string]::Concat('.ie','vo/hooks/scripts/failure-capture.sh')",
            "& sh (('ievo/hooks/scripts/failure-capture.sh').Insert(0,'.'))",
            "& sh (('ievo/hooks/scripts/failure-capture.sh').PadLeft(47,'.'))",
            "$[IEVO_HOOKS]/scripts/failure-capture.sh",
            r"%A\scripts\failure-capture.sh",
            r"!IEVO_HOOKS:~0,11!\scripts\failure-capture.sh",
            r"IEVO~1\hooks\scripts\failure-capture.sh",
            r".ievo\HOOKS~1\scripts\failure-capture.sh",
            "@hookArgs",
            ".ievo/hooks",
        )
        for alias in aliases:
            with self.subTest(alias=alias), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                shared = {
                    "hooks": {
                        "FutureEvent": [
                            self.group(
                                {"type": "command", "command": "sh", "args": [alias]}
                            )
                        ]
                    }
                }
                self.write_json(root, manager.CLAUDE_SHARED, shared)
                self.create_generated_files(root)
                target = manager.SCRIPT_TARGETS["correction-capture"]
                before = (root / manager.CLAUDE_SHARED).read_text(encoding="utf-8")

                with self.assertRaisesRegex(
                    manager.HookLifecycleError,
                    "unsupported iEvo hook reference",
                ):
                    manager.disable(root)

                self.assertEqual(
                    (root / manager.CLAUDE_SHARED).read_text(encoding="utf-8"),
                    before,
                )
                self.assertTrue((root / target).is_file())

    def test_disable_rejects_a_directory_shaped_script_before_mutation(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            target = manager.SCRIPT_TARGETS["correction-capture"]
            self.write_json(root, manager.CLAUDE_SHARED, {"hooks": {}})
            self.write_json(
                root,
                manager.CLAUDE_LOCAL,
                {
                    "hooks": {
                        "UserPromptSubmit": [self.group(self.command_entry(target))]
                    }
                },
            )
            self.create_generated_files(root)
            self.create_gitignore(root, upstream_shims=False)
            target_path = root / target
            target_path.unlink()
            target_path.mkdir()
            sentinel = target_path / "force-tracked.txt"
            sentinel.write_text("keep\n", encoding="utf-8")
            subprocess.run(
                ["git", "-C", str(root), "add", "-f", sentinel.relative_to(root)],
                check=True,
            )
            local_before = (root / manager.CLAUDE_LOCAL).read_bytes()

            with self.assertRaisesRegex(
                manager.HookLifecycleError,
                "generated removal target must be a regular file",
            ):
                manager.disable(root)

            self.assertEqual(
                (root / manager.CLAUDE_LOCAL).read_bytes(),
                local_before,
            )
            self.assertEqual(sentinel.read_text(encoding="utf-8"), "keep\n")

    def test_disable_fails_before_mutation_when_vendor_walk_fails(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            target = manager.SCRIPT_TARGETS["correction-capture"]
            self.write_json(root, manager.CLAUDE_SHARED, {"hooks": {}})
            self.write_json(
                root,
                manager.CLAUDE_LOCAL,
                {
                    "hooks": {
                        "UserPromptSubmit": [self.group(self.command_entry(target))]
                    }
                },
            )
            self.create_generated_files(root)
            self.create_gitignore(root, upstream_shims=False)
            local_before = (root / manager.CLAUDE_LOCAL).read_bytes()
            generated_before = {
                relative: (root / relative).read_bytes()
                for relative in (
                    *manager.SCRIPT_TARGETS.values(),
                    *manager.LOCAL_COMPANIONS,
                    manager.VENDOR_DIRECTORY / "capture.mjs",
                )
            }

            def fail_walk(*_args: object, **kwargs: object) -> tuple[()]:
                onerror = kwargs["onerror"]
                assert callable(onerror)
                onerror(PermissionError("vendor subtree is unreadable"))
                return ()

            with (
                mock.patch.object(manager.os, "walk", side_effect=fail_walk),
                self.assertRaisesRegex(
                    manager.HookLifecycleError,
                    "cannot inspect managed vendor tree",
                ),
            ):
                manager.disable(root)

            self.assertEqual(
                (root / manager.CLAUDE_LOCAL).read_bytes(),
                local_before,
            )
            for relative, contents in generated_before.items():
                self.assertEqual((root / relative).read_bytes(), contents)

    def test_disable_fails_before_mutation_when_vendor_root_probe_fails(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            target = manager.SCRIPT_TARGETS["correction-capture"]
            self.write_json(root, manager.CLAUDE_SHARED, {"hooks": {}})
            self.write_json(
                root,
                manager.CLAUDE_LOCAL,
                {
                    "hooks": {
                        "UserPromptSubmit": [self.group(self.command_entry(target))]
                    }
                },
            )
            self.create_generated_files(root)
            self.create_gitignore(root, upstream_shims=False)
            local_before = (root / manager.CLAUDE_LOCAL).read_bytes()
            vendor = root / manager.VENDOR_DIRECTORY
            original_lstat = Path.lstat

            def fail_vendor_lstat(path: Path) -> object:
                if path == vendor:
                    raise PermissionError("vendor root is unreadable")
                return original_lstat(path)

            with (
                mock.patch.object(
                    manager,
                    "ensure_no_symlink_components",
                    return_value=None,
                ),
                mock.patch.object(Path, "lstat", new=fail_vendor_lstat),
                self.assertRaisesRegex(
                    manager.HookLifecycleError,
                    "cannot inspect managed vendor path",
                ),
            ):
                manager.disable(root)

            self.assertEqual((root / manager.CLAUDE_LOCAL).read_bytes(), local_before)
            self.assertTrue((root / target).is_file())
            self.assertTrue((vendor / "capture.mjs").is_file())

    def test_lifecycle_lock_blocks_a_concurrent_disable(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            target = manager.SCRIPT_TARGETS["correction-capture"]
            self.write_json(root, manager.CLAUDE_SHARED, {"hooks": {}})
            self.write_json(
                root,
                manager.CLAUDE_LOCAL,
                {
                    "hooks": {
                        "UserPromptSubmit": [self.group(self.command_entry(target))]
                    }
                },
            )
            self.create_generated_files(root)
            self.create_gitignore(root, upstream_shims=False)
            local_before = (root / manager.CLAUDE_LOCAL).read_bytes()

            with manager.lifecycle_lock(root):
                with self.assertRaisesRegex(
                    manager.HookLifecycleError,
                    "another iEvo hook lifecycle operation is active",
                ):
                    manager.disable(root)

            self.assertEqual((root / manager.CLAUDE_LOCAL).read_bytes(), local_before)
            self.assertTrue((root / target).is_file())

    def test_linked_worktrees_use_distinct_absolute_and_relative_lock_paths(
        self,
    ) -> None:
        with tempfile.TemporaryDirectory() as directory:
            workspace = Path(directory)
            gitdirs = workspace / "common.git" / "worktrees"
            relative_gitdir = gitdirs / "relative"
            absolute_gitdir = gitdirs / "absolute"
            relative_gitdir.mkdir(parents=True)
            absolute_gitdir.mkdir()
            relative_root = workspace / "relative-worktree"
            absolute_root = workspace / "absolute-worktree"
            relative_root.mkdir()
            absolute_root.mkdir()
            (relative_root / ".git").write_text(
                "gitdir: ../common.git/worktrees/relative\n",
                encoding="utf-8",
            )
            (absolute_root / ".git").write_text(
                f"gitdir: {absolute_gitdir.resolve()}\n",
                encoding="utf-8",
            )

            relative_lock = manager.lifecycle_lock_path(relative_root)
            absolute_lock = manager.lifecycle_lock_path(absolute_root)

            self.assertEqual(
                relative_lock,
                relative_gitdir.resolve() / "pycc-ievo-hooks.lock",
            )
            self.assertEqual(
                absolute_lock,
                absolute_gitdir.resolve() / "pycc-ievo-hooks.lock",
            )
            self.assertNotEqual(relative_lock, absolute_lock)
            with manager.lifecycle_lock(relative_root):
                with self.assertRaisesRegex(
                    manager.HookLifecycleError,
                    "another iEvo hook lifecycle operation is active",
                ):
                    with manager.lifecycle_lock(relative_root):
                        self.fail("the same linked-worktree lock was acquired twice")
            with manager.lifecycle_lock(absolute_root):
                pass

    def test_linked_worktree_rejects_a_malformed_gitdir_declaration(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / ".git").write_text("not-a-gitdir\n", encoding="utf-8")

            with self.assertRaisesRegex(
                manager.HookLifecycleError,
                "invalid gitdir declaration",
            ):
                manager.lifecycle_lock_path(root)

    @unittest.skipUnless(os.name != "nt", "POSIX symlink regression")
    def test_linked_worktree_rejects_a_symlinked_gitdir_component(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            workspace = Path(directory).resolve()
            real_parent = workspace / "real-gitdirs"
            gitdir = real_parent / "worktree"
            gitdir.mkdir(parents=True)
            redirected_parent = workspace / "redirected-gitdirs"
            redirected_parent.symlink_to(real_parent, target_is_directory=True)
            root = workspace / "worktree"
            root.mkdir()
            (root / ".git").write_text(
                f"gitdir: {redirected_parent / 'worktree'}\n",
                encoding="utf-8",
            )

            with self.assertRaisesRegex(
                manager.HookLifecycleError,
                "directory path contains.*symlink",
            ):
                manager.lifecycle_lock_path(root)

    @unittest.skipUnless(os.name == "nt", "Windows junction regression")
    def test_linked_worktree_rejects_a_junctioned_gitdir_component(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            workspace = Path(directory)
            real_parent = workspace / "real-gitdirs"
            gitdir = real_parent / "worktree"
            gitdir.mkdir(parents=True)
            redirected_parent = workspace / "redirected-gitdirs"
            subprocess.run(
                ["cmd", "/c", "mklink", "/J", str(redirected_parent), str(real_parent)],
                check=True,
                capture_output=True,
                text=True,
            )
            root = workspace / "worktree"
            root.mkdir()
            (root / ".git").write_text(
                f"gitdir: {redirected_parent / 'worktree'}\n",
                encoding="utf-8",
            )

            with self.assertRaisesRegex(
                manager.HookLifecycleError,
                "directory path contains.*reparse point",
            ):
                manager.lifecycle_lock_path(root)

    @unittest.skipUnless(os.name != "nt", "POSIX symlink regression")
    def test_lifecycle_lock_rejects_a_symlinked_entry(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            workspace = Path(directory)
            root = workspace / "repo"
            root.mkdir()
            self.create_gitignore(root, upstream_shims=False)
            external = workspace / "external-lock"
            external.write_bytes(b"keep\n")
            lock = manager.lifecycle_lock_path(root)
            lock.symlink_to(external)

            with self.assertRaisesRegex(
                manager.HookLifecycleError,
                "regular non-link file",
            ):
                with manager.lifecycle_lock(root):
                    self.fail("a symlinked lifecycle lock was acquired")

            self.assertEqual(external.read_bytes(), b"keep\n")

    @unittest.skipUnless(os.name == "nt", "Windows reparse-point regression")
    def test_lifecycle_lock_rejects_a_windows_junction_entry(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            workspace = Path(directory)
            root = workspace / "repo"
            root.mkdir()
            self.create_gitignore(root, upstream_shims=False)
            external = workspace / "external-lock-directory"
            external.mkdir()
            sentinel = external / "sentinel"
            sentinel.write_text("keep\n", encoding="utf-8")
            lock = manager.lifecycle_lock_path(root)
            subprocess.run(
                ["cmd", "/c", "mklink", "/J", str(lock), str(external)],
                check=True,
                capture_output=True,
                text=True,
            )

            with self.assertRaisesRegex(
                manager.HookLifecycleError,
                "regular non-link file",
            ):
                with manager.lifecycle_lock(root):
                    self.fail("a junction lifecycle lock was acquired")

            self.assertEqual(sentinel.read_text(encoding="utf-8"), "keep\n")

    def test_non_git_lifecycle_lock_stays_inside_the_validated_root(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            lock = manager.lifecycle_lock_path(root)

            self.assertEqual(lock, root / ".pycc-ievo-hooks.lock")
            with manager.lifecycle_lock(root):
                with self.assertRaisesRegex(
                    manager.HookLifecycleError,
                    "another iEvo hook lifecycle operation is active",
                ):
                    with manager.lifecycle_lock(root):
                        self.fail("the same non-git lock was acquired twice")
            self.assertTrue(lock.is_file())

    def test_stale_lock_file_does_not_block_disable(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            target = manager.SCRIPT_TARGETS["correction-capture"]
            self.write_json(root, manager.CLAUDE_SHARED, {"hooks": {}})
            self.write_json(
                root,
                manager.CLAUDE_LOCAL,
                {
                    "hooks": {
                        "UserPromptSubmit": [self.group(self.command_entry(target))]
                    }
                },
            )
            self.create_generated_files(root)
            self.create_gitignore(root, upstream_shims=False)
            lock = manager.lifecycle_lock_path(root)
            lock.write_bytes(b"\0orphaned owner metadata\n")

            manager.disable(root)

            self.assertFalse((root / target).exists())
            self.assertFalse((root / manager.VENDOR_DIRECTORY).exists())
            self.assertTrue(lock.is_file())

    def test_disable_revalidates_vendor_snapshot_before_removal(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            target = manager.SCRIPT_TARGETS["correction-capture"]
            self.write_json(root, manager.CLAUDE_SHARED, {"hooks": {}})
            self.write_json(
                root,
                manager.CLAUDE_LOCAL,
                {
                    "hooks": {
                        "UserPromptSubmit": [self.group(self.command_entry(target))]
                    }
                },
            )
            self.create_generated_files(root)
            self.create_gitignore(root, upstream_shims=False)
            original_check = manager.ensure_machine_local_paths_ignored
            calls = 0
            inserted = root / manager.VENDOR_DIRECTORY / "late.mjs"

            def mutate_before_recheck(check_root: Path) -> manager.VendorSnapshot:
                nonlocal calls
                calls += 1
                if calls == 2:
                    inserted.write_text("// late\n", encoding="utf-8")
                return original_check(check_root)

            with (
                mock.patch.object(
                    manager,
                    "ensure_machine_local_paths_ignored",
                    side_effect=mutate_before_recheck,
                ),
                self.assertRaisesRegex(
                    manager.HookLifecycleError,
                    "managed vendor tree changed",
                ),
            ):
                manager.disable(root)

            self.assertTrue((root / target).is_file())
            self.assertTrue((root / manager.VENDOR_DIRECTORY / "capture.mjs").is_file())
            self.assertEqual(inserted.read_text(encoding="utf-8"), "// late\n")

    def test_disable_revalidates_generated_file_snapshot_before_removal(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            target = manager.SCRIPT_TARGETS["correction-capture"]
            self.write_json(root, manager.CLAUDE_SHARED, {"hooks": {}})
            self.write_json(
                root,
                manager.CLAUDE_LOCAL,
                {
                    "hooks": {
                        "UserPromptSubmit": [self.group(self.command_entry(target))]
                    }
                },
            )
            self.create_generated_files(root)
            self.create_gitignore(root, upstream_shims=False)
            original_inspect = manager.inspect_regular_removal_files
            calls = 0
            changed = root / target

            def mutate_before_recheck(
                inspect_root: Path,
                targets: object,
            ) -> dict[str, manager.FileIdentity]:
                nonlocal calls
                calls += 1
                if calls == 2:
                    changed.write_text(
                        "#!/bin/sh\n# changed\nexit 0\n", encoding="utf-8"
                    )
                return original_inspect(inspect_root, targets)  # type: ignore[arg-type]

            with (
                mock.patch.object(
                    manager,
                    "inspect_regular_removal_files",
                    side_effect=mutate_before_recheck,
                ),
                self.assertRaisesRegex(
                    manager.HookLifecycleError,
                    "generated removal targets changed",
                ),
            ):
                manager.disable(root)

            self.assertEqual(
                changed.read_text(encoding="utf-8"),
                "#!/bin/sh\n# changed\nexit 0\n",
            )
            self.assertTrue((root / manager.VENDOR_DIRECTORY / "capture.mjs").is_file())

    def test_disable_revalidates_ancestors_after_snapshots(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            workspace = Path(directory)
            root = workspace / "repo"
            root.mkdir()
            target = manager.SCRIPT_TARGETS["correction-capture"]
            self.write_json(root, manager.CLAUDE_SHARED, {"hooks": {}})
            self.write_json(
                root,
                manager.CLAUDE_LOCAL,
                {
                    "hooks": {
                        "UserPromptSubmit": [self.group(self.command_entry(target))]
                    }
                },
            )
            self.create_generated_files(root)
            self.create_gitignore(root, upstream_shims=False)
            original_check = manager.ensure_snapshot_unchanged
            checks = 0
            hooks = root / manager.HOOK_DIRECTORY
            relocated = workspace / "relocated-hooks"

            def redirect_after_snapshot(
                label: str,
                expected: dict[str, manager.FileIdentity],
                actual: dict[str, manager.FileIdentity],
            ) -> None:
                nonlocal checks
                original_check(label, expected, actual)
                checks += 1
                if checks == 2:
                    hooks.rename(relocated)
                    hooks.symlink_to(relocated, target_is_directory=True)

            with (
                mock.patch.object(
                    manager,
                    "ensure_snapshot_unchanged",
                    side_effect=redirect_after_snapshot,
                ),
                self.assertRaisesRegex(
                    manager.HookLifecycleError,
                    "symlink component",
                ),
            ):
                manager.disable(root)

            relocated_target = relocated / target.relative_to(manager.HOOK_DIRECTORY)
            self.assertTrue(relocated_target.is_file())
            self.assertTrue(
                (relocated / "scripts" / "vendor" / "capture.mjs").is_file()
            )

    def test_disable_removes_a_nested_vendor_tree_deepest_first(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            target = manager.SCRIPT_TARGETS["correction-capture"]
            self.write_json(root, manager.CLAUDE_SHARED, {"hooks": {}})
            self.write_json(
                root,
                manager.CLAUDE_LOCAL,
                {
                    "hooks": {
                        "UserPromptSubmit": [self.group(self.command_entry(target))]
                    }
                },
            )
            self.create_generated_files(root)
            nested = root / manager.VENDOR_DIRECTORY / "one" / "two"
            nested.mkdir(parents=True)
            (nested / "nested.mjs").write_text("// nested\n", encoding="utf-8")
            unrelated = root / manager.SCRIPT_DIRECTORY / "unrelated.txt"
            unrelated.write_text("keep\n", encoding="utf-8")
            self.create_gitignore(root, upstream_shims=False)

            manager.disable(root)

            self.assertFalse((root / manager.VENDOR_DIRECTORY).exists())
            self.assertEqual(unrelated.read_text(encoding="utf-8"), "keep\n")

    def test_disable_detects_a_config_change_before_its_write(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            target = manager.SCRIPT_TARGETS["correction-capture"]
            entry = self.group(self.command_entry(target))
            self.write_json(
                root,
                manager.CLAUDE_SHARED,
                {"hooks": {"UserPromptSubmit": [entry]}},
            )
            self.write_json(
                root,
                manager.CLAUDE_LOCAL,
                {"hooks": {"UserPromptSubmit": [entry]}},
            )
            self.create_generated_files(root)
            self.create_gitignore(root, upstream_shims=False)
            original_write = manager.atomic_write_json

            def mutate_after_shared_write(
                write_root: Path,
                relative: Path,
                value: dict[str, object],
            ) -> None:
                original_write(write_root, relative, value)
                if relative == manager.CLAUDE_SHARED:
                    local = self.read_json(root, manager.CLAUDE_LOCAL)
                    local["concurrent"] = "keep"
                    self.write_json(root, manager.CLAUDE_LOCAL, local)

            with (
                mock.patch.object(
                    manager,
                    "atomic_write_json",
                    side_effect=mutate_after_shared_write,
                ),
                self.assertRaisesRegex(
                    manager.HookLifecycleError,
                    "configuration changed during disable",
                ),
            ):
                manager.disable(root)

            local = self.read_json(root, manager.CLAUDE_LOCAL)
            self.assertEqual(local["concurrent"], "keep")
            self.assertNotEqual(self.records(local), set())
            self.assertTrue((root / target).is_file())

    def test_disable_checks_effective_ignore_policy_before_mutation(self) -> None:
        for state in ("unignored", "force-tracked-local", "force-tracked-vendor"):
            with self.subTest(state=state), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                target = manager.SCRIPT_TARGETS["correction-capture"]
                self.write_json(root, manager.CLAUDE_SHARED, {"hooks": {}})
                self.write_json(
                    root,
                    manager.CLAUDE_LOCAL,
                    {
                        "hooks": {
                            "UserPromptSubmit": [self.group(self.command_entry(target))]
                        }
                    },
                )
                self.create_generated_files(root)
                self.create_gitignore(root, upstream_shims=False)
                if state == "unignored":
                    gitignore = root / manager.GITIGNORE
                    gitignore.write_text(
                        gitignore.read_text(encoding="utf-8")
                        + "!.claude/settings.local.json\n",
                        encoding="utf-8",
                    )
                elif state == "force-tracked-local":
                    subprocess.run(
                        [
                            "git",
                            "-C",
                            str(root),
                            "add",
                            "-f",
                            manager.CLAUDE_LOCAL.as_posix(),
                        ],
                        check=True,
                    )
                else:
                    subprocess.run(
                        [
                            "git",
                            "-C",
                            str(root),
                            "add",
                            "-f",
                            manager.VENDOR_DIRECTORY.joinpath("capture.mjs").as_posix(),
                        ],
                        check=True,
                    )
                local_before = (root / manager.CLAUDE_LOCAL).read_text(encoding="utf-8")
                vendor_path = root / manager.VENDOR_DIRECTORY / "capture.mjs"
                vendor_before = vendor_path.read_text(encoding="utf-8")

                with self.assertRaisesRegex(
                    manager.HookLifecycleError,
                    "machine-local iEvo paths are not ignored",
                ):
                    manager.disable(root)

                self.assertEqual(
                    (root / manager.CLAUDE_LOCAL).read_text(encoding="utf-8"),
                    local_before,
                )
                self.assertTrue((root / target).is_file())
                self.assertEqual(
                    vendor_path.read_text(encoding="utf-8"),
                    vendor_before,
                )

    def test_disable_rejects_a_symlinked_vendor_descendant(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            workspace = Path(directory)
            root = workspace / "repo"
            root.mkdir()
            target = manager.SCRIPT_TARGETS["correction-capture"]
            self.write_json(root, manager.CLAUDE_SHARED, {"hooks": {}})
            self.write_json(
                root,
                manager.CLAUDE_LOCAL,
                {
                    "hooks": {
                        "UserPromptSubmit": [self.group(self.command_entry(target))]
                    }
                },
            )
            self.create_generated_files(root)
            self.create_gitignore(root, upstream_shims=False)
            external = workspace / "external.mjs"
            external.write_text("// keep\n", encoding="utf-8")
            vendor_file = root / manager.VENDOR_DIRECTORY / "capture.mjs"
            vendor_file.unlink()
            vendor_file.symlink_to(external)
            local_before = (root / manager.CLAUDE_LOCAL).read_text(encoding="utf-8")

            with self.assertRaisesRegex(
                manager.HookLifecycleError,
                "managed vendor tree contains a non-regular file",
            ):
                manager.disable(root)

            self.assertEqual(
                (root / manager.CLAUDE_LOCAL).read_text(encoding="utf-8"),
                local_before,
            )
            self.assertTrue((root / target).is_file())
            self.assertEqual(external.read_text(encoding="utf-8"), "// keep\n")

    def test_managed_references_scan_nested_vendor_values(self) -> None:
        settings = {
            "hooks": {
                "FutureEvent": {
                    "custom": [
                        "run",
                        manager.VENDOR_DIRECTORY.joinpath("runtime.sh")
                        .as_posix()
                        .replace("/", "\\"),
                    ]
                }
            }
        }

        self.assertEqual(
            manager.managed_target_references(settings),
            [("FutureEvent", manager.HOOK_DIRECTORY)],
        )

    def test_managed_references_normalize_static_path_aliases(self) -> None:
        aliases = (
            "sh .ievo/tmp/../hooks/scripts/future-capture.sh",
            "sh .IEVO//hooks/scripts/future-capture.sh",
            'sh ".ievo"/hooks/scripts/future-capture.sh',
            "echo ß; sh .ievo/hooks/scripts/future-capture.sh",
            r"sh .ievo/hoo\ks/scripts/future-capture.sh",
        )
        for alias in aliases:
            with self.subTest(alias=alias):
                settings = {"hooks": {"FutureEvent": [alias]}}
                self.assertEqual(
                    manager.managed_target_references(settings),
                    [("FutureEvent", manager.HOOK_DIRECTORY)],
                )

    def test_localize_rejects_future_managed_hook_before_mutation(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            target = manager.SCRIPT_TARGETS["correction-capture"]
            future_target = manager.HOOK_DIRECTORY / "scripts/future-capture.sh"
            shared = {
                "hooks": {
                    "UserPromptSubmit": [self.group(self.command_entry(target))],
                    "FutureEvent": [
                        self.group(self.command_entry(future_target, shell_form=True))
                    ],
                }
            }
            self.write_json(root, manager.CLAUDE_SHARED, shared)
            self.create_generated_files(root)
            self.create_gitignore(root, upstream_shims=False)
            shared_before = (root / manager.CLAUDE_SHARED).read_text(encoding="utf-8")

            with self.assertRaisesRegex(
                manager.HookLifecycleError,
                "unsupported iEvo hook reference",
            ):
                manager.localize(root)

            self.assertEqual(
                (root / manager.CLAUDE_SHARED).read_text(encoding="utf-8"),
                shared_before,
            )
            self.assertFalse((root / manager.CLAUDE_LOCAL).exists())

    def test_localize_rejects_a_lexical_managed_path_alias_before_mutation(
        self,
    ) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            target = manager.SCRIPT_TARGETS["correction-capture"]
            shared = {
                "hooks": {
                    "UserPromptSubmit": [self.group(self.command_entry(target))],
                    "FutureEvent": [
                        self.group(
                            self.command_entry(
                                Path(".ievo/tmp/../hooks/scripts/future-capture.sh"),
                                shell_form=True,
                            )
                        )
                    ],
                }
            }
            self.write_json(root, manager.CLAUDE_SHARED, shared)
            self.create_generated_files(root)
            self.create_gitignore(root, upstream_shims=False)
            shared_before = (root / manager.CLAUDE_SHARED).read_text(encoding="utf-8")

            with self.assertRaisesRegex(
                manager.HookLifecycleError,
                "unsupported iEvo hook reference",
            ):
                manager.localize(root)

            self.assertEqual(
                (root / manager.CLAUDE_SHARED).read_text(encoding="utf-8"),
                shared_before,
            )
            self.assertFalse((root / manager.CLAUDE_LOCAL).exists())

    def test_disable_rejects_a_case_insensitive_managed_path_alias_before_mutation(
        self,
    ) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            target = manager.SCRIPT_TARGETS["correction-capture"]
            shared = {
                "hooks": {
                    "FutureEvent": [
                        self.group(
                            self.command_entry(
                                Path(".IEVO/hooks/scripts/correction-capture.sh"),
                                shell_form=True,
                            )
                        )
                    ]
                }
            }
            self.write_json(root, manager.CLAUDE_SHARED, shared)
            self.create_generated_files(root)
            shared_before = (root / manager.CLAUDE_SHARED).read_text(encoding="utf-8")

            with self.assertRaisesRegex(
                manager.HookLifecycleError,
                "unsupported iEvo hook reference",
            ):
                manager.disable(root)

            self.assertEqual(
                (root / manager.CLAUDE_SHARED).read_text(encoding="utf-8"),
                shared_before,
            )
            self.assertTrue((root / target).is_file())

    def test_disable_rejects_a_managed_path_after_unicode_before_mutation(
        self,
    ) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            target = manager.SCRIPT_TARGETS["correction-capture"]
            shared = {
                "hooks": {
                    "FutureEvent": [
                        self.group(
                            {
                                "type": "command",
                                "command": f"echo ß; sh {target.as_posix()}",
                            }
                        )
                    ]
                }
            }
            self.write_json(root, manager.CLAUDE_SHARED, shared)
            self.create_generated_files(root)
            shared_before = (root / manager.CLAUDE_SHARED).read_text(encoding="utf-8")

            with self.assertRaisesRegex(
                manager.HookLifecycleError,
                "unsupported iEvo hook reference",
            ):
                manager.disable(root)

            self.assertEqual(
                (root / manager.CLAUDE_SHARED).read_text(encoding="utf-8"),
                shared_before,
            )
            self.assertTrue((root / target).is_file())

    def test_disable_rejects_a_posix_escaped_managed_path_before_mutation(
        self,
    ) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            target = manager.SCRIPT_TARGETS["failure-capture"]
            shared = {
                "hooks": {
                    "FutureEvent": [
                        self.group(
                            {
                                "type": "command",
                                "command": r"sh .ievo/hoo\ks/scripts/failure-capture.sh",
                            }
                        )
                    ]
                }
            }
            self.write_json(root, manager.CLAUDE_SHARED, shared)
            self.create_generated_files(root)
            shared_before = (root / manager.CLAUDE_SHARED).read_text(encoding="utf-8")

            with self.assertRaisesRegex(
                manager.HookLifecycleError,
                "unsupported iEvo hook reference",
            ):
                manager.disable(root)

            self.assertEqual(
                (root / manager.CLAUDE_SHARED).read_text(encoding="utf-8"),
                shared_before,
            )
            self.assertTrue((root / target).is_file())

    def test_symlinked_hook_ancestor_blocks_smoke_and_disable(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            workspace = Path(directory)
            root = workspace / "repo"
            root.mkdir()
            external_hooks = workspace / "external-hooks"
            external_scripts = external_hooks / "scripts"
            external_scripts.mkdir(parents=True)
            target = manager.SCRIPT_TARGETS["correction-capture"]
            external_target = external_hooks / target.relative_to(".ievo/hooks")
            smoke_marker = workspace / "smoke-ran"
            external_target.write_text(
                f"#!/bin/sh\ntouch '{smoke_marker}'\n",
                encoding="utf-8",
            )
            vendor = external_scripts / "vendor"
            vendor.mkdir()
            sentinel = vendor / "sentinel"
            sentinel.write_text("keep\n", encoding="utf-8")

            shared = {"hooks": {}}
            local = {
                "hooks": {"UserPromptSubmit": [self.group(self.command_entry(target))]}
            }
            self.write_json(root, manager.CLAUDE_SHARED, shared)
            self.write_json(root, manager.CLAUDE_LOCAL, local)
            flag = root / manager.FLAG
            flag.parent.mkdir(parents=True)
            flag.write_text(
                "enabled: true\nsignal: corrections-only\n"
                "auto_write_scope: project-wide-only\n",
                encoding="utf-8",
            )
            (root / ".ievo/hooks").symlink_to(
                external_hooks,
                target_is_directory=True,
            )
            self.create_gitignore(root, upstream_shims=False)
            local_before = (root / manager.CLAUDE_LOCAL).read_text(encoding="utf-8")

            with self.assertRaisesRegex(
                manager.HookLifecycleError,
                "symlink component",
            ):
                manager.check(root, smoke=True)
            self.assertFalse(smoke_marker.exists())

            with self.assertRaisesRegex(
                manager.HookLifecycleError,
                "symlink component",
            ):
                manager.disable(root)
            self.assertEqual(
                (root / manager.CLAUDE_LOCAL).read_text(encoding="utf-8"),
                local_before,
            )
            self.assertTrue(external_target.is_file())
            self.assertEqual(sentinel.read_text(encoding="utf-8"), "keep\n")

    def test_check_smoke_invokes_every_distinct_configured_target_exactly_once(
        self,
    ) -> None:
        # #168: the smoke loop's own fixtures are all inert "exit 0" scripts
        # with no observable side effect, so a mutation that deletes the
        # entire subprocess-invocation loop leaves every other test green.
        # Spy on subprocess.run (wraps=, so every call still executes for
        # real -- this only records calls, it never fakes a result) rather
        # than giving fixtures a real side effect: writing an observable
        # marker from inside a "sh" script would need an absolute path
        # embedded in the script's own text, which is untested territory on
        # Windows CI (this suite's tempdir paths are backslash-separated
        # there, and no existing test proves such a path survives Git
        # Bash's sh parsing) -- a subprocess-call spy sidesteps that risk
        # entirely while still proving each configured target actually ran.
        #
        # failure-capture is deliberately configured under two events
        # (PostToolUseFailure and PermissionDenied, both real EVENT_TARGETS
        # entries) so this exercises check()'s dict.fromkeys dedup-by-target
        # for real: a per-command count, not a bare set of invoked commands,
        # is required to prove a shared target still runs exactly once
        # rather than once per event that references it (a codex review
        # finding on this PR -- a set comparison alone cannot distinguish
        # "invoked once" from "invoked twice", since both produce the same
        # set of distinct commands).
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            self.create_gitignore(root, upstream_shims=False)
            self.write_json(root, manager.CLAUDE_SHARED, {"hooks": {}})
            local = {
                "hooks": {
                    "UserPromptSubmit": [
                        self.group(
                            self.command_entry(
                                manager.SCRIPT_TARGETS["correction-capture"]
                            )
                        )
                    ],
                    "SessionStart": [
                        self.group(
                            self.command_entry(
                                manager.SCRIPT_TARGETS["evo-analysis-nudge"]
                            )
                        )
                    ],
                    "PostToolUseFailure": [
                        self.group(
                            self.command_entry(manager.SCRIPT_TARGETS["failure-capture"])
                        )
                    ],
                    "PermissionDenied": [
                        self.group(
                            self.command_entry(manager.SCRIPT_TARGETS["failure-capture"])
                        )
                    ],
                }
            }
            self.write_json(root, manager.CLAUDE_LOCAL, local)
            self.create_generated_files(root)

            with mock.patch.object(
                manager.subprocess, "run", wraps=manager.subprocess.run
            ) as spy_run:
                manager.check(root, smoke=True)

            smoke_commands = [
                tuple(call.args[0])
                for call in spy_run.call_args_list
                if call.args and call.args[0] and call.args[0][0] == "sh"
            ]
            self.assertEqual(
                collections.Counter(smoke_commands),
                collections.Counter(
                    [
                        ("sh", manager.SCRIPT_TARGETS["correction-capture"].as_posix()),
                        ("sh", manager.SCRIPT_TARGETS["evo-analysis-nudge"].as_posix()),
                        ("sh", manager.SCRIPT_TARGETS["failure-capture"].as_posix()),
                    ]
                ),
            )

    def test_check_smoke_rejects_a_failing_hook_through_the_cli(self) -> None:
        # #168's second half: a non-zero-exit hook must still make
        # `check --smoke` fail through the real public CLI. This fixture
        # needs no embedded path at all ("exit 3" alone), so unlike the
        # invocation-proof test above, a real script is safe and portable
        # here -- and exercising it through the actual CLI (not
        # manager.check() directly) proves the whole path: subprocess ->
        # HookLifecycleError -> main()'s error handler -> non-zero exit.
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            self.create_gitignore(root, upstream_shims=False)
            self.write_json(root, manager.CLAUDE_SHARED, {"hooks": {}})
            target = manager.SCRIPT_TARGETS["correction-capture"]
            local = {
                "hooks": {"UserPromptSubmit": [self.group(self.command_entry(target))]}
            }
            self.write_json(root, manager.CLAUDE_LOCAL, local)
            self.create_generated_files(root)
            (root / target).write_text("#!/bin/sh\nexit 3\n", encoding="utf-8")

            result = subprocess.run(
                [
                    sys.executable,
                    str(Path(manager.__file__).resolve()),
                    "--root",
                    # #169 follow-up: resolve so this test fails for the
                    # reason it means to (the hook's own exit 3), not
                    # because the tempdir's own OS-level symlink prefix
                    # (e.g. macOS's /var -> /private/var) trips the new
                    # raw-root ancestor check.
                    str(root.resolve()),
                    "check",
                    "--smoke",
                ],
                check=False,
                capture_output=True,
                text=True,
            )

            self.assertNotEqual(result.returncode, 0)
            self.assertRegex(result.stderr, r"hook smoke failed for .* exit 3")

    def test_mounted_config_ancestor_blocks_localize_before_mutation(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            target = manager.SCRIPT_TARGETS["correction-capture"]
            self.write_json(
                root,
                manager.CLAUDE_SHARED,
                {
                    "hooks": {
                        "UserPromptSubmit": [self.group(self.command_entry(target))]
                    }
                },
            )
            self.create_generated_files(root)
            self.create_gitignore(root, upstream_shims=False)
            shared_before = (root / manager.CLAUDE_SHARED).read_bytes()
            target_before = (root / target).read_bytes()
            mounted = root / manager.CLAUDE_SHARED.parent

            with (
                mock.patch.object(
                    manager.os.path,
                    "ismount",
                    side_effect=lambda path: Path(path) == mounted,
                ),
                self.assertRaisesRegex(
                    manager.HookLifecycleError,
                    "managed path crosses a mount boundary",
                ),
            ):
                manager.localize(root)

            self.assertEqual((root / manager.CLAUDE_SHARED).read_bytes(), shared_before)
            self.assertFalse((root / manager.CLAUDE_LOCAL).exists())
            self.assertEqual((root / target).read_bytes(), target_before)

    def test_mounted_hook_ancestor_blocks_disable_before_mutation(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            target = manager.SCRIPT_TARGETS["correction-capture"]
            self.write_json(root, manager.CLAUDE_SHARED, {"hooks": {}})
            self.write_json(
                root,
                manager.CLAUDE_LOCAL,
                {
                    "hooks": {
                        "UserPromptSubmit": [self.group(self.command_entry(target))]
                    }
                },
            )
            self.create_generated_files(root)
            self.create_gitignore(root, upstream_shims=False)
            local_before = (root / manager.CLAUDE_LOCAL).read_bytes()
            target_before = (root / target).read_bytes()
            vendor = root / manager.VENDOR_DIRECTORY / "capture.mjs"
            vendor_before = vendor.read_bytes()
            mounted = root / manager.HOOK_DIRECTORY

            with (
                mock.patch.object(
                    manager.os.path,
                    "ismount",
                    side_effect=lambda path: Path(path) == mounted,
                ),
                self.assertRaisesRegex(
                    manager.HookLifecycleError,
                    "managed path crosses a mount boundary",
                ),
            ):
                manager.disable(root)

            self.assertEqual((root / manager.CLAUDE_LOCAL).read_bytes(), local_before)
            self.assertEqual((root / target).read_bytes(), target_before)
            self.assertEqual(vendor.read_bytes(), vendor_before)

    @unittest.skipUnless(os.name == "nt", "Windows junction regression")
    def test_windows_junctioned_hook_ancestor_blocks_disable(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            workspace = Path(directory)
            root = workspace / "repo"
            root.mkdir()
            external_hooks = workspace / "external-hooks"
            external_scripts = external_hooks / "scripts"
            external_scripts.mkdir(parents=True)
            target = manager.SCRIPT_TARGETS["correction-capture"]
            external_target = external_hooks / target.relative_to(".ievo/hooks")
            external_target.write_text("#!/bin/sh\nexit 0\n", encoding="utf-8")
            vendor = external_scripts / "vendor"
            vendor.mkdir()
            sentinel = vendor / "sentinel"
            sentinel.write_text("keep\n", encoding="utf-8")
            self.write_json(root, manager.CLAUDE_SHARED, {"hooks": {}})
            self.write_json(
                root,
                manager.CLAUDE_LOCAL,
                {
                    "hooks": {
                        "UserPromptSubmit": [self.group(self.command_entry(target))]
                    }
                },
            )
            flag = root / manager.FLAG
            flag.parent.mkdir(parents=True, exist_ok=True)
            flag.write_text(
                "enabled: true\nsignal: corrections-only\n"
                "auto_write_scope: project-wide-only\n",
                encoding="utf-8",
            )
            self.create_gitignore(root, upstream_shims=False)
            junction = root / manager.HOOK_DIRECTORY
            subprocess.run(
                ["cmd", "/c", "mklink", "/J", str(junction), str(external_hooks)],
                check=True,
                capture_output=True,
                text=True,
            )
            local_before = (root / manager.CLAUDE_LOCAL).read_bytes()

            with self.assertRaisesRegex(
                manager.HookLifecycleError,
                "reparse point",
            ):
                manager.disable(root)

            self.assertEqual((root / manager.CLAUDE_LOCAL).read_bytes(), local_before)
            self.assertTrue(external_target.is_file())
            self.assertEqual(sentinel.read_text(encoding="utf-8"), "keep\n")

    @unittest.skipUnless(os.name == "nt", "Windows short-path regression")
    def test_windows_short_path_alias_blocks_disable(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            target = manager.SCRIPT_TARGETS["correction-capture"]
            self.create_generated_files(root)

            def short_path(path: Path) -> Path:
                buffer = ctypes.create_unicode_buffer(32768)
                length = ctypes.windll.kernel32.GetShortPathNameW(
                    str(path), buffer, len(buffer)
                )
                self.assertGreater(length, 0)
                self.assertLess(length, len(buffer))
                return Path(buffer.value)

            short_root = short_path(root)
            short_ievo = short_path(root / ".ievo")
            short_component = short_ievo.relative_to(short_root)
            if "~" not in short_component.as_posix():
                self.skipTest("8.3 short names are unavailable on this volume")
            alias = short_component / target.relative_to(".ievo")
            shared = {
                "hooks": {
                    "FutureEvent": [
                        self.group(
                            {
                                "type": "command",
                                "command": "sh",
                                "args": [alias.as_posix()],
                            }
                        )
                    ]
                }
            }
            self.write_json(root, manager.CLAUDE_SHARED, shared)
            shared_before = (root / manager.CLAUDE_SHARED).read_bytes()

            with self.assertRaisesRegex(
                manager.HookLifecycleError,
                "unsupported iEvo hook reference",
            ):
                manager.disable(root)

            self.assertEqual((root / manager.CLAUDE_SHARED).read_bytes(), shared_before)
            self.assertTrue((root / target).is_file())

    def test_symlinked_config_ancestor_blocks_disable(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            workspace = Path(directory)
            root = workspace / "repo"
            root.mkdir()
            external_claude = workspace / "external-claude"
            external_claude.mkdir()
            (root / ".claude").symlink_to(
                external_claude,
                target_is_directory=True,
            )
            target = manager.SCRIPT_TARGETS["correction-capture"]
            self.write_json(root, manager.CLAUDE_SHARED, {"hooks": {}})
            local = {
                "hooks": {"UserPromptSubmit": [self.group(self.command_entry(target))]}
            }
            self.write_json(root, manager.CLAUDE_LOCAL, local)
            self.create_generated_files(root)
            local_path = external_claude / manager.CLAUDE_LOCAL.name
            local_before = local_path.read_text(encoding="utf-8")

            with self.assertRaisesRegex(
                manager.HookLifecycleError,
                "symlink component",
            ):
                manager.disable(root)

            self.assertEqual(local_path.read_text(encoding="utf-8"), local_before)
            self.assertTrue((root / target).is_file())

    def test_symlinked_root_rejects_direct_lifecycle_call(self) -> None:
        # #169 follow-up: ensure_root_is_a_real_directory's own symlink/
        # reparse-point check on the root argument itself is a separate
        # contract from ensure_cli_root_is_not_redirected's ancestor walk --
        # it is what protects a direct/library caller of
        # localize()/check()/disable() that bypasses the CLI (and therefore
        # never reaches main()'s new check) from being handed a symlinked
        # root directly. A tracked git baseline is required here: without
        # a ".git" marker, lifecycle_lock_path()'s own no-repo fallback
        # calls ensure_lock_directory() on the root argument first and
        # raises its own, different message before this test can reach the
        # function under test; with ".git" present, lifecycle_lock_path()
        # resolves the lock path through the symlink and only
        # ensure_root_is_a_real_directory (via ensure_no_symlink_components)
        # still sees the original, unresolved alias.
        with tempfile.TemporaryDirectory() as directory:
            workspace = Path(directory)
            root = workspace / "repo"
            root.mkdir()
            self.create_gitignore(root, upstream_shims=False)
            target = manager.SCRIPT_TARGETS["correction-capture"]
            self.write_json(root, manager.CLAUDE_SHARED, {"hooks": {}})
            local = {
                "hooks": {"UserPromptSubmit": [self.group(self.command_entry(target))]}
            }
            self.write_json(root, manager.CLAUDE_LOCAL, local)
            self.create_generated_files(root)
            self.commit_tracked_baseline(root)
            local_path = root / manager.CLAUDE_LOCAL
            local_before = local_path.read_text(encoding="utf-8")

            alias = workspace / "repo-alias"
            alias.symlink_to(root, target_is_directory=True)

            with self.assertRaisesRegex(
                manager.HookLifecycleError,
                "managed path root must be a regular directory",
            ):
                manager.disable(alias)

            self.assertEqual(local_path.read_text(encoding="utf-8"), local_before)
            self.assertTrue((root / target).is_file())

    @unittest.skipUnless(os.name != "nt", "POSIX symlink regression")
    def test_symlinked_root_rejects_lifecycle_mutation_through_the_cli(self) -> None:
        # #169: a symlinked --root must be rejected before any lifecycle
        # mutation, not just a symlinked *component* underneath an already
        # -real root (test_symlinked_config_ancestor_blocks_disable covers
        # that separate case). Exercised through the real public CLI
        # (subprocess, not manager.disable() directly), because the bug this
        # regresses lives in main()'s own argument handling, before disable()
        # is ever called. workspace is resolved up front (see run_manager's
        # own comment): this machine's tempdir prefix (e.g. macOS's
        # /var -> /private/var) is itself a symlink ancestor, and
        # ensure_cli_root_is_not_redirected walks ancestors left to right --
        # an unresolved workspace would make this test pass because it hit
        # that prefix, not because of the "repo-alias" leaf this test
        # actually means to exercise. The assertion below pins the flagged
        # path to "alias" specifically so a regression that stops catching
        # leaf symlinks (while some earlier, unrelated ancestor is still
        # caught) cannot pass silently.
        with tempfile.TemporaryDirectory() as directory:
            workspace = Path(directory).resolve()
            root = workspace / "repo"
            root.mkdir()
            self.create_gitignore(root, upstream_shims=False)
            target = manager.SCRIPT_TARGETS["correction-capture"]
            self.write_json(root, manager.CLAUDE_SHARED, {"hooks": {}})
            local = {
                "hooks": {"UserPromptSubmit": [self.group(self.command_entry(target))]}
            }
            self.write_json(root, manager.CLAUDE_LOCAL, local)
            self.create_generated_files(root)
            self.commit_tracked_baseline(root)
            local_path = root / manager.CLAUDE_LOCAL
            local_before = local_path.read_text(encoding="utf-8")

            alias = workspace / "repo-alias"
            alias.symlink_to(root, target_is_directory=True)

            result = subprocess.run(
                [
                    sys.executable,
                    str(Path(manager.__file__).resolve()),
                    "--root",
                    str(alias),
                    "disable",
                ],
                check=False,
                capture_output=True,
                text=True,
            )

            self.assertNotEqual(result.returncode, 0)
            self.assertRegex(
                result.stderr,
                "--root contains a symlink component.*" + re.escape(str(alias)),
            )
            self.assertEqual(local_path.read_text(encoding="utf-8"), local_before)
            self.assertTrue((root / target).is_file())

    @unittest.skipUnless(os.name != "nt", "POSIX symlink regression")
    def test_symlinked_root_ancestor_rejects_lifecycle_mutation_through_the_cli(
        self,
    ) -> None:
        # #169 follow-up (reopened by the repository owner's own post-merge
        # review): a symlink anywhere in --root's ancestor chain must be
        # rejected, not just a symlinked leaf. Here the final "repo"
        # component is a real, non-symlink directory -- only its *parent*
        # is an alias -- matching the owner's own reproduction exactly
        # (alias-parent -> real-parent, --root alias-parent/repo). workspace
        # is resolved up front for the same reason as the sibling leaf-alias
        # test above: this machine's own tempdir prefix is itself a symlink
        # ancestor on macOS, and without resolving first this test would
        # pass because of that unrelated prefix rather than the
        # "alias-parent" component it actually means to exercise.
        with tempfile.TemporaryDirectory() as directory:
            workspace = Path(directory).resolve()
            real_parent = workspace / "real-parent"
            root = real_parent / "repo"
            root.mkdir(parents=True)
            self.create_gitignore(root, upstream_shims=False)
            target = manager.SCRIPT_TARGETS["correction-capture"]
            self.write_json(root, manager.CLAUDE_SHARED, {"hooks": {}})
            local = {
                "hooks": {"UserPromptSubmit": [self.group(self.command_entry(target))]}
            }
            self.write_json(root, manager.CLAUDE_LOCAL, local)
            self.create_generated_files(root)
            self.commit_tracked_baseline(root)
            local_path = root / manager.CLAUDE_LOCAL
            local_before = local_path.read_text(encoding="utf-8")

            alias_parent = workspace / "alias-parent"
            alias_parent.symlink_to(real_parent, target_is_directory=True)

            result = subprocess.run(
                [
                    sys.executable,
                    str(Path(manager.__file__).resolve()),
                    "--root",
                    str(alias_parent / "repo"),
                    "disable",
                ],
                check=False,
                capture_output=True,
                text=True,
            )

            self.assertNotEqual(result.returncode, 0)
            self.assertRegex(
                result.stderr,
                "--root contains a symlink component.*" + re.escape(str(alias_parent)),
            )
            self.assertEqual(local_path.read_text(encoding="utf-8"), local_before)
            self.assertTrue((root / target).is_file())

    def test_mounted_root_rejects_lifecycle_mutation_before_resolution(self) -> None:
        # #169 follow-up: the raw --root argument itself must be rejected
        # if it is a mount point, even though it passes every symlink/
        # reparse check -- ensure_cli_root_is_not_redirected must call
        # os.path.ismount(root) directly, not rely on the descendant-only
        # mount check ensure_no_symlink_components already does for paths
        # *below* an already-accepted root. A real mount can't be created
        # portably in a test, so this patches os.path.ismount to report the
        # temporary directory itself as mounted, exercised directly against
        # the new function (in-process, not through the CLI subprocess --
        # the patch cannot cross a process boundary).
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory).resolve()

            def fake_ismount(path: object) -> bool:
                return Path(path) == root

            with mock.patch.object(
                manager.os.path, "ismount", side_effect=fake_ismount
            ):
                with self.assertRaisesRegex(
                    manager.HookLifecycleError,
                    "--root must be a regular, non-mounted directory",
                ):
                    manager.ensure_cli_root_is_not_redirected(root)

    def test_non_directory_root_rejects_lifecycle_mutation_through_the_cli(
        self,
    ) -> None:
        # #169 follow-up: --root must be rejected when it exists, has no
        # symlink/reparse ancestor, and is not itself a mount point, but is
        # not a directory at all (e.g. a plain file) -- the
        # "not stat.S_ISDIR(...)" operand of
        # ensure_cli_root_is_not_redirected's leaf check, independent of the
        # "or os.path.ismount(...)" operand the mount test above exercises.
        with tempfile.TemporaryDirectory() as directory:
            workspace = Path(directory).resolve()
            not_a_directory = workspace / "not-a-directory"
            not_a_directory.write_text("", encoding="utf-8")

            result = subprocess.run(
                [
                    sys.executable,
                    str(Path(manager.__file__).resolve()),
                    "--root",
                    str(not_a_directory),
                    "disable",
                ],
                check=False,
                capture_output=True,
                text=True,
            )

            self.assertNotEqual(result.returncode, 0)
            self.assertRegex(
                result.stderr, "--root must be a regular, non-mounted directory"
            )

    @unittest.skipUnless(os.name == "nt", "Windows junction regression")
    def test_junctioned_root_rejects_lifecycle_mutation_through_the_cli(self) -> None:
        # #169 follow-up: resolve workspace up front and pin the assertion to
        # the junction itself, matching the POSIX symlink siblings above --
        # an unresolved workspace plus a generic message-shape assertion
        # would let this test pass because of an unrelated ancestor rather
        # than the "repo-alias" junction it actually means to exercise, and
        # this leg only runs in the required native Windows matrix, so that
        # defect class would be invisible to the macOS discovery run.
        with tempfile.TemporaryDirectory() as directory:
            workspace = Path(directory).resolve()
            root = workspace / "repo"
            root.mkdir()
            self.create_gitignore(root, upstream_shims=False)
            target = manager.SCRIPT_TARGETS["correction-capture"]
            self.write_json(root, manager.CLAUDE_SHARED, {"hooks": {}})
            local = {
                "hooks": {"UserPromptSubmit": [self.group(self.command_entry(target))]}
            }
            self.write_json(root, manager.CLAUDE_LOCAL, local)
            self.create_generated_files(root)
            self.commit_tracked_baseline(root)
            local_path = root / manager.CLAUDE_LOCAL
            local_before = local_path.read_text(encoding="utf-8")

            alias = workspace / "repo-alias"
            subprocess.run(
                ["cmd", "/c", "mklink", "/J", str(alias), str(root)],
                check=True,
                capture_output=True,
                text=True,
            )

            result = subprocess.run(
                [
                    sys.executable,
                    str(Path(manager.__file__).resolve()),
                    "--root",
                    str(alias),
                    "disable",
                ],
                check=False,
                capture_output=True,
                text=True,
            )

            self.assertNotEqual(result.returncode, 0)
            self.assertRegex(
                result.stderr,
                "--root contains a symlink component.*" + re.escape(str(alias)),
            )
            self.assertEqual(local_path.read_text(encoding="utf-8"), local_before)
            self.assertTrue((root / target).is_file())

    @unittest.skipUnless(os.name == "nt", "Windows junction regression")
    def test_junctioned_root_ancestor_rejects_lifecycle_mutation_through_the_cli(
        self,
    ) -> None:
        # #169 follow-up: Windows sibling of
        # test_symlinked_root_ancestor_rejects_lifecycle_mutation_through_
        # the_cli above -- an *ancestor* junction, not the leaf itself
        # (test_junctioned_root_rejects_lifecycle_mutation_through_the_cli
        # covers that separate, leaf-only case). Without this, the
        # ancestor-walk branch of ensure_cli_root_is_not_redirected -- the
        # exact code this reopened #169 -- would have no coverage on the
        # required native Windows matrix, only on the macOS discovery run.
        with tempfile.TemporaryDirectory() as directory:
            workspace = Path(directory).resolve()
            real_parent = workspace / "real-parent"
            root = real_parent / "repo"
            root.mkdir(parents=True)
            self.create_gitignore(root, upstream_shims=False)
            target = manager.SCRIPT_TARGETS["correction-capture"]
            self.write_json(root, manager.CLAUDE_SHARED, {"hooks": {}})
            local = {
                "hooks": {"UserPromptSubmit": [self.group(self.command_entry(target))]}
            }
            self.write_json(root, manager.CLAUDE_LOCAL, local)
            self.create_generated_files(root)
            self.commit_tracked_baseline(root)
            local_path = root / manager.CLAUDE_LOCAL
            local_before = local_path.read_text(encoding="utf-8")

            alias_parent = workspace / "alias-parent"
            subprocess.run(
                ["cmd", "/c", "mklink", "/J", str(alias_parent), str(real_parent)],
                check=True,
                capture_output=True,
                text=True,
            )

            result = subprocess.run(
                [
                    sys.executable,
                    str(Path(manager.__file__).resolve()),
                    "--root",
                    str(alias_parent / "repo"),
                    "disable",
                ],
                check=False,
                capture_output=True,
                text=True,
            )

            self.assertNotEqual(result.returncode, 0)
            self.assertRegex(
                result.stderr,
                "--root contains a symlink component.*" + re.escape(str(alias_parent)),
            )
            self.assertEqual(local_path.read_text(encoding="utf-8"), local_before)
            self.assertTrue((root / target).is_file())


if __name__ == "__main__":
    unittest.main()
