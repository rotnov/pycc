#!/usr/bin/env python3
"""Validate clean-clone invariants for committed agent configuration."""

from __future__ import annotations

import json
import shlex
import subprocess
import sys
from pathlib import Path
from typing import Any


ROOT = Path(__file__).resolve().parents[1]
LOCAL_PREFIXES = (".ievo/", ".claude/", ".agents/", ".github/", "scripts/")
PROJECT_PREFIXES = ("$CLAUDE_PROJECT_DIR/", "${CLAUDE_PROJECT_DIR}/")


def tracked_files() -> set[str]:
    result = subprocess.run(
        ["git", "ls-files", "-z"],
        cwd=ROOT,
        check=True,
        capture_output=True,
    )
    return {entry.decode("utf-8") for entry in result.stdout.split(b"\0") if entry}


def hook_targets(settings: dict[str, Any]) -> list[str]:
    targets: list[str] = []
    hooks = settings.get("hooks", {})
    if not isinstance(hooks, dict):
        return targets
    for groups in hooks.values():
        if not isinstance(groups, list):
            continue
        for group in groups:
            if not isinstance(group, dict):
                continue
            entries = group.get("hooks", [])
            if not isinstance(entries, list):
                continue
            for entry in entries:
                if not isinstance(entry, dict):
                    continue
                arguments = entry.get("args", [])
                values = [entry.get("command")]
                if isinstance(arguments, list):
                    values.extend(arguments)
                for value in values:
                    if not isinstance(value, str):
                        continue
                    try:
                        tokens = shlex.split(value)
                    except ValueError:
                        tokens = [value]
                    for token in tokens:
                        normalized = token.removeprefix("./")
                        for project_prefix in PROJECT_PREFIXES:
                            normalized = normalized.removeprefix(project_prefix)
                        if normalized.startswith(LOCAL_PREFIXES):
                            targets.append(normalized)
    return targets


def validate_hook_schema(settings: dict[str, Any]) -> list[str]:
    failures: list[str] = []
    hooks = settings.get("hooks", {})
    if not isinstance(hooks, dict):
        return ["shared hooks must be a JSON object"]
    for event, groups in hooks.items():
        if not isinstance(groups, list):
            failures.append(f"hooks.{event} must be a list")
            continue
        for group_index, group in enumerate(groups):
            if not isinstance(group, dict):
                failures.append(f"hooks.{event}[{group_index}] must be an object")
                continue
            entries = group.get("hooks")
            if not isinstance(entries, list):
                failures.append(f"hooks.{event}[{group_index}].hooks must be a list")
                continue
            for entry_index, entry in enumerate(entries):
                location = f"hooks.{event}[{group_index}].hooks[{entry_index}]"
                if not isinstance(entry, dict):
                    failures.append(f"{location} must be an object")
                    continue
                if not isinstance(entry.get("command"), str):
                    failures.append(f"{location}.command must be a string")
                arguments = entry.get("args", [])
                if not isinstance(arguments, list) or not all(
                    isinstance(argument, str) for argument in arguments
                ):
                    failures.append(f"{location}.args must be a list of strings")
    return failures


def parse_flag(contents: str) -> dict[str, str]:
    result: dict[str, str] = {}
    for line in contents.splitlines():
        key, separator, value = line.partition(":")
        if separator:
            result[key.strip()] = value.strip()
    return result


def validate_hook_targets(settings: dict[str, Any], tracked: set[str]) -> list[str]:
    return [
        f"shared hook target is not tracked: {target}"
        for target in hook_targets(settings)
        if target not in tracked
    ]


def main() -> int:
    failures: list[str] = []
    settings = json.loads(
        (ROOT / ".claude" / "settings.json").read_text(encoding="utf-8")
    )
    tracked = tracked_files()
    failures.extend(validate_hook_schema(settings))
    failures.extend(validate_hook_targets(settings, tracked))

    ignore_check = subprocess.run(
        ["git", "check-ignore", "--quiet", ".claude/settings.local.json"],
        cwd=ROOT,
        check=False,
    )
    if ignore_check.returncode != 0:
        failures.append(".claude/settings.local.json must remain ignored")

    flag = parse_flag((ROOT / ".ievo" / "evo-auto.flag").read_text(encoding="utf-8"))
    if flag.get("enabled") != "true":
        failures.append(".ievo/evo-auto.flag must state the shared enabled intent")
    if flag.get("signal") != "corrections-only":
        failures.append("auto-evolution signal must remain corrections-only")
    if flag.get("auto_write_scope") != "project-wide-only":
        failures.append("auto-write scope must remain project-wide-only")

    if failures:
        for failure in failures:
            print(f"error: {failure}", file=sys.stderr)
        return 1
    print("agent policies: valid")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
