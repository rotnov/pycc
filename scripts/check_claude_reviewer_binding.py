#!/usr/bin/env python3
"""Locally verify a structurally intact ievo@ievo-skills install.

D-155 stopped exact-pinning the Claude-side iEvo reviewer to a specific commit:
Claude Code's plugin marketplace registry is a single global, name-keyed file
(``~/.claude/plugins/known_marketplaces.json``) that no repository pull request
can control, so an exact ``sha`` recorded in this repository's own
``.claude/settings.json`` was never actually enforced on a real, non-isolated
machine -- whichever project or manual ``/plugin marketplace add`` last
registered the ``ievo-skills`` name unpinned governs resolution for every
project referencing that name, this repository's own declared pin included.

This script is the local, non-CI half of D-155's contract (the CI-safe half,
which only ever sees a clean install, is ``scripts/check-claude-marketplace.sh``):
it hard-fails only when no structurally intact ``ievo@ievo-skills`` install can
be found at all, and otherwise prints an advisory freshness note that never
blocks dispatch of the local review loop.
"""

from __future__ import annotations

import json
import os
import re
import subprocess
import sys
from pathlib import Path

SEMVER = re.compile(r"^\d+\.\d+\.\d+")
PLUGIN_KEY = "ievo@ievo-skills"
REQUIRED_ARTIFACTS = (
    ".claude-plugin/plugin.json",
    "skills/deep-review/SKILL.md",
    "agents/deep-reviewer.md",
)
IEVO_REMOTE = "https://github.com/ievo-ai/skills.git"


class BindingError(Exception):
    """Raised when no structurally intact ievo@ievo-skills install can be found."""


def claude_config_dir() -> Path:
    configured = os.environ.get("CLAUDE_CONFIG_DIR")
    if configured:
        return Path(configured).expanduser()
    return Path.home() / ".claude"


def load_installed_plugins(config_dir: Path) -> dict:
    path = config_dir / "plugins" / "installed_plugins.json"
    if not path.is_file():
        return {}
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError):
        return {}


def resolve_project_root() -> str | None:
    try:
        result = subprocess.run(
            ["git", "rev-parse", "--show-toplevel"],
            capture_output=True,
            text=True,
            check=True,
        )
    except (OSError, subprocess.CalledProcessError):
        return None
    root = result.stdout.strip()
    return root or None


def select_install(entries: list, project_root: str | None) -> dict | None:
    if project_root:
        for entry in entries:
            if (
                isinstance(entry, dict)
                and entry.get("scope") == "project"
                and entry.get("projectPath") == project_root
            ):
                return entry
    for entry in entries:
        if isinstance(entry, dict) and entry.get("scope") == "user":
            return entry
    return None


def check_structural_presence(install_path: Path) -> list[str]:
    problems = []
    for relative in REQUIRED_ARTIFACTS:
        artifact = install_path / relative
        if not artifact.is_file() or artifact.stat().st_size == 0:
            problems.append(f"{relative} is missing or empty")
    return problems


def read_version(install_path: Path) -> str | None:
    manifest = install_path / ".claude-plugin" / "plugin.json"
    try:
        data = json.loads(manifest.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError):
        return None
    version = data.get("version")
    if isinstance(version, str) and SEMVER.match(version):
        return version
    return None


def semver_tuple(version: str) -> tuple[int, ...]:
    match = SEMVER.match(version)
    assert match is not None
    return tuple(int(part) for part in match.group(0).split("."))


def latest_upstream_tag(remote: str = IEVO_REMOTE) -> str | None:
    try:
        result = subprocess.run(
            ["git", "ls-remote", "--tags", remote],
            capture_output=True,
            text=True,
            check=True,
            timeout=10,
        )
    except (OSError, subprocess.CalledProcessError, subprocess.TimeoutExpired):
        return None

    best: tuple[int, ...] | None = None
    best_tag: str | None = None
    for line in result.stdout.splitlines():
        if "^{}" in line:
            continue
        _, _, ref = line.partition("refs/tags/")
        if not ref:
            continue
        candidate = ref[1:] if ref.startswith("v") else ref
        if not SEMVER.match(candidate):
            continue
        parts = semver_tuple(candidate)
        if best is None or parts > best:
            best = parts
            best_tag = candidate
    return best_tag


def check_binding(config_dir: Path, project_root: str | None) -> str:
    installed = load_installed_plugins(config_dir)
    plugins = installed.get("plugins", {})
    entries = plugins.get(PLUGIN_KEY) if isinstance(plugins, dict) else None
    if not isinstance(entries, list) or not entries:
        raise BindingError(f"{PLUGIN_KEY}: NOT FOUND")

    install = select_install(entries, project_root)
    if install is None:
        raise BindingError(f"{PLUGIN_KEY}: NOT FOUND")

    raw_install_path = install.get("installPath")
    if not raw_install_path:
        raise BindingError(f"{PLUGIN_KEY}: NOT FOUND")
    install_path = Path(raw_install_path)
    if not install_path.is_dir():
        raise BindingError(f"{PLUGIN_KEY}: NOT FOUND")

    problems = check_structural_presence(install_path)
    if problems:
        raise BindingError(
            f"{PLUGIN_KEY}: structurally incomplete install ({'; '.join(problems)})"
        )

    version = read_version(install_path)
    if version is None:
        return f"{PLUGIN_KEY} OK (version unknown)"

    latest = latest_upstream_tag()
    if latest is None:
        return f"{PLUGIN_KEY} {version} OK (freshness unknown: could not reach ievo-ai/skills)"
    if semver_tuple(latest) > semver_tuple(version):
        return f"{PLUGIN_KEY} {version} OK, {latest} available — consider updating"
    return f"{PLUGIN_KEY} {version} OK (latest {latest})"


def main() -> int:
    try:
        message = check_binding(claude_config_dir(), resolve_project_root())
    except BindingError as error:
        print(str(error), file=sys.stderr)
        return 1
    print(message)
    return 0


if __name__ == "__main__":
    sys.exit(main())
