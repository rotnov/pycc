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
SCRIPT_SUFFIXES = (
    ".sh",
    ".py",
    ".rb",
    ".pl",
    ".php",
    ".ps1",
    ".bat",
    ".cmd",
    ".vbs",
    ".js",
    ".mjs",
    ".cjs",
)
SHELL_INTERPRETERS = ("sh", "bash", "zsh", "dash", "ash", "ksh", "mksh", "fish")
NODE_INTERPRETERS = ("node", "nodejs")
POWERSHELL_INTERPRETERS = ("powershell", "pwsh", "pwsh-preview")
COMMAND_LAUNCHERS = ("command", "env", "exec")
OPAQUE_INLINE_OPTIONS = {
    "-c",
    "-e",
    "-r",
    "--command",
    "--encodedcommand",
    "--eval",
    "--execute",
}
OPAQUE_INLINE_SUBCOMMANDS = {"eval", "repl"}
EMBEDDED_PATH_OPTIONS = {
    "-r",
    "--experimental-loader",
    "--import",
    "--loader",
    "--require",
}
LOADER_URL_PREFIXES = ("data:", "file:", "http:", "https:")
FAIL_SILENT_WRAPPER_CONTRACTS: dict[str, str] = {}
ENV_ASSIGNMENT = re.compile(r"[A-Za-z_][A-Za-z0-9_]*=.*")
WINDOWS_ABSOLUTE_PATH = re.compile(r"^[A-Za-z]:[\\/]")
WINDOWS_ABSOLUTE_IN_COMMAND = re.compile(r"(?:^|[\s\"'])[A-Za-z]:[\\/]")
HOME_RELATIVE_PATH = re.compile(r"^~[^/\\]*[/\\]")
HOME_ENV_PREFIXES = (
    "$home/",
    "$home\\",
    "${home}/",
    "${home}\\",
    "$userprofile/",
    "$userprofile\\",
    "${userprofile}/",
    "${userprofile}\\",
    "$env:userprofile/",
    "$env:userprofile\\",
    "%userprofile%/",
    "%userprofile%\\",
)
HOME_RELATIVE_IN_COMMAND = re.compile(
    r"(?:^|[\s\"'])(?:~[^/\\\s\"']*|"
    r"\$(?:home|userprofile)|\$\{(?:home|userprofile)\}|"
    r"\$env:userprofile|%userprofile%)[/\\]",
    re.IGNORECASE,
)
SHELL_CONTROL = re.compile(r"&&|\|\||[;&|]")


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
                command_for_split = (
                    command.replace("\\", "\\\\")
                    if WINDOWS_ABSOLUTE_IN_COMMAND.search(command)
                    or HOME_RELATIVE_IN_COMMAND.search(command)
                    else command
                )
                try:
                    command_tokens = shlex.split(command_for_split)
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
    name = executable.replace("\\", "/").rsplit("/", 1)[-1].lower()
    name = name.removesuffix(".exe")
    if name in SHELL_INTERPRETERS:
        return "shell"
    if name in {"py", "pyw"} or re.fullmatch(
        r"pythonw?(?:\d+(?:\.\d+)*)?",
        name,
    ):
        return "python"
    if name in NODE_INTERPRETERS:
        return "node"
    if name in POWERSHELL_INTERPRETERS:
        return "powershell"
    if re.fullmatch(r"ruby(?:\d+(?:\.\d+)*)?", name):
        return "ruby"
    return None


def inline_interpreter_mode(kind: str, tokens: list[str]) -> str | None:
    for token in tokens:
        if token == "--":
            return None
        if (
            kind == "shell"
            and token.startswith("-")
            and not token.startswith("--")
            and any(mode in token[1:] for mode in ("c", "s"))
        ):
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
        if kind == "powershell" and token.startswith("-"):
            option = token.lstrip("-").split(":", 1)[0].lower()
            if option and (
                "command".startswith(option) or "encodedcommand".startswith(option)
            ):
                return token
        if kind == "ruby" and token.startswith("-e"):
            return token
    return None


def opaque_inline_mode(executable: str, tokens: list[str]) -> str | None:
    for token in tokens:
        lowered = token.lower()
        option = lowered.split("=", 1)[0].split(":", 1)[0]
        if option in OPAQUE_INLINE_OPTIONS:
            return token
        if lowered in OPAQUE_INLINE_SUBCOMMANDS:
            return token
        if any(
            lowered.startswith(short_option) and len(lowered) > len(short_option)
            for short_option in ("-c", "-e", "-r")
        ):
            return token

    name = executable.replace("\\", "/").rsplit("/", 1)[-1].lower()
    name = name.removesuffix(".exe")
    if name == "perl":
        for token in tokens:
            if (
                token.startswith("-")
                and not token.startswith("--")
                and "e" in token[1:]
            ):
                return token
    if name == "busybox" and tokens and tokens[0] in SHELL_INTERPRETERS:
        for token in tokens[1:]:
            if (
                token.startswith("-")
                and not token.startswith("--")
                and "c" in token[1:]
            ):
                return token
    return None


