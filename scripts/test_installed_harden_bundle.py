#!/usr/bin/env python3
"""Regression tests for the repository-installed harden bundle."""

from __future__ import annotations

import contextlib
import importlib.util
import io
import os
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock


ROOT = Path(__file__).resolve().parents[1]
UNPACK_SCRIPT = ROOT / ".claude/skills/harden/scripts/unpack-skills.py"
SESSION_SCRIPTS = (
    ROOT / ".claude/skills/sessions/scripts/session-snapshot.py",
    ROOT / ".claude/skills/harden/skills/sessions/scripts/session-snapshot.py",
)


def load_script(name: str, path: Path):
    spec = importlib.util.spec_from_file_location(name, path)
    if spec is None or spec.loader is None:
        raise RuntimeError(f"could not load {path}")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


class InstalledHardenBundleTests(unittest.TestCase):
    def test_unpack_preserves_project_owned_symlink_without_force(self) -> None:
        unpack = load_script("installed_harden_unpack", UNPACK_SCRIPT)

        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            bundled = root / "bundled"
            source = bundled / "sample"
            source.mkdir(parents=True)
            (source / "SKILL.md").write_text("bundle\n", encoding="utf-8")

            project_owned = root / "project-owned"
            project_owned.mkdir()
            (project_owned / "SKILL.md").write_text("project\n", encoding="utf-8")
            destination = root / "sample"
            destination.symlink_to(project_owned, target_is_directory=True)

            with mock.patch.object(unpack, "BUNDLED", bundled):
                result = unpack.place("sample", destination, force=False)

            self.assertIsNotNone(result)
            self.assertIn("kept", result or "")
            self.assertTrue(destination.is_symlink())
            self.assertEqual(
                (project_owned / "SKILL.md").read_text(encoding="utf-8"),
                "project\n",
            )

    def test_snapshot_relative_output_is_rooted_in_requested_repository(self) -> None:
        for index, script in enumerate(SESSION_SCRIPTS):
            with self.subTest(script=script):
                snapshot = load_script(f"installed_session_snapshot_{index}", script)
                with tempfile.TemporaryDirectory() as directory:
                    root = Path(directory)
                    repository = root / "repository"
                    caller = root / "caller"
                    repository.mkdir()
                    caller.mkdir()

                    arguments = [
                        str(script),
                        "--root",
                        str(repository),
                        "--slug",
                        "rooted-output",
                    ]
                    with (
                        contextlib.chdir(caller),
                        mock.patch.object(sys, "argv", arguments),
                        contextlib.redirect_stdout(io.StringIO()),
                    ):
                        self.assertEqual(snapshot.main(), 0)

                    expected = list(
                        (repository / "docs/sessions").glob("*-rooted-output.md")
                    )
                    self.assertEqual(len(expected), 1)
                    self.assertFalse((caller / "docs/sessions").exists())

    def test_snapshot_rejects_a_missing_repository_root(self) -> None:
        for index, script in enumerate(SESSION_SCRIPTS):
            with self.subTest(script=script):
                snapshot = load_script(f"missing_root_session_snapshot_{index}", script)
                with tempfile.TemporaryDirectory() as directory:
                    missing = Path(directory) / "missing"
                    arguments = [str(script), "--root", str(missing)]
                    with (
                        mock.patch.object(sys, "argv", arguments),
                        contextlib.redirect_stdout(io.StringIO()),
                    ):
                        self.assertEqual(snapshot.main(), 2)

                    self.assertFalse(missing.exists())


if __name__ == "__main__":
    unittest.main()
