#!/usr/bin/env python3
"""Validate clean-clone invariants for committed agent configuration."""

from __future__ import annotations

import json
import re
import shlex
import subprocess
import sys
from collections.abc import Collection, Mapping
from pathlib import Path
from typing import Any


ROOT = Path(__file__).resolve().parents[1]
LOCAL_PREFIXES = (".ievo/", ".claude/", ".agents/", ".github/", "scripts/")
PROJECT_PREFIXES = (
    "$CLAUDE_PROJECT_DIR/",
    "$CLAUDE_PROJECT_DIR\\",
    "${CLAUDE_PROJECT_DIR}/",
    "${CLAUDE_PROJECT_DIR}\\",
)
EXEC_PROJECT_PREFIXES = (
    "${CLAUDE_PROJECT_DIR}/",
    "${CLAUDE_PROJECT_DIR}\\",
)
SCRIPT_SUFFIXES = (
    ".sh",
    ".bat",
    ".cmd",
    ".py",
    ".rb",
    ".pl",
    ".php",
    ".ps1",
    ".js",
    ".mjs",
    ".cjs",
    ".ts",
    ".mts",
    ".cts",
    ".vbs",
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
TRUSTED_POSIX_EXECUTABLE_PREFIXES = (
    "/bin/",
    "/usr/bin/",
)
TRUSTED_WINDOWS_EXECUTABLE_PREFIXES = (
    "c:/windows/system32/",
    "c:/program files/powershell/",
)
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
SHELL_CONTROL = re.compile(r"&&|\|\||[;&|\r\n]")
UNSAFE_RAW_SHELL_SYNTAX = re.compile(r"[\r\n`()<>]")
RAW_SHELL_EXPANSION = re.compile(r"\$|%[A-Za-z_][A-Za-z0-9_]*%|[*?\[{]")
PROJECT_VARIABLE = r"\$(?:CLAUDE_PROJECT_DIR|\{CLAUDE_PROJECT_DIR\})"
QUOTED_PROJECT_PREFIX = re.compile(
    rf'(?P<boundary>^|[\s;&|])"{PROJECT_VARIABLE}'
    rf'(?=(?:[/\\][^"\r\n]*")|(?:"[/\\]))'
)


def has_unsafe_raw_shell_syntax(command: str) -> bool:
    without_project_prefix = QUOTED_PROJECT_PREFIX.sub(
        r'\g<boundary>"PROJECT_ROOT',
        command,
    )
    return (
        UNSAFE_RAW_SHELL_SYNTAX.search(command) is not None
        or RAW_SHELL_EXPANSION.search(without_project_prefix) is not None
    )


def tracked_files() -> dict[str, str]:
    result = subprocess.run(
        ["git", "ls-files", "-s", "-z"],
        cwd=ROOT,
        check=True,
        capture_output=True,
    )
    tracked: dict[str, str] = {}
    for entry in result.stdout.split(b"\0"):
        if not entry:
            continue
        header, separator, encoded_path = entry.partition(b"\t")
        fields = header.split()
        if not separator or len(fields) != 3:
            raise ValueError("unexpected git ls-files --stage output")
        tracked[encoded_path.decode("utf-8")] = fields[0].decode("ascii")
    return tracked


def parsed_hook_commands(
    settings: dict[str, Any],
) -> list[tuple[str, list[str], list[str], bool, bool, bool]]:
    commands: list[tuple[str, list[str], list[str], bool, bool, bool]] = []
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
                exec_form = "args" in entry
                arguments = entry.get("args", [])
                command = entry.get("command")
                if not isinstance(command, str):
                    continue
                if exec_form:
                    command_tokens = [command]
                else:
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
                commands.append(
                    (
                        command,
                        command_tokens,
                        argument_tokens,
                        not exec_form,
                        "shell" in entry,
                        entry.get("type", "command") != "command",
                    )
                )
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


def loader_candidate_target(
    candidate: str,
    project_expansion: str,
) -> tuple[str | None, str | None]:
    if candidate.lower().startswith(LOADER_URL_PREFIXES):
        return candidate, None
    normalized, explicit_project_path = normalize_hook_token(
        candidate,
        project_expansion,
    )
    explicit_filesystem_path = (
        explicit_project_path
        or candidate.startswith(("./", "../", ".\\", "..\\"))
        or is_home_relative_script_path(normalized)
        or is_absolute_script_path(normalized)
    )
    if explicit_filesystem_path:
        return normalized, None
    return (
        None,
        f"loader specifier {candidate!r} is not an explicit filesystem target",
    )


def trailing_filesystem_targets(
    tokens: list[str],
    project_expansion: str,
) -> list[str]:
    targets: list[str] = []
    for token in tokens:
        candidates = [token]
        if token.startswith("-") and "=" in token:
            candidates.append(token.split("=", 1)[1])
        for candidate in candidates:
            normalized, explicit_project_path = normalize_hook_token(
                candidate,
                project_expansion,
            )
            if (
                explicit_project_path
                or normalized.casefold().startswith(LOCAL_PREFIXES)
                or is_home_relative_script_path(normalized)
                or is_absolute_script_path(normalized)
            ):
                targets.append(normalized)
    return targets


def interpreter_invocation_targets(
    kind: str,
    tokens: list[str],
    project_expansion: str,
) -> tuple[list[str], str | None]:
    targets: list[str] = []
    index = 0
    while index < len(tokens):
        token = tokens[index]
        if token == "--":
            index += 1
            if index >= len(tokens) or not tokens[index]:
                return targets, "option terminator has no script operand"
            target, _ = normalize_hook_token(
                tokens[index],
                project_expansion,
            )
            targets.append(target)
            targets.extend(
                trailing_filesystem_targets(tokens[index + 1 :], project_expansion)
            )
            return targets, None

        if token == "-" or (kind == "shell" and token == "-s"):
            return targets, f"stdin/REPL mode {token!r} has no tracked script"

        if kind == "powershell" and token.lower() == "-file":
            if index + 1 >= len(tokens) or not tokens[index + 1]:
                return targets, "-File has no script operand"
            target, _ = normalize_hook_token(
                tokens[index + 1],
                project_expansion,
            )
            targets.append(target)
            targets.extend(
                trailing_filesystem_targets(tokens[index + 2 :], project_expansion)
            )
            return targets, None

        candidate: str | None = None
        separated_options = (
            EMBEDDED_PATH_OPTIONS if kind == "node" else {"-r", "--require"}
        )
        if kind in {"node", "ruby"} and token.startswith("-") and "=" in token:
            option, value = token.split("=", 1)
            if (
                kind == "node"
                and option.lower() in EMBEDDED_PATH_OPTIONS
                or kind == "ruby"
                and option == "--require"
            ):
                candidate = value
                if not candidate:
                    return targets, f"loader option {token!r} has no operand"
        elif kind in {"node", "ruby"} and token.startswith("-r") and len(token) > 2:
            candidate = token[2:].removeprefix("=")
            if not candidate:
                return targets, f"loader option {token!r} has no operand"
        elif kind in {"node", "ruby"} and token.lower() in separated_options:
            if index + 1 < len(tokens):
                candidate = tokens[index + 1]
                index += 1
            else:
                return targets, f"loader option {token!r} has no operand"

        if candidate is not None:
            if not candidate:
                return targets, f"loader option {token!r} has no operand"
            loader_target, loader_error = loader_candidate_target(
                candidate,
                project_expansion,
            )
            if loader_error is not None:
                return targets, loader_error
            if loader_target is not None:
                targets.append(loader_target)
            index += 1
            continue

        if token.startswith("-"):
            return targets, f"option {token!r} is outside the supported argv grammar"

        target, _ = normalize_hook_token(token, project_expansion)
        targets.append(target)
        targets.extend(
            trailing_filesystem_targets(tokens[index + 1 :], project_expansion)
        )
        return targets, None

    return targets, "interpreter invocation has no tracked script"


def is_trusted_system_executable(executable: str) -> bool:
    if not is_absolute_script_path(executable):
        return True
    normalized = executable.replace("\\", "/")
    if ".." in normalized.split("/"):
        return False
    if WINDOWS_ABSOLUTE_PATH.match(executable) is not None:
        return normalized.lower().startswith(TRUSTED_WINDOWS_EXECUTABLE_PREFIXES)
    return normalized.startswith(TRUSTED_POSIX_EXECUTABLE_PREFIXES)


def is_command_launcher(executable: str, legacy_shell: bool) -> bool:
    normalized = executable.replace("\\", "/")
    name = normalized.rsplit("/", 1)[-1]
    if name not in COMMAND_LAUNCHERS:
        return False
    if not legacy_shell and name != "env":
        return False
    return "/" not in normalized or (
        is_absolute_script_path(executable) and is_trusted_system_executable(executable)
    )


def unwrap_command_launcher(
    tokens: list[str],
    legacy_shell: bool,
) -> tuple[list[str], str | None]:
    resolved = list(tokens)
    while resolved:
        if ENV_ASSIGNMENT.fullmatch(resolved[0]):
            variable = resolved[0].split("=", 1)[0]
            return (
                [],
                f"shared hook environment assignment cannot be validated: {variable}",
            )
        project_expansion = "legacy" if legacy_shell else "exec"
        executable, explicit_executable_path = normalize_hook_token(
            resolved[0],
            project_expansion,
        )
        if explicit_executable_path or not is_command_launcher(
            executable,
            legacy_shell,
        ):
            return resolved, None

        original = resolved.pop(0)
        if resolved and resolved[0] == "--":
            resolved.pop(0)
        if not resolved or resolved[0].startswith("-"):
            return [], f"shared hook command launcher cannot be validated: {original}"
    return [], "shared hook command launcher has no executable target"


def hook_targets(settings: dict[str, Any]) -> list[str]:
    targets: list[str] = []
    for (
        raw_command,
        command_tokens,
        argument_tokens,
        legacy_shell,
        custom_shell,
        unsupported_type,
    ) in parsed_hook_commands(settings):
        if custom_shell or unsupported_type:
            continue
        if legacy_shell and has_unsafe_raw_shell_syntax(raw_command):
            continue
        tokens = [*command_tokens, *argument_tokens]
        if any(SHELL_CONTROL.search(token) for token in tokens):
            continue
        project_expansion = "legacy" if legacy_shell else "exec"
        for token in tokens:
            normalized, explicit_project_path = normalize_hook_token(
                token,
                project_expansion,
            )
            if explicit_project_path or normalized.casefold().startswith(
                LOCAL_PREFIXES
            ):
                targets.append(normalized)

        resolved, error = unwrap_command_launcher(tokens, legacy_shell)
        if error is not None or not resolved:
            continue
        executable, _ = normalize_hook_token(resolved[0], project_expansion)
        if is_relative_script_path(executable):
            targets.append(executable)
            continue
        if is_home_relative_script_path(executable):
            targets.append(executable)
            continue
        kind = interpreter_kind(executable)
        if is_absolute_script_path(executable) and (
            kind is None or not is_trusted_system_executable(executable)
        ):
            targets.append(executable)
            continue
        if kind is None:
            after_option_terminator = False
            for token in resolved[1:]:
                if token == "--" and not after_option_terminator:
                    after_option_terminator = True
                    continue
                normalized, explicit_project_path = normalize_hook_token(
                    token,
                    project_expansion,
                )
                if token.startswith("-") and not after_option_terminator:
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
            interpreter_targets, _ = interpreter_invocation_targets(
                kind,
                script_tokens,
                project_expansion,
            )
            targets.extend(interpreter_targets)
    return list(dict.fromkeys(targets))


def normalize_hook_token(
    token: str,
    project_expansion: str = "legacy",
) -> tuple[str, bool]:
    if token.startswith("./"):
        return token.removeprefix("./"), True
    normalized = token
    if project_expansion != "none":
        project_prefixes = (
            EXEC_PROJECT_PREFIXES if project_expansion == "exec" else PROJECT_PREFIXES
        )
        for project_prefix in project_prefixes:
            if normalized.startswith(project_prefix):
                project_path = normalized.removeprefix(project_prefix)
                return project_path.replace("\\", "/"), True
    return normalized, False


def is_relative_script_path(token: str) -> bool:
    return (
        not token.startswith(("/", "~", "\\"))
        and WINDOWS_ABSOLUTE_PATH.match(token) is None
        and not is_home_relative_script_path(token)
        and "://" not in token
        and ("/" in token or token.lower().endswith(SCRIPT_SUFFIXES))
    )


def is_absolute_script_path(token: str) -> bool:
    return (
        (
            token.startswith(("/", "\\"))
            or WINDOWS_ABSOLUTE_PATH.match(token) is not None
        )
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
                elif "args" not in entry:
                    try:
                        shlex.split(command)
                    except ValueError as error:
                        failures.append(
                            f"{location}.command is not valid shell syntax: {error}"
                        )
                if "shell" in entry:
                    failures.append(
                        f"{location}.shell is not supported in shared hooks"
                    )
                if entry.get("type", "command") != "command":
                    failures.append(f"{location}.type must be command in shared hooks")
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
    tracked: Collection[str],
    wrapper_contracts: Mapping[str, str] | None = None,
) -> list[str]:
    failures: list[str] = []
    for (
        raw_command,
        command_tokens,
        argument_tokens,
        legacy_shell,
        custom_shell,
        unsupported_type,
    ) in parsed_hook_commands(settings):
        if unsupported_type:
            failures.append(
                f"shared hook type cannot be validated as a command: {raw_command}"
            )
            continue
        if custom_shell:
            failures.append(
                f"shared hook shell selection cannot be validated: {raw_command}"
            )
            continue
        if legacy_shell and has_unsafe_raw_shell_syntax(raw_command):
            executable = command_tokens[0] if command_tokens else "<command>"
            failures.append(
                f"shared hook dynamic shell syntax cannot be validated: {executable}"
            )
            continue
        resolved, launcher_error = unwrap_command_launcher(
            [*command_tokens, *argument_tokens],
            legacy_shell,
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
        project_expansion = "legacy" if legacy_shell else "exec"
        executable, explicit_executable_path = normalize_hook_token(
            resolved[0],
            project_expansion,
        )
        kind = interpreter_kind(executable)
        tracked_wrapper = executable in tracked and (
            explicit_executable_path or "/" in executable or "\\" in executable
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
        if mode is not None:
            failures.append(
                "shared hook inline interpreter mode cannot be validated: "
                f"{resolved[0]} {mode}"
            )
            continue
        if (
            kind is None
            and not tracked_wrapper
            and not explicit_executable_path
            and not is_relative_script_path(executable)
            and not is_home_relative_script_path(executable)
            and not is_absolute_script_path(executable)
        ):
            failures.append(
                "shared hook executable is not an explicitly supported "
                f"interpreter or tracked wrapper: {executable}"
            )
            continue
        if kind is not None and is_absolute_script_path(executable):
            if not is_trusted_system_executable(executable):
                continue
        if kind is not None and not tracked_wrapper:
            _, invocation_error = interpreter_invocation_targets(
                kind,
                resolved[1:],
                project_expansion,
            )
            if invocation_error is not None:
                failures.append(
                    "shared hook interpreter invocation cannot be validated: "
                    f"{resolved[0]} ({invocation_error})"
                )
    for target in hook_targets(settings):
        if target.lower().startswith(LOADER_URL_PREFIXES):
            failures.append(f"shared hook loader URL cannot be validated: {target}")
        elif is_home_relative_script_path(target):
            failures.append(f"shared hook target must not be home-relative: {target}")
        elif is_absolute_script_path(target):
            failures.append(f"shared hook target must not be absolute: {target}")
        elif target.casefold().startswith(".ievo/hooks/"):
            failures.append(f"shared hook target must remain machine-local: {target}")
        elif target not in tracked:
            failures.append(f"shared hook target is not tracked: {target}")
        elif wrapper_contracts is not None and target not in wrapper_contracts:
            failures.append(
                f"shared hook target lacks a registered fail-silent contract: {target}"
            )
    return failures


def validate_tracked_hook_modes(
    settings: dict[str, Any],
    tracked_modes: Mapping[str, str],
) -> list[str]:
    failures: list[str] = []
    for target in hook_targets(settings):
        mode = tracked_modes.get(target)
        if mode == "120000":
            failures.append(f"shared hook target must not be a symlink: {target}")
        elif mode == "160000":
            failures.append(f"shared hook target must not be a submodule: {target}")

    for (
        raw_command,
        command_tokens,
        argument_tokens,
        legacy_shell,
        custom_shell,
        unsupported_type,
    ) in parsed_hook_commands(settings):
        if custom_shell or unsupported_type:
            continue
        if legacy_shell and has_unsafe_raw_shell_syntax(raw_command):
            continue
        resolved, error = unwrap_command_launcher(
            [*command_tokens, *argument_tokens],
            legacy_shell,
        )
        if error is not None or not resolved:
            continue
        project_expansion = "legacy" if legacy_shell else "exec"
        executable, explicit_path = normalize_hook_token(
            resolved[0],
            project_expansion,
        )
        direct_tracked_wrapper = executable in tracked_modes and (
            explicit_path or "/" in executable or "\\" in executable
        )
        if direct_tracked_wrapper and tracked_modes[executable] == "100644":
            failures.append(
                f"direct shared hook wrapper must be executable: {executable}"
            )
    return failures


def validate_wrapper_contracts(
    tracked: Collection[str],
    wrapper_contracts: Mapping[str, str] | None = None,
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


def validate_machine_local_files(tracked: Collection[str]) -> list[str]:
    return [
        f"machine-local iEvo hook must not be tracked: {target}"
        for target in sorted(tracked)
        if target.casefold().startswith(".ievo/hooks/")
    ]


def main() -> int:
    failures: list[str] = []
    settings = json.loads(
        (ROOT / ".claude" / "settings.json").read_text(encoding="utf-8")
    )
    tracked = tracked_files()
    failures.extend(validate_hook_schema(settings))
    failures.extend(
        validate_hook_targets(settings, tracked, FAIL_SILENT_WRAPPER_CONTRACTS)
    )
    failures.extend(validate_tracked_hook_modes(settings, tracked))
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
