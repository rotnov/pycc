#!/usr/bin/env python3
"""Lifecycle tests for the project-local iEvo hook manager."""

from __future__ import annotations

import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

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
        result = subprocess.run(
            [
                sys.executable,
                str(Path(manager.__file__).resolve()),
                "--root",
                str(root),
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
            manager.remove_path(root / manager.SCRIPT_DIRECTORY)

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
            self.assertTrue(manager.flag_enabled(root))
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

    def test_managed_references_scan_nested_vendor_values(self) -> None:
        settings = {
            "hooks": {
                "FutureEvent": {
                    "custom": [
                        "run",
                        manager.VENDOR_DIRECTORY.joinpath("runtime.sh").as_posix(),
                    ]
                }
            }
        }

        self.assertEqual(
            manager.managed_target_references(settings),
            [("FutureEvent", manager.VENDOR_DIRECTORY)],
        )

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
            flag.write_text("enabled: true\n", encoding="utf-8")
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


if __name__ == "__main__":
    unittest.main()
