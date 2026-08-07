#!/usr/bin/env python3
"""Tests for the local ievo@ievo-skills structural-presence + freshness check."""

from __future__ import annotations

import json
import tempfile
import unittest
from pathlib import Path
from unittest import mock

import check_claude_reviewer_binding as binder

PROJECT_ROOT = "/repo/pycc-proto"


class CheckClaudeReviewerBindingTests(unittest.TestCase):
    def make_install(
        self,
        config_dir: Path,
        *,
        version: str | None = "0.78.8",
        omit_skill: bool = False,
        omit_agent: bool = False,
        omit_manifest: bool = False,
        empty_skill: bool = False,
    ) -> Path:
        install_path = config_dir / "plugins" / "cache" / "ievo-skills" / "ievo" / "0.78.8"
        install_path.mkdir(parents=True)
        if not omit_manifest:
            manifest_dir = install_path / ".claude-plugin"
            manifest_dir.mkdir(parents=True)
            manifest = {"name": "ievo"}
            if version is not None:
                manifest["version"] = version
            (manifest_dir / "plugin.json").write_text(
                json.dumps(manifest), encoding="utf-8"
            )
        if not omit_skill:
            skill_dir = install_path / "skills" / "deep-review"
            skill_dir.mkdir(parents=True)
            (skill_dir / "SKILL.md").write_text(
                "" if empty_skill else "# deep-review\n", encoding="utf-8"
            )
        if not omit_agent:
            agents_dir = install_path / "agents"
            agents_dir.mkdir(parents=True)
            (agents_dir / "deep-reviewer.md").write_text(
                "# deep-reviewer\n", encoding="utf-8"
            )
        return install_path

    def write_installed_plugins(
        self, config_dir: Path, entries: list[dict]
    ) -> None:
        plugins_dir = config_dir / "plugins"
        plugins_dir.mkdir(parents=True, exist_ok=True)
        (plugins_dir / "installed_plugins.json").write_text(
            json.dumps({"version": 2, "plugins": {binder.PLUGIN_KEY: entries}}),
            encoding="utf-8",
        )

    def project_entry(self, install_path: Path, project_root: str = PROJECT_ROOT) -> dict:
        return {
            "scope": "project",
            "projectPath": project_root,
            "installPath": str(install_path),
        }

    def user_entry(self, install_path: Path) -> dict:
        return {"scope": "user", "installPath": str(install_path)}

    def test_no_entry_fails_closed(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            config_dir = Path(directory)
            self.write_installed_plugins(config_dir, [])
            with self.assertRaises(binder.BindingError) as ctx:
                binder.check_binding(config_dir, PROJECT_ROOT)
            self.assertIn("NOT FOUND", str(ctx.exception))

    def test_missing_installed_plugins_file_fails_closed(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            config_dir = Path(directory)
            with self.assertRaises(binder.BindingError):
                binder.check_binding(config_dir, PROJECT_ROOT)

    def test_installed_plugins_file_without_plugins_wrapper_fails_closed(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            config_dir = Path(directory)
            plugins_dir = config_dir / "plugins"
            plugins_dir.mkdir(parents=True)
            (plugins_dir / "installed_plugins.json").write_text(
                json.dumps({"version": 2}), encoding="utf-8"
            )
            with self.assertRaises(binder.BindingError) as ctx:
                binder.check_binding(config_dir, PROJECT_ROOT)
            self.assertIn("NOT FOUND", str(ctx.exception))

    def test_entry_missing_install_path_fails_closed_as_not_found(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            config_dir = Path(directory)
            entry = self.project_entry(Path("/unused"))
            del entry["installPath"]
            self.write_installed_plugins(config_dir, [entry])
            with self.assertRaises(binder.BindingError) as ctx:
                binder.check_binding(config_dir, PROJECT_ROOT)
            self.assertIn("NOT FOUND", str(ctx.exception))

    def test_entry_empty_install_path_fails_closed_as_not_found(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            config_dir = Path(directory)
            entry = self.project_entry(Path("/unused"))
            entry["installPath"] = ""
            self.write_installed_plugins(config_dir, [entry])
            with self.assertRaises(binder.BindingError) as ctx:
                binder.check_binding(config_dir, PROJECT_ROOT)
            self.assertIn("NOT FOUND", str(ctx.exception))

    def test_structural_files_missing_fails_closed(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            config_dir = Path(directory)
            install_path = self.make_install(config_dir, omit_skill=True)
            self.write_installed_plugins(
                config_dir, [self.project_entry(install_path)]
            )
            with self.assertRaises(binder.BindingError) as ctx:
                binder.check_binding(config_dir, PROJECT_ROOT)
            self.assertIn("structurally incomplete", str(ctx.exception))

    def test_structural_file_empty_fails_closed(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            config_dir = Path(directory)
            install_path = self.make_install(config_dir, empty_skill=True)
            self.write_installed_plugins(
                config_dir, [self.project_entry(install_path)]
            )
            with self.assertRaises(binder.BindingError) as ctx:
                binder.check_binding(config_dir, PROJECT_ROOT)
            self.assertIn("structurally incomplete", str(ctx.exception))

    def test_missing_plugin_manifest_fails_closed(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            config_dir = Path(directory)
            install_path = self.make_install(config_dir, omit_manifest=True)
            self.write_installed_plugins(
                config_dir, [self.project_entry(install_path)]
            )
            with self.assertRaises(binder.BindingError) as ctx:
                binder.check_binding(config_dir, PROJECT_ROOT)
            self.assertIn("structurally incomplete", str(ctx.exception))

    def test_known_version_no_update_available(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            config_dir = Path(directory)
            install_path = self.make_install(config_dir, version="0.78.8")
            self.write_installed_plugins(
                config_dir, [self.project_entry(install_path)]
            )
            with mock.patch.object(
                binder, "latest_upstream_tag", return_value="0.78.8"
            ):
                message = binder.check_binding(config_dir, PROJECT_ROOT)
            self.assertEqual(
                message, "ievo@ievo-skills 0.78.8 OK (latest 0.78.8)"
            )

    def test_known_version_update_available(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            config_dir = Path(directory)
            install_path = self.make_install(config_dir, version="0.58.1")
            self.write_installed_plugins(
                config_dir, [self.project_entry(install_path)]
            )
            with mock.patch.object(
                binder, "latest_upstream_tag", return_value="0.78.8"
            ):
                message = binder.check_binding(config_dir, PROJECT_ROOT)
            self.assertEqual(
                message,
                "ievo@ievo-skills 0.58.1 OK, 0.78.8 available — consider updating",
            )

    def test_missing_version_field_is_advisory_only(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            config_dir = Path(directory)
            install_path = self.make_install(config_dir, version=None)
            self.write_installed_plugins(
                config_dir, [self.project_entry(install_path)]
            )
            with mock.patch.object(
                binder, "latest_upstream_tag"
            ) as latest_upstream_tag:
                message = binder.check_binding(config_dir, PROJECT_ROOT)
            latest_upstream_tag.assert_not_called()
            self.assertEqual(message, "ievo@ievo-skills OK (version unknown)")

    def test_malformed_version_field_is_advisory_only(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            config_dir = Path(directory)
            install_path = self.make_install(config_dir, version="not-a-version")
            self.write_installed_plugins(
                config_dir, [self.project_entry(install_path)]
            )
            message = binder.check_binding(config_dir, PROJECT_ROOT)
            self.assertEqual(message, "ievo@ievo-skills OK (version unknown)")

    def test_network_failure_degrades_to_freshness_unknown(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            config_dir = Path(directory)
            install_path = self.make_install(config_dir, version="0.78.8")
            self.write_installed_plugins(
                config_dir, [self.project_entry(install_path)]
            )
            with mock.patch.object(
                binder, "latest_upstream_tag", return_value=None
            ):
                message = binder.check_binding(config_dir, PROJECT_ROOT)
            self.assertEqual(
                message,
                "ievo@ievo-skills 0.78.8 OK "
                "(freshness unknown: could not reach ievo-ai/skills)",
            )

    def test_project_scope_wins_over_unrelated_user_scope(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            config_dir = Path(directory)
            project_install = self.make_install(config_dir, version="0.78.8")
            user_install_root = config_dir / "plugins" / "cache" / "ievo-skills" / "ievo" / "0.50.0"
            user_install_root.mkdir(parents=True)
            manifest_dir = user_install_root / ".claude-plugin"
            manifest_dir.mkdir()
            (manifest_dir / "plugin.json").write_text(
                json.dumps({"version": "0.50.0"}), encoding="utf-8"
            )
            (user_install_root / "skills" / "deep-review").mkdir(parents=True)
            (user_install_root / "skills" / "deep-review" / "SKILL.md").write_text(
                "# deep-review\n", encoding="utf-8"
            )
            (user_install_root / "agents").mkdir()
            (user_install_root / "agents" / "deep-reviewer.md").write_text(
                "# deep-reviewer\n", encoding="utf-8"
            )
            self.write_installed_plugins(
                config_dir,
                [
                    self.user_entry(user_install_root),
                    self.project_entry(project_install),
                ],
            )
            with mock.patch.object(
                binder, "latest_upstream_tag", return_value="0.78.8"
            ):
                message = binder.check_binding(config_dir, PROJECT_ROOT)
            self.assertEqual(
                message, "ievo@ievo-skills 0.78.8 OK (latest 0.78.8)"
            )

    def test_falls_back_to_user_scope_when_no_project_entry_matches(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            config_dir = Path(directory)
            install_path = self.make_install(config_dir, version="0.78.8")
            self.write_installed_plugins(
                config_dir,
                [
                    self.project_entry(install_path, project_root="/somewhere/else"),
                    self.user_entry(install_path),
                ],
            )
            with mock.patch.object(
                binder, "latest_upstream_tag", return_value="0.78.8"
            ):
                message = binder.check_binding(config_dir, PROJECT_ROOT)
            self.assertEqual(
                message, "ievo@ievo-skills 0.78.8 OK (latest 0.78.8)"
            )

    def test_main_exits_nonzero_and_prints_to_stderr_when_not_found(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            config_dir = Path(directory)
            self.write_installed_plugins(config_dir, [])
            with (
                mock.patch.object(binder, "claude_config_dir", return_value=config_dir),
                mock.patch.object(binder, "resolve_project_root", return_value=PROJECT_ROOT),
            ):
                exit_code = binder.main()
            self.assertEqual(exit_code, 1)

    def test_main_exits_zero_when_a_qualifying_install_is_found(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            config_dir = Path(directory)
            install_path = self.make_install(config_dir, version="0.78.8")
            self.write_installed_plugins(
                config_dir, [self.project_entry(install_path)]
            )
            with (
                mock.patch.object(binder, "claude_config_dir", return_value=config_dir),
                mock.patch.object(binder, "resolve_project_root", return_value=PROJECT_ROOT),
                mock.patch.object(binder, "latest_upstream_tag", return_value="0.78.8"),
            ):
                exit_code = binder.main()
            self.assertEqual(exit_code, 0)

    def test_claude_config_dir_respects_env_override(self) -> None:
        with mock.patch.dict("os.environ", {"CLAUDE_CONFIG_DIR": "/tmp/example"}):
            self.assertEqual(binder.claude_config_dir(), Path("/tmp/example"))

    def test_claude_config_dir_defaults_to_home(self) -> None:
        with mock.patch.dict("os.environ", {}, clear=True):
            self.assertEqual(binder.claude_config_dir(), Path.home() / ".claude")

    def test_resolve_project_root_handles_non_git_directory(self) -> None:
        with mock.patch("subprocess.run", side_effect=FileNotFoundError()):
            self.assertIsNone(binder.resolve_project_root())


if __name__ == "__main__":
    unittest.main()