def loader_candidate_target(candidate: str) -> str | None:
    if candidate.lower().startswith(LOADER_URL_PREFIXES):
        return candidate
    explicit_relative_path = candidate.startswith(("./", "../"))
    normalized, explicit_project_path = normalize_hook_token(candidate)
    if (
        explicit_project_path
        or normalized.startswith(LOCAL_PREFIXES)
        or explicit_relative_path
        or is_home_relative_script_path(normalized)
        or is_absolute_script_path(normalized)
    ):
        return normalized
    return None


def embedded_option_target(kind: str | None, token: str) -> str | None:
    candidate: str | None = None
    if token.startswith("-") and "=" in token:
        option, value = token.split("=", 1)
        if (
            kind == "node"
            and option in EMBEDDED_PATH_OPTIONS
            or kind == "ruby"
            and option == "--require"
        ):
            candidate = value
    elif kind in {"node", "ruby"} and token.startswith("-r") and len(token) > 2:
        candidate = token[2:].removeprefix("=")
    return loader_candidate_target(candidate) if candidate else None


def is_embedded_loader_option(kind: str | None, token: str) -> bool:
    if token.startswith("-") and "=" in token:
        option = token.split("=", 1)[0]
        return (
            kind == "node"
            and option in EMBEDDED_PATH_OPTIONS
            or kind == "ruby"
            and option == "--require"
        )
    return kind in {"node", "ruby"} and token.startswith("-r") and len(token) > 2


def is_separated_loader_option(kind: str | None, token: str) -> bool:
    if kind == "node":
        return token in EMBEDDED_PATH_OPTIONS
    if kind == "ruby":
        return token in {"-r", "--require"}
    return False


def loader_operand_indices(kind: str | None, tokens: list[str]) -> set[int]:
    return {
        index + 1
        for index, token in enumerate(tokens[:-1])
        if is_separated_loader_option(kind, token)
    }


def stdin_interpreter_mode(kind: str, tokens: list[str]) -> str | None:
    loader_operands = loader_operand_indices(kind, tokens)
    after_separator = False
    for index, token in enumerate(tokens):
        if index in loader_operands:
            continue
        if token == "--":
            after_separator = True
            continue
        if after_separator:
            return f"-- {token}" if token.startswith("-") else None
        if token == "-":
            return token
        if token.startswith("-"):
            continue
        return None
    return "<stdin>"


def unsupported_interpreter_option(kind: str, tokens: list[str]) -> str | None:
    loader_operands = loader_operand_indices(kind, tokens)
    after_separator = False
    for index, token in enumerate(tokens):
        if index in loader_operands:
            continue
        if token == "--":
            after_separator = True
            continue
        if after_separator or token == "-":
            return None
        if is_separated_loader_option(kind, token) or is_embedded_loader_option(
            kind,
            token,
        ):
            continue
        if kind == "powershell" and token.lower() == "-file":
            return None
        if token.startswith("-"):
            return token
        return None
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
        if any(SHELL_CONTROL.search(token) for token in tokens):
            continue
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
        if is_home_relative_script_path(executable):
            targets.append(executable)
            continue
        kind = interpreter_kind(executable)
        for token in resolved[1:]:
            embedded_target = embedded_option_target(kind, token)
            if embedded_target is not None:
                targets.append(embedded_target)
        if kind is None and is_absolute_script_path(executable):
            targets.append(executable)
            continue
        if kind is None:
            for token in resolved[1:]:
                normalized, explicit_project_path = normalize_hook_token(token)
                if token.startswith("-"):
                    continue
                if (
                    explicit_project_path
                    or is_relative_script_path(normalized)
                    or is_home_relative_script_path(normalized)
                    or is_absolute_script_path(normalized)
                ):
                    targets.append(normalized)
            continue
        if kind is not None:
            script_tokens = resolved[1:]
            if inline_interpreter_mode(kind, script_tokens):
                continue
            loader_operands = loader_operand_indices(kind, script_tokens)
            for index, token in enumerate(script_tokens[:-1]):
                if not is_separated_loader_option(kind, token):
                    continue
                loader_target = loader_candidate_target(script_tokens[index + 1])
                if loader_target is not None:
                    targets.append(loader_target)
            for index, token in enumerate(script_tokens):
                if index in loader_operands:
                    continue
                normalized, explicit_project_path = normalize_hook_token(token)
                if token.startswith("-"):
                    continue
                if (
                    explicit_project_path
                    or is_relative_script_path(normalized)
                    or is_home_relative_script_path(normalized)
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
        and not is_home_relative_script_path(token)
        and "://" not in token
        and ("/" in token or token.lower().endswith(SCRIPT_SUFFIXES))
    )


