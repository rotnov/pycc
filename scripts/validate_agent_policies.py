#!/usr/bin/env python3
"""Validate clean-clone invariants for committed agent configuration."""

from __future__ import annotations

import json
import re
import shlex
import subprocess
import sys
from pathlib import Path
from typing import Any


ROOT = Path(__file__).resolve().parents[1]
LOCAL_PREFIXES = (".ievo/", ".claude/", ".agents/", ".github/", "scripts/")
PROJECT_PREFIXES = ("$CLAUDE_PROJECT_DIR/", "${CLAUDE_PROJECT_DIR}/")
SCRIPT_SUFFIXES = (".sh", ".py", ".js", ".mjs", ".cjs")
SHELL_INTERPRETERS = ("sh", "bash", "zsh")
NODE_INTERPRETERS = ("node", "nodejs")
COMMAND_LAUNCHERS = ("command", "env", "exec")
ENV_ASSIGNMENT = re.compile(r"[A-Za-z_][A-Za-z0-9_]*=.*")
WINDOWS_ABSOLUTE_PATH = re.compile(r"^[A-Za-z]:[\\/]")


def tracked_files() -> set[str]:
    result = subprocess.run(
        ["git", "ls-files", "-z"],
        cwd=ROOT,
        check=True,
        capture_output=True,
    )
    return {entry.decode("utf-8") for entry in result.stdout.split(b"\0") if entry}


def parsed_hook_commands(
    settings: dict[str, Any],
) -> list[tuple[list[str], list[str]]]:
    commands: list[tuple[list[str], list[str]]] = []
    hooks = settings.get("hooks", {})
    if not isinstance(hooks, dict):
        return commands
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
                command = entry.get("command")
                if not isinstance(command, str):
                    continue
                try:
                    command_tokens = shlex.split(command)
                except ValueError:
                    command_tokens = [command]
                argument_tokens = (
                    [argument for argument in arguments if isinstance(argument, str)]
                    if isinstance(arguments, list)
                    else []
                )
                commands.append((command_tokens, argument_tokens))
    return commands


def interpreter_kind(executable: str) -> str | None:
    name = executable.replace("\\", "/").rsplit("/", 1)[-1]
    if name in SHELL_INTERPRETERS:
        return "shell"
    if re.fullmatch(r"python(?:\d+(?:\.\d+)*)?", name):
        return "python"
    if name in NODE_INTERPRETERS:
        return "node"
    return None


def inline_interpreter_mode(kind: str, tokens: list[str]) -> str | None:
    for token in tokens:
        if token == "--":
            return None
        if kind == "shell" and token.startswith("-") and "c" in token[1:]:
            return token
        if kind == "python" and token.startswith(("-c", "-m")):
            return token
        if kind == "node" and (
            token in {"-e", "--eval", "-p", "--print"}
            or token.startswith(("-e=", "-p="))
            or token.startswith("--eval=")
            or token.startswith("--print=")
        ):
            return token
    return None


def unwrap_command_launcher(tokens: list[str]) -> tuple[list[str], str | None]:
    resolved = list(tokens)
    while resolved:
        executable, _ = normalize_hook_token(resolved[0])
        launcher = executable.replace("\\", "/").rsplit("/", 1)[-1]
        if launcher not in COMMAND_LAUNCHERS:
            return resolved, None

        original = resolved.pop(0)
        if resolved and resolved[0] == "--":
            resolved.pop(0)
        if launcher == "env":
            while resolved and ENV_ASSIGNMENT.fullmatch(resolved[0]):
                resolved.pop(0)
        if not resolved or resolved[0].startswith("-"):
            return [], f"shared hook command launcher cannot be validated: {original}"
    return [], "shared hook command launcher has no executable target"


def hook_targets(settings: dict[str, Any]) -> list[str]:
    targets: list[str] = []
    for command_tokens, argument_tokens in parsed_hook_commands(settings):
        tokens = [*command_tokens, *argument_tokens]
        for token in tokens:
            normalized, explicit_project_path = normalize_hook_token(token)
            if explicit_project_path or normalized.startswith(LOCAL_PREFIXES):
                targets.append(normalized)

        resolved, error = unwrap_command_launcher(tokens)
        if error is not None or not resolved:
            continue
        executable, _ = normalize_hook_token(resolved[0])
        if is_relative_script_path(executable):
            targets.append(executable)
            continue
        kind = interpreter_kind(executable)
        if kind is None and is_absolute_script_path(executable):
            targets.append(executable)
            continue
        if kind is not None:
            script_tokens = resolved[1:]
            if inline_interpreter_mode(kind, script_tokens):
                continue
            for token in script_tokens:
                normalized, explicit_project_path = normalize_hook_token(token)
                if token.startswith("-"):
                    continue
                if (
                    explicit_project_path
                    or is_relative_script_path(normalized)
                    or is_absolute_script_path(normalized)
                    or normalized == token
                ):
                    targets.append(normalized)
                    break
    return list(dict.fromkeys(targets))


def normalize_hook_token(token: str) -> tuple[str, bool]:
    if token.startswith("./"):
        return token.removeprefix("./"), True
    normalized = token
    for project_prefix in PROJECT_PREFIXES:
        if normalized.startswith(project_prefix):
            return normalized.removeprefix(project_prefix), True
    return normalized, False


def is_relative_script_path(token: str) -> bool:
    return (
        not token.startswith(("/", "~"))
        and "://" not in token
        and ("/" in token or token.endswith(SCRIPT_SUFFIXES))
    )


def is_absolute_script_path(token: str) -> bool:
    return (
        (token.startswith("/") or WINDOWS_ABSOLUTE_PATH.match(token) is not None)
        and "://" not in token
        and ("/" in token or "\\" in token or token.endswith(SCRIPT_SUFFIXES))
    )


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
                command = entry.get("command")
                if not isinstance(command, str):
                    failures.append(f"{location}.command must be a string")
                else:
                    try:
                        shlex.split(command)
                    except ValueError as error:
                        failures.append(
                            f"{location}.command is not valid shell syntax: {error}"
                        )
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
    failures: list[str] = []
    for command_tokens, argument_tokens in parsed_hook_commands(settings):
        resolved, launcher_error = unwrap_command_launcher(
            [*command_tokens, *argument_tokens]
        )
        if launcher_error is not None:
            failures.append(launcher_error)
            continue
        if not resolved:
            continue
        executable, _ = normalize_hook_token(resolved[0])
        kind = interpreter_kind(executable)
        mode = (
            inline_interpreter_mode(
                kind,
                resolved[1:],
            )
            if kind is not None
            else None
        )
        if mode is not None:
            failures.append(
                "shared hook inline interpreter mode cannot be validated: "
                f"{resolved[0]} {mode}"
            )
    for target in hook_targets(settings):
        if is_absolute_script_path(target):
            failures.append(f"shared hook target must not be absolute: {target}")
        elif target.startswith(".ievo/hooks/"):
            failures.append(f"shared hook target must remain machine-local: {target}")
        elif target not in tracked:
            failures.append(f"shared hook target is not tracked: {target}")
    return failures


def validate_machine_local_files(tracked: set[str]) -> list[str]:
    return [
        f"machine-local iEvo hook must not be tracked: {target}"
        for target in sorted(tracked)
        if target.startswith(".ievo/hooks/")
    ]


def main() -> int:
    failures: list[str] = []
    settings = json.loads(
        (ROOT / ".claude" / "settings.json").read_text(encoding="utf-8")
    )
    tracked = tracked_files()
    failures.extend(validate_hook_schema(settings))
    failures.extend(validate_hook_targets(settings, tracked))
    failures.extend(validate_machine_local_files(tracked))

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