def is_absolute_script_path(token: str) -> bool:
    return (
        (token.startswith("/") or WINDOWS_ABSOLUTE_PATH.match(token) is not None)
        and "://" not in token
        and ("/" in token or "\\" in token or token.lower().endswith(SCRIPT_SUFFIXES))
    )


def is_home_relative_script_path(token: str) -> bool:
    normalized = token.lower()
    return (
        (
            HOME_RELATIVE_PATH.match(token) is not None
            or normalized.startswith(HOME_ENV_PREFIXES)
        )
        and "://" not in token
        and ("/" in token or "\\" in token or token.lower().endswith(SCRIPT_SUFFIXES))
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


def validate_hook_targets(
    settings: dict[str, Any],
    tracked: set[str],
    wrapper_contracts: dict[str, str] | None = None,
) -> list[str]:
    failures: list[str] = []
    contracts = (
        FAIL_SILENT_WRAPPER_CONTRACTS
        if wrapper_contracts is None
        else wrapper_contracts
    )
    for command_tokens, argument_tokens in parsed_hook_commands(settings):
        resolved, launcher_error = unwrap_command_launcher(
            [*command_tokens, *argument_tokens]
        )
        if launcher_error is not None:
            failures.append(launcher_error)
            continue
        if not resolved:
            continue
        if any(SHELL_CONTROL.search(token) for token in resolved):
            failures.append(
                "shared hook shell control operators cannot be validated: "
                + " ".join(resolved)
            )
            continue
        executable, _ = normalize_hook_token(resolved[0])
        kind = interpreter_kind(executable)
        tracked_wrapper = is_relative_script_path(executable) and executable in tracked
        script_tokens = resolved[1:]
        missing_loader_operand = kind is not None and any(
            is_separated_loader_option(kind, token) and index + 1 == len(script_tokens)
            for index, token in enumerate(script_tokens)
        )
        mode = (
            None
            if tracked_wrapper
            else (
                inline_interpreter_mode(
                    kind,
                    resolved[1:],
                )
                if kind is not None
                else opaque_inline_mode(executable, resolved[1:])
            )
        )
        if (
            mode is None
            and kind is not None
            and not tracked_wrapper
            and not missing_loader_operand
        ):
            mode = stdin_interpreter_mode(kind, resolved[1:])
        if mode is None and kind is not None and not tracked_wrapper:
            mode = unsupported_interpreter_option(kind, resolved[1:])
        if mode is not None:
            failures.append(
                "shared hook inline interpreter mode cannot be validated: "
                f"{resolved[0]} {mode}"
            )
        if missing_loader_operand:
            failures.append(
                "shared hook loader option is missing its operand: "
                + " ".join(resolved)
            )
    for target in hook_targets(settings):
        if target.lower().startswith(LOADER_URL_PREFIXES):
            failures.append(f"shared hook loader URL cannot be validated: {target}")
        elif is_home_relative_script_path(target):
            failures.append(f"shared hook target must not be home-relative: {target}")
        elif is_absolute_script_path(target):
            failures.append(f"shared hook target must not be absolute: {target}")
        elif target.startswith(".ievo/hooks/"):
            failures.append(f"shared hook target must remain machine-local: {target}")
        elif target not in tracked:
            failures.append(f"shared hook target is not tracked: {target}")
        elif target not in contracts:
            failures.append(
                f"shared hook target lacks a registered fail-silent contract: {target}"
            )
    return failures


def validate_wrapper_contracts(
    tracked: set[str],
    wrapper_contracts: dict[str, str] | None = None,
) -> list[str]:
    contracts = (
        FAIL_SILENT_WRAPPER_CONTRACTS
        if wrapper_contracts is None
        else wrapper_contracts
    )
    failures: list[str] = []
    for wrapper, contract_test in sorted(contracts.items()):
        if wrapper not in tracked:
            failures.append(f"fail-silent wrapper is not tracked: {wrapper}")
        if not contract_test.startswith("scripts/test_") or not contract_test.endswith(
            ".py"
        ):
            failures.append(
                "fail-silent wrapper contract must be a discovered Python test: "
                f"{contract_test}"
            )
        elif contract_test not in tracked:
            failures.append(
                f"fail-silent wrapper contract test is not tracked: {contract_test}"
            )
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
    failures.extend(validate_wrapper_contracts(tracked))
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
