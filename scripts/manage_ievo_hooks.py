#!/usr/bin/env python3
"""Keep iEvo's generated hook wiring machine-local for this repository."""

from __future__ import annotations

import argparse
import copy
import json
import os
import posixpath
import re
import shlex
import stat
import subprocess
import sys
import tempfile
from collections.abc import Callable, Iterable, Iterator
from contextlib import contextmanager
from fnmatch import fnmatchcase
from functools import wraps
from pathlib import Path
from typing import Any

if os.name == "nt":
    import msvcrt
else:
    import fcntl


ROOT = Path(__file__).resolve().parents[1]
CLAUDE_SHARED = Path(".claude/settings.json")
CLAUDE_LOCAL = Path(".claude/settings.local.json")
CODEX_LOCAL = Path(".codex/hooks.json")
FLAG = Path(".ievo/evo-auto.flag")
SCRIPT_DIRECTORY = Path(".ievo/hooks/scripts")
HOOK_DIRECTORY = SCRIPT_DIRECTORY.parent
SCRIPT_TARGETS = {
    "correction-capture": SCRIPT_DIRECTORY / "correction-capture.sh",
    "evo-analysis-nudge": SCRIPT_DIRECTORY / "evo-analysis-nudge.sh",
    "failure-capture": SCRIPT_DIRECTORY / "failure-capture.sh",
}
EVENT_TARGETS = {
    "UserPromptSubmit": SCRIPT_TARGETS["correction-capture"],
    "SessionStart": SCRIPT_TARGETS["evo-analysis-nudge"],
    "PostToolUseFailure": SCRIPT_TARGETS["failure-capture"],
    "PermissionDenied": SCRIPT_TARGETS["failure-capture"],
    "PermissionRequest": SCRIPT_TARGETS["failure-capture"],
}
LOCAL_COMPANIONS = tuple(
    target.with_name(f"{target.stem}.local.sh") for target in SCRIPT_TARGETS.values()
)
VENDOR_DIRECTORY = SCRIPT_DIRECTORY / "vendor"
FLAG_REQUIREMENTS = {
    "enabled": "true",
    "signal": "corrections-only",
    "auto_write_scope": "project-wide-only",
}
REFERENCE_TOKEN_SPLIT = re.compile(r"[\s`;|&()<>{}\[\],=]+")
GLOB_TOKEN_SPLIT = re.compile(r"[\s`;|&()<>{},=]+")
SHELL_EXPANSION_MARKER = "__pycc_shell_expansion__"
SHELL_EXPANSIONS = (
    re.compile(r"\$\([^()\r\n]*\)"),
    re.compile(r"\$\[[^\]\r\n]*\]"),
    re.compile(r"\$'(?:\\.|[^'\\])*'"),
    re.compile(r'\$"(?:\\.|[^"\\])*"'),
    re.compile(r"[@+?!*]\([^()\r\n]*\)"),
    re.compile(r"@[A-Za-z_][A-Za-z0-9_]*"),
    re.compile(r"`[^`]*`"),
    re.compile(r"\$\{[^{}\r\n]*\}"),
    re.compile(r"\$env:[A-Za-z_][A-Za-z0-9_]*", re.IGNORECASE),
    re.compile(r"\$[A-Za-z_][A-Za-z0-9_]*"),
    re.compile(r"%[^%\r\n]+%"),
    re.compile(r"![A-Za-z_][A-Za-z0-9_]*!"),
    re.compile(r"\{[^{}\r\n]*(?:,|\.\.)[^{}\r\n]*\}"),
)
MANAGED_REFERENCE_PARTS = tuple(
    component.casefold() for component in HOOK_DIRECTORY.parts
)
DOS_SHORT_NAME_COMPONENT = re.compile(r"^[^./\\\s]{1,6}~[0-9]+(?:\.[^./\\\s]{0,3})?$")
POWERSHELL_CONSTANT_EXPRESSION = re.compile(
    r"(?:[+&|;()]|::|(?:^|[\s,])-(?:(?:c|i)?replace|join|f)(?=$|[\s,])|"
    r"\.[A-Za-z_][A-Za-z0-9_]*\s*\()",
    re.IGNORECASE,
)
GITIGNORE = Path(".gitignore")
REQUIRED_IGNORE_LINES = (
    ".claude/settings.local.json",
    ".codex/hooks.json",
    ".ievo/hooks/",
)
UPSTREAM_TRACKED_SHIM_LINES = {
    ".ievo/hooks/*",
    "!.ievo/hooks/scripts/",
    ".ievo/hooks/scripts/*",
    "!.ievo/hooks/scripts/correction-capture.sh",
    "!.ievo/hooks/scripts/evo-analysis-nudge.sh",
    "!.ievo/hooks/scripts/failure-capture.sh",
}
SMOKE_PAYLOAD = json.dumps(
    {
        "session_id": "pycc-ievo-hook-smoke",
        "prompt": "",
        "error": "",
    }
)


class HookLifecycleError(RuntimeError):
    """An iEvo hook lifecycle operation cannot complete safely."""


FileIdentity = tuple[int, int, int, int, int]


def file_identity(result: os.stat_result) -> FileIdentity:
    return (
        result.st_dev,
        result.st_ino,
        result.st_mode,
        result.st_size,
        result.st_mtime_ns,
    )


def stat_is_reparse_point(result: os.stat_result) -> bool:
    attributes = getattr(result, "st_file_attributes", 0)
    reparse_attribute = getattr(stat, "FILE_ATTRIBUTE_REPARSE_POINT", 0x400)
    return bool(attributes & reparse_attribute)


def ensure_lock_directory(path: Path) -> None:
    try:
        path_stat = path.lstat()
    except OSError as error:
        raise HookLifecycleError(
            f"cannot inspect lifecycle lock directory {path}: {error}"
        ) from error
    if (
        not stat.S_ISDIR(path_stat.st_mode)
        or stat.S_ISLNK(path_stat.st_mode)
        or stat_is_reparse_point(path_stat)
    ):
        raise HookLifecycleError(
            f"lifecycle lock directory must be a regular non-link directory: {path}"
        )


def ensure_lock_directory_components(path: Path) -> None:
    if not path.is_absolute():
        raise HookLifecycleError(f"lifecycle lock directory must be absolute: {path}")
    current = Path(path.anchor)
    for component in path.parts[1:]:
        current /= component
        try:
            current_stat = current.lstat()
        except OSError as error:
            raise HookLifecycleError(
                f"cannot inspect lifecycle lock directory component {current}: {error}"
            ) from error
        if (
            not stat.S_ISDIR(current_stat.st_mode)
            or stat.S_ISLNK(current_stat.st_mode)
            or stat_is_reparse_point(current_stat)
        ):
            raise HookLifecycleError(
                "lifecycle lock directory path contains a non-directory, symlink, "
                f"or reparse point: {current}"
            )


def lifecycle_lock_path(root: Path) -> Path:
    marker = root / ".git"
    try:
        marker_stat = marker.lstat()
    except FileNotFoundError:
        ensure_lock_directory(root)
        return root / ".pycc-ievo-hooks.lock"
    except OSError as error:
        raise HookLifecycleError(
            f"cannot inspect repository metadata: {error}"
        ) from error

    if stat.S_ISLNK(marker_stat.st_mode) or stat_is_reparse_point(marker_stat):
        raise HookLifecycleError(
            "repository .git path must not be a symlink or reparse point"
        )
    if stat.S_ISDIR(marker_stat.st_mode):
        # The marker itself was inspected without following links. Canonicalize
        # only its already-trusted ancestors (for example macOS /var -> /private/var).
        git_directory = marker.resolve()
    elif stat.S_ISREG(marker_stat.st_mode):
        try:
            declaration = marker.read_text(encoding="utf-8").strip()
        except (OSError, UnicodeError) as error:
            raise HookLifecycleError(
                f"cannot read repository metadata: {error}"
            ) from error
        prefix = "gitdir: "
        if not declaration.startswith(prefix):
            raise HookLifecycleError(
                "worktree .git file has an invalid gitdir declaration"
            )
        git_directory = Path(declaration[len(prefix) :])
        if not git_directory.is_absolute():
            git_directory = marker.parent.resolve() / git_directory
    else:
        raise HookLifecycleError("repository .git path must be a file or directory")
    git_directory = Path(os.path.abspath(git_directory))
    ensure_lock_directory_components(git_directory)
    ensure_lock_directory(git_directory)
    return git_directory / "pycc-ievo-hooks.lock"


@contextmanager
def lifecycle_lock(root: Path) -> Iterator[None]:
    path = lifecycle_lock_path(root)
    try:
        before = path.lstat()
    except FileNotFoundError:
        before = None
    except OSError as error:
        raise HookLifecycleError(
            f"cannot inspect lifecycle lock {path}: {error}"
        ) from error
    if before is not None and (
        not stat.S_ISREG(before.st_mode)
        or stat.S_ISLNK(before.st_mode)
        or stat_is_reparse_point(before)
    ):
        raise HookLifecycleError(
            f"lifecycle lock must be a regular non-link file: {path}"
        )
    try:
        descriptor = os.open(
            path,
            os.O_CREAT
            | os.O_RDWR
            | getattr(os, "O_CLOEXEC", 0)
            | getattr(os, "O_NOFOLLOW", 0),
            0o600,
        )
    except OSError as error:
        raise HookLifecycleError(
            f"cannot open lifecycle lock {path}: {error}"
        ) from error

    acquired = False
    try:
        descriptor_stat = os.fstat(descriptor)
        try:
            after = path.lstat()
        except OSError as error:
            raise HookLifecycleError(
                f"cannot revalidate lifecycle lock {path}: {error}"
            ) from error
        if (
            not stat.S_ISREG(descriptor_stat.st_mode)
            or stat_is_reparse_point(descriptor_stat)
            or not stat.S_ISREG(after.st_mode)
            or stat.S_ISLNK(after.st_mode)
            or stat_is_reparse_point(after)
            or file_identity(descriptor_stat)[:3] != file_identity(after)[:3]
            or (
                before is not None
                and file_identity(before)[:3] != file_identity(after)[:3]
            )
        ):
            raise HookLifecycleError(
                f"lifecycle lock changed or redirected while opening: {path}"
            )
        if os.name == "nt":
            if descriptor_stat.st_size == 0:
                os.write(descriptor, b"\0")
            os.lseek(descriptor, 0, os.SEEK_SET)
            try:
                msvcrt.locking(descriptor, msvcrt.LK_NBLCK, 1)
            except OSError as error:
                raise HookLifecycleError(
                    f"another iEvo hook lifecycle operation is active: {path}"
                ) from error
        else:
            try:
                fcntl.flock(descriptor, fcntl.LOCK_EX | fcntl.LOCK_NB)
            except BlockingIOError as error:
                raise HookLifecycleError(
                    f"another iEvo hook lifecycle operation is active: {path}"
                ) from error
            except OSError as error:
                raise HookLifecycleError(
                    f"cannot acquire lifecycle lock {path}: {error}"
                ) from error
        acquired = True
        yield
    finally:
        if acquired:
            if os.name == "nt":
                os.lseek(descriptor, 0, os.SEEK_SET)
                msvcrt.locking(descriptor, msvcrt.LK_UNLCK, 1)
            else:
                fcntl.flock(descriptor, fcntl.LOCK_UN)
        os.close(descriptor)


def serialized_lifecycle(
    function: Callable[..., Any],
) -> Callable[..., Any]:
    @wraps(function)
    def wrapper(root: Path, *args: object, **kwargs: object) -> Any:
        with lifecycle_lock(root):
            return function(root, *args, **kwargs)

    return wrapper


def read_json(root: Path, relative: Path, *, required: bool) -> dict[str, Any] | None:
    path = root / relative
    if not path.exists():
        if required:
            raise HookLifecycleError(f"required configuration is missing: {relative}")
        return None
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeError, json.JSONDecodeError) as error:
        raise HookLifecycleError(
            f"cannot read valid JSON from {relative}: {error}"
        ) from error
    if not isinstance(value, dict):
        raise HookLifecycleError(f"configuration must be a JSON object: {relative}")
    hooks = value.get("hooks")
    if hooks is not None and not isinstance(hooks, dict):
        raise HookLifecycleError(f"hooks must be a JSON object: {relative}")
    return value


def read_json_snapshot(
    root: Path,
    relative: Path,
    *,
    required: bool,
) -> tuple[dict[str, Any] | None, bytes | None]:
    path = root / relative
    try:
        contents = path.read_bytes()
    except FileNotFoundError:
        if required:
            raise HookLifecycleError(f"required configuration is missing: {relative}")
        return None, None
    except OSError as error:
        raise HookLifecycleError(f"cannot read {relative}: {error}") from error
    try:
        value = json.loads(contents.decode("utf-8"))
    except (UnicodeError, json.JSONDecodeError) as error:
        raise HookLifecycleError(
            f"cannot read valid JSON from {relative}: {error}"
        ) from error
    if not isinstance(value, dict):
        raise HookLifecycleError(f"configuration must be a JSON object: {relative}")
    hooks = value.get("hooks")
    if hooks is not None and not isinstance(hooks, dict):
        raise HookLifecycleError(f"hooks must be a JSON object: {relative}")
    return value, contents


def current_file_bytes(root: Path, relative: Path) -> bytes | None:
    try:
        return (root / relative).read_bytes()
    except FileNotFoundError:
        return None
    except OSError as error:
        raise HookLifecycleError(f"cannot re-read {relative}: {error}") from error


def atomic_write_json(root: Path, relative: Path, value: dict[str, Any]) -> None:
    path = root / relative
    path.parent.mkdir(parents=True, exist_ok=True)
    existing_mode = stat.S_IMODE(path.stat().st_mode) if path.exists() else 0o600
    descriptor, temporary_name = tempfile.mkstemp(
        prefix=f".{path.name}.",
        suffix=".tmp",
        dir=path.parent,
    )
    temporary = Path(temporary_name)
    try:
        with os.fdopen(descriptor, "w", encoding="utf-8") as handle:
            json.dump(value, handle, indent=2)
            handle.write("\n")
            handle.flush()
            os.fsync(handle.fileno())
        os.chmod(temporary, existing_mode)
        os.replace(temporary, path)
    finally:
        if temporary.exists():
            temporary.unlink()


def atomic_write_text(root: Path, relative: Path, contents: str) -> None:
    path = root / relative
    path.parent.mkdir(parents=True, exist_ok=True)
    existing_mode = stat.S_IMODE(path.stat().st_mode) if path.exists() else 0o644
    descriptor, temporary_name = tempfile.mkstemp(
        prefix=f".{path.name}.",
        suffix=".tmp",
        dir=path.parent,
    )
    temporary = Path(temporary_name)
    try:
        with os.fdopen(descriptor, "w", encoding="utf-8") as handle:
            handle.write(contents)
            handle.flush()
            os.fsync(handle.fileno())
        os.chmod(temporary, existing_mode)
        os.replace(temporary, path)
    finally:
        if temporary.exists():
            temporary.unlink()


def normalized_gitignore(root: Path) -> str:
    path = root / GITIGNORE
    if not path.exists():
        raise HookLifecycleError("required configuration is missing: .gitignore")
    contents = path.read_text(encoding="utf-8")
    lines: list[str] = []
    restored_hook_rule = False
    for line in contents.splitlines():
        if line not in UPSTREAM_TRACKED_SHIM_LINES:
            lines.append(line)
            continue
        if not restored_hook_rule:
            lines.append(".ievo/hooks/")
            restored_hook_rule = True
    marker = "# iEvo local-only artifacts"
    if marker not in lines:
        raise HookLifecycleError(
            ".gitignore is missing the managed iEvo local-only section"
        )
    if REQUIRED_IGNORE_LINES[0] not in lines:
        lines.insert(lines.index(marker) + 1, REQUIRED_IGNORE_LINES[0])
    if REQUIRED_IGNORE_LINES[1] not in lines:
        lines.insert(
            lines.index(REQUIRED_IGNORE_LINES[0]) + 1, REQUIRED_IGNORE_LINES[1]
        )
    if REQUIRED_IGNORE_LINES[2] not in lines:
        anchor = ".ievo/cache/"
        insertion = (
            lines.index(anchor) + 1
            if anchor in lines
            else lines.index(REQUIRED_IGNORE_LINES[1]) + 1
        )
        lines.insert(insertion, REQUIRED_IGNORE_LINES[2])
    return "\n".join(lines) + "\n"


VendorSnapshot = dict[str, FileIdentity]


def ensure_machine_local_paths_ignored(root: Path) -> VendorSnapshot:
    vendor_snapshot = inspect_managed_vendor(root)
    paths = [
        *REQUIRED_IGNORE_LINES[:2],
        *(target.as_posix() for target in SCRIPT_TARGETS.values()),
        *(target.as_posix() for target in LOCAL_COMPANIONS),
        VENDOR_DIRECTORY.as_posix(),
        *(
            relative
            for relative, identity in vendor_snapshot.items()
            if stat.S_ISREG(identity[2])
        ),
    ]
    not_ignored: list[str] = []
    for relative in paths:
        result = subprocess.run(
            ["git", "check-ignore", "--quiet", "--", relative],
            cwd=root,
            check=False,
            capture_output=True,
        )
        if result.returncode != 0:
            not_ignored.append(relative)
    if not_ignored:
        raise HookLifecycleError(
            "machine-local iEvo paths are not ignored: " + ", ".join(not_ignored)
        )
    return vendor_snapshot


def inspect_managed_vendor(root: Path) -> VendorSnapshot:
    vendor = root / VENDOR_DIRECTORY
    try:
        vendor_stat = vendor.lstat()
    except FileNotFoundError:
        return {}
    except OSError as error:
        raise HookLifecycleError(
            f"cannot inspect managed vendor path {VENDOR_DIRECTORY}: {error}"
        ) from error
    if not stat.S_ISDIR(vendor_stat.st_mode) or stat_is_reparse_point(vendor_stat):
        raise HookLifecycleError(
            f"managed vendor path must be a regular directory: {VENDOR_DIRECTORY}"
        )
    try:
        parent_stat = vendor.parent.lstat()
    except OSError as error:
        raise HookLifecycleError(
            f"cannot inspect managed vendor parent: {error}"
        ) from error
    if (
        not stat.S_ISDIR(parent_stat.st_mode)
        or stat_is_reparse_point(parent_stat)
        or vendor_stat.st_dev != parent_stat.st_dev
        or os.path.ismount(vendor)
    ):
        raise HookLifecycleError(
            f"managed vendor path crosses a mount boundary: {VENDOR_DIRECTORY}"
        )

    def raise_walk_error(error: OSError) -> None:
        raise HookLifecycleError(
            f"cannot inspect managed vendor tree: {error}"
        ) from error

    snapshot: VendorSnapshot = {VENDOR_DIRECTORY.as_posix(): file_identity(vendor_stat)}
    vendor_device = vendor_stat.st_dev
    for directory, directory_names, file_names in os.walk(
        vendor,
        topdown=True,
        onerror=raise_walk_error,
        followlinks=False,
    ):
        current = Path(directory)
        try:
            current_stat = current.lstat()
        except OSError as error:
            raise HookLifecycleError(
                f"cannot inspect managed vendor tree: {error}"
            ) from error
        current_relative = current.relative_to(root).as_posix()
        if (
            not stat.S_ISDIR(current_stat.st_mode)
            or stat_is_reparse_point(current_stat)
            or current_stat.st_dev != vendor_device
            or (current != vendor and os.path.ismount(current))
        ):
            raise HookLifecycleError(
                f"managed vendor tree contains an unsafe directory: {current_relative}"
            )
        snapshot[current_relative] = file_identity(current_stat)
        for name in directory_names:
            path = current / name
            try:
                path_stat = path.lstat()
            except OSError as error:
                raise HookLifecycleError(
                    f"cannot inspect managed vendor tree: {error}"
                ) from error
            relative = path.relative_to(root).as_posix()
            if (
                not stat.S_ISDIR(path_stat.st_mode)
                or stat_is_reparse_point(path_stat)
                or path_stat.st_dev != vendor_device
                or os.path.ismount(path)
            ):
                raise HookLifecycleError(
                    f"managed vendor tree contains an unsafe directory: {relative}"
                )
            snapshot[relative] = file_identity(path_stat)
        for name in file_names:
            path = current / name
            try:
                path_stat = path.lstat()
            except OSError as error:
                raise HookLifecycleError(
                    f"cannot inspect managed vendor tree: {error}"
                ) from error
            relative = path.relative_to(root).as_posix()
            if (
                not stat.S_ISREG(path_stat.st_mode)
                or stat_is_reparse_point(path_stat)
                or path_stat.st_dev != vendor_device
            ):
                raise HookLifecycleError(
                    f"managed vendor tree contains a non-regular file: {relative}"
                )
            snapshot[relative] = file_identity(path_stat)
    return snapshot


def hook_target(event: str, entry: object) -> Path | None:
    expected = EVENT_TARGETS.get(event)
    if expected is None or not isinstance(entry, dict):
        return None
    if entry.get("type", "command") != "command":
        return None
    command = entry.get("command")
    arguments = entry.get("args")
    target = expected.as_posix()
    if command == "sh" and arguments == [target]:
        return expected
    if command == f"sh {target}" and (arguments is None or arguments == []):
        return expected
    return None


HookRecord = tuple[str, Path, dict[str, Any]]


def strip_ievo_entries(
    settings: dict[str, Any],
) -> tuple[dict[str, Any], list[HookRecord]]:
    result = copy.deepcopy(settings)
    hooks = result.get("hooks")
    if not isinstance(hooks, dict):
        return result, []

    records: list[HookRecord] = []
    rewritten_hooks: dict[str, Any] = {}
    for event, groups in hooks.items():
        if not isinstance(groups, list):
            rewritten_hooks[event] = groups
            continue
        rewritten_groups: list[Any] = []
        event_changed = False
        for group in groups:
            if not isinstance(group, dict) or not isinstance(group.get("hooks"), list):
                rewritten_groups.append(group)
                continue
            remaining_entries: list[Any] = []
            group_changed = False
            for entry in group["hooks"]:
                target = hook_target(event, entry)
                if target is None:
                    remaining_entries.append(entry)
                    continue
                group_changed = True
                event_changed = True
                moved_group = copy.deepcopy(group)
                moved_group["hooks"] = [copy.deepcopy(entry)]
                records.append((event, target, moved_group))
            if remaining_entries or not group_changed:
                rewritten_group = copy.deepcopy(group)
                rewritten_group["hooks"] = remaining_entries
                rewritten_groups.append(rewritten_group)
        if rewritten_groups or not event_changed:
            rewritten_hooks[event] = rewritten_groups

    if rewritten_hooks or not records:
        result["hooks"] = rewritten_hooks
    else:
        result.pop("hooks", None)
    return result, records


ManagedReference = tuple[str, Path]


def string_values(value: object) -> Iterable[str]:
    if isinstance(value, str):
        yield value
    elif isinstance(value, dict):
        for nested in value.values():
            yield from string_values(nested)
    elif isinstance(value, list):
        for nested in value:
            yield from string_values(nested)


def string_references_managed_target(candidate: str) -> bool:
    collapsed = (
        candidate.replace("\\\r\n", "")
        .replace("\\\n", "")
        .replace("`\r\n", "")
        .replace("`\n", "")
        .replace("^\r\n", "")
        .replace("^\n", "")
    )
    tokens: list[str] = []
    try:
        tokens.extend(shlex.split(collapsed, posix=True))
    except ValueError:
        # The quote-stripped conservative pass below still catches path-like
        # malformed shell strings; rejecting a possible alias is safer than
        # deleting its target.
        pass
    unquoted = collapsed.replace('"', "").replace("'", "")
    if POWERSHELL_CONSTANT_EXPRESSION.search(unquoted):
        # PowerShell can assemble a path from constants without a variable or
        # command substitution. Unmodeled expression syntax is ambiguous, so
        # destructive cleanup must retain both configuration and targets.
        return True
    tokens.extend(REFERENCE_TOKEN_SPLIT.split(unquoted))

    for token in tokens:
        if not token:
            continue
        normalized = posixpath.normpath(token.replace("\\", "/")).casefold()
        parts = tuple(component for component in normalized.split("/") if component)
        if any(DOS_SHORT_NAME_COMPONENT.fullmatch(component) for component in parts):
            # A DOS 8.3 alias does not retain enough spelling information to
            # prove that it cannot resolve to the managed tree on Windows.
            return True
        width = len(MANAGED_REFERENCE_PARTS)
        if any(
            parts[index : index + width] == MANAGED_REFERENCE_PARTS
            for index in range(len(parts) - width + 1)
        ):
            # Case-folding on every host is intentionally conservative: an alias
            # that is distinct on one clone may name the managed tree on another.
            return True
    folded = unquoted.replace("\\", "/").casefold()
    if ".ievo" in folded and "hooks" in folded:
        return True

    expansion_masked = collapsed
    while True:
        previous = expansion_masked
        for pattern in SHELL_EXPANSIONS:
            expansion_masked = pattern.sub(
                SHELL_EXPANSION_MARKER,
                expansion_masked,
            )
        if expansion_masked == previous:
            break
    if SHELL_EXPANSION_MARKER in expansion_masked:
        # A substitution may emit separators and the complete managed path. Its
        # value is unknowable during disable, so retaining config and targets is
        # the only safe outcome even when no static component names `.ievo`.
        return True
    if any(sigil in expansion_masked for sigil in ("$", "%", "!", "`")):
        # Malformed, legacy, or shell-specific substitutions are intentionally
        # unsupported. A remaining sigil may still supply the complete path.
        return True
    if any(marker in unquoted for marker in ("[[:", "[[.", "[[=")):
        # Python fnmatch intentionally does not implement the POSIX bracket
        # class, collating-symbol, or equivalence-class forms supported by
        # shells. Their locale-dependent result cannot be proven unrelated.
        return True

    shell_escape_normalized = unquoted.replace("^", "")
    if shell_escape_normalized != unquoted:
        # Cmd carets escape the following character. PowerShell backticks and
        # POSIX backtick substitutions were handled fail-closed above.
        return string_references_managed_target(shell_escape_normalized)

    for token in GLOB_TOKEN_SPLIT.split(unquoted):
        normalized = posixpath.normpath(token.replace("\\", "/")).casefold()
        parts = tuple(component for component in normalized.split("/") if component)
        if "**" in parts:
            return True
        for index in range(len(parts) - 1):
            patterns = parts[index : index + 2]
            if all(
                fnmatchcase(expected, pattern)
                for pattern, expected in zip(patterns, MANAGED_REFERENCE_PARTS)
            ):
                return True
    return False


def managed_target_references(settings: dict[str, Any]) -> list[ManagedReference]:
    hooks = settings.get("hooks")
    if not isinstance(hooks, dict):
        return []

    references: list[ManagedReference] = []
    for event, value in hooks.items():
        if not isinstance(event, str):
            continue
        if any(
            string_references_managed_target(candidate)
            for candidate in string_values(value)
        ):
            references.append((event, HOOK_DIRECTORY))
    return list(dict.fromkeys(references))


def ensure_no_managed_target_references(
    relative: Path,
    settings: dict[str, Any],
) -> None:
    references = managed_target_references(settings)
    if not references:
        return
    details = ", ".join(
        f"{event} -> {target.as_posix()}" for event, target in references
    )
    raise HookLifecycleError(
        f"unsupported iEvo hook reference remains in {relative}: {details}; "
        "update the lifecycle helper before deleting managed targets"
    )


def add_records(
    settings: dict[str, Any],
    records: Iterable[HookRecord],
) -> dict[str, Any]:
    result = copy.deepcopy(settings)
    hooks = result.setdefault("hooks", {})
    if not isinstance(hooks, dict):
        raise HookLifecycleError("hooks must be a JSON object")
    seen: set[tuple[str, Path]] = set()
    for event, target, group in records:
        key = (event, target)
        if key in seen:
            continue
        seen.add(key)
        event_groups = hooks.setdefault(event, [])
        if not isinstance(event_groups, list):
            raise HookLifecycleError(f"hooks.{event} must be a JSON array")
        event_groups.append(copy.deepcopy(group))
    return result


def ensure_no_symlink_components(root: Path, relative: Path) -> None:
    if relative.is_absolute() or ".." in relative.parts:
        raise HookLifecycleError(f"managed path must stay relative: {relative}")
    try:
        root_stat = root.lstat()
    except OSError as error:
        raise HookLifecycleError(
            f"cannot inspect managed path root {root}: {error}"
        ) from error
    if (
        not stat.S_ISDIR(root_stat.st_mode)
        or stat.S_ISLNK(root_stat.st_mode)
        or stat_is_reparse_point(root_stat)
    ):
        raise HookLifecycleError(
            f"managed path root must be a regular directory: {root}"
        )

    current = root
    for index, component in enumerate(relative.parts):
        current /= component
        try:
            current_stat = current.lstat()
        except FileNotFoundError:
            continue
        except OSError as error:
            raise HookLifecycleError(
                f"cannot inspect managed path component {current}: {error}"
            ) from error
        if stat.S_ISLNK(current_stat.st_mode) or stat_is_reparse_point(current_stat):
            raise HookLifecycleError(
                "managed path contains a symlink component or reparse point: "
                f"{current.relative_to(root)}"
            )
        if current_stat.st_dev != root_stat.st_dev or os.path.ismount(current):
            raise HookLifecycleError(
                f"managed path crosses a mount boundary: {current.relative_to(root)}"
            )
        if index < len(relative.parts) - 1 and not stat.S_ISDIR(current_stat.st_mode):
            raise HookLifecycleError(
                "managed path ancestor must be a directory: "
                f"{current.relative_to(root)}"
            )


def ensure_safe_lifecycle_paths(root: Path, relatives: Iterable[Path]) -> None:
    for relative in relatives:
        ensure_no_symlink_components(root, relative)


def existing_targets(root: Path, records: Iterable[HookRecord]) -> None:
    missing: list[str] = []
    unsafe: list[str] = []
    for target in dict.fromkeys(target for _, target, _ in records):
        ensure_no_symlink_components(root, target)
        path = root / target
        if not path.exists():
            missing.append(target.as_posix())
        elif path.is_symlink() or not path.is_file():
            unsafe.append(target.as_posix())
    if missing:
        raise HookLifecycleError(
            "hook target is missing; run the iEvo enable/refresh workflow first: "
            + ", ".join(missing)
        )
    if unsafe:
        raise HookLifecycleError(
            "hook target must be a regular non-symlink file: " + ", ".join(unsafe)
        )


@serialized_lifecycle
def localize(root: Path) -> None:
    ensure_safe_lifecycle_paths(
        root,
        (CLAUDE_SHARED, CLAUDE_LOCAL, CODEX_LOCAL, FLAG, GITIGNORE),
    )
    shared = read_json(root, CLAUDE_SHARED, required=True)
    local_value = read_json(root, CLAUDE_LOCAL, required=False)
    local = local_value or {}
    codex = read_json(root, CODEX_LOCAL, required=False) or {}
    assert shared is not None

    rewritten_shared, shared_records = strip_ievo_entries(shared)
    stripped_local, local_records = strip_ievo_entries(local)
    stripped_codex, codex_records = strip_ievo_entries(codex)
    for relative, stripped in (
        (CLAUDE_SHARED, rewritten_shared),
        (CLAUDE_LOCAL, stripped_local),
        (CODEX_LOCAL, stripped_codex),
    ):
        ensure_no_managed_target_references(relative, stripped)
    # A refresh writes authoritative metadata to shared settings. Prefer that
    # record over an older local copy when add_records() deduplicates by target.
    claude_records = [*shared_records, *local_records]
    all_records = [*claude_records, *codex_records]
    if not all_records:
        raise HookLifecycleError(
            "no iEvo hook entries found; run the iEvo enable/refresh workflow first"
        )
    existing_targets(root, all_records)
    ensure_corrections_only_intent(root)
    rewritten_local = add_records(stripped_local, claude_records)
    original_gitignore = (root / GITIGNORE).read_text(encoding="utf-8")
    rewritten_gitignore = normalized_gitignore(root)

    if rewritten_gitignore != original_gitignore:
        atomic_write_text(root, GITIGNORE, rewritten_gitignore)
    ensure_machine_local_paths_ignored(root)
    if rewritten_local != local and (local_value is not None or claude_records):
        atomic_write_json(root, CLAUDE_LOCAL, rewritten_local)
    if rewritten_shared != shared:
        atomic_write_json(root, CLAUDE_SHARED, rewritten_shared)


def intent_flag_values(root: Path) -> dict[str, str]:
    path = root / FLAG
    if not path.exists():
        raise HookLifecycleError(f"required configuration is missing: {FLAG}")
    try:
        contents = path.read_text(encoding="utf-8")
    except (OSError, UnicodeError) as error:
        raise HookLifecycleError(f"cannot read {FLAG}: {error}") from error

    values: dict[str, str] = {}
    for line in contents.splitlines():
        key, separator, value = line.partition(":")
        if not separator:
            continue
        key = key.strip()
        value = value.strip()
        if key in values and values[key] != value:
            raise HookLifecycleError(
                f"{FLAG} contains conflicting duplicate field: {key}"
            )
        values[key] = value
    return values


def ensure_corrections_only_intent(root: Path) -> None:
    values = intent_flag_values(root)
    mismatches = [
        f"{key}: {expected}"
        for key, expected in FLAG_REQUIREMENTS.items()
        if values.get(key) != expected
    ]
    if mismatches:
        raise HookLifecycleError(
            f"{FLAG} must state the corrections-only intent: " + ", ".join(mismatches)
        )


def local_records(root: Path) -> list[HookRecord]:
    records: list[HookRecord] = []
    for relative in (CLAUDE_LOCAL, CODEX_LOCAL):
        settings = read_json(root, relative, required=False)
        if settings is None:
            continue
        without_ievo, found = strip_ievo_entries(settings)
        ensure_no_managed_target_references(relative, without_ievo)
        records.extend(found)
    return records


@serialized_lifecycle
def check(root: Path, *, smoke: bool) -> None:
    ensure_safe_lifecycle_paths(
        root,
        (CLAUDE_SHARED, CLAUDE_LOCAL, CODEX_LOCAL, FLAG, GITIGNORE),
    )
    shared = read_json(root, CLAUDE_SHARED, required=True)
    assert shared is not None
    without_shared_ievo, shared_records = strip_ievo_entries(shared)
    ensure_no_managed_target_references(CLAUDE_SHARED, without_shared_ievo)
    if shared_records:
        raise HookLifecycleError(
            "iEvo hook entries remain in shared .claude/settings.json; run localize"
        )
    ensure_corrections_only_intent(root)

    records = local_records(root)
    if not records:
        raise HookLifecycleError("enabled iEvo mode has no machine-local hook entries")
    existing_targets(root, records)
    ensure_machine_local_paths_ignored(root)

    if not smoke:
        return
    for target in dict.fromkeys(target for _, target, _ in records):
        result = subprocess.run(
            ["sh", target.as_posix()],
            cwd=root,
            input=SMOKE_PAYLOAD,
            text=True,
            capture_output=True,
            check=False,
            timeout=15,
        )
        if result.returncode != 0:
            diagnostic = (result.stderr or result.stdout).strip()
            suffix = f": {diagnostic}" if diagnostic else ""
            raise HookLifecycleError(
                f"hook smoke failed for {target} with exit {result.returncode}{suffix}"
            )


def inspect_regular_removal_files(
    root: Path,
    targets: Iterable[Path],
) -> dict[str, FileIdentity]:
    snapshot: dict[str, FileIdentity] = {}
    for target in targets:
        path = root / target
        try:
            path_stat = path.lstat()
        except FileNotFoundError:
            continue
        except OSError as error:
            raise HookLifecycleError(
                f"cannot inspect generated removal target {target}: {error}"
            ) from error
        if not stat.S_ISREG(path_stat.st_mode):
            raise HookLifecycleError(
                f"generated removal target must be a regular file: {target}"
            )
        snapshot[target.as_posix()] = file_identity(path_stat)
    return snapshot


def ensure_snapshot_unchanged(
    label: str,
    expected: dict[str, FileIdentity],
    actual: dict[str, FileIdentity],
) -> None:
    if actual != expected:
        raise HookLifecycleError(f"{label} changed during the lifecycle operation")


def remove_validated_files(
    root: Path,
    snapshot: dict[str, FileIdentity],
) -> None:
    for relative, expected in snapshot.items():
        ensure_no_symlink_components(root, Path(relative))
        path = root / relative
        try:
            actual = file_identity(path.lstat())
        except OSError as error:
            raise HookLifecycleError(
                f"cannot revalidate generated removal target {relative}: {error}"
            ) from error
        if actual != expected:
            raise HookLifecycleError(
                f"generated removal target changed before unlink: {relative}"
            )
        path.unlink()


def remove_validated_vendor(root: Path, snapshot: VendorSnapshot) -> None:
    file_entries = [
        relative for relative, identity in snapshot.items() if stat.S_ISREG(identity[2])
    ]
    directory_entries = [
        relative for relative, identity in snapshot.items() if stat.S_ISDIR(identity[2])
    ]
    for relative in sorted(
        file_entries, key=lambda item: item.count("/"), reverse=True
    ):
        ensure_no_symlink_components(root, Path(relative))
        path = root / relative
        try:
            actual = file_identity(path.lstat())
        except OSError as error:
            raise HookLifecycleError(
                f"cannot revalidate managed vendor file {relative}: {error}"
            ) from error
        if actual != snapshot[relative]:
            raise HookLifecycleError(
                f"managed vendor file changed before unlink: {relative}"
            )
        path.unlink()
    for relative in sorted(
        directory_entries,
        key=lambda item: item.count("/"),
        reverse=True,
    ):
        ensure_no_symlink_components(root, Path(relative))
        path = root / relative
        try:
            actual = file_identity(path.lstat())
        except OSError as error:
            raise HookLifecycleError(
                f"cannot revalidate managed vendor directory {relative}: {error}"
            ) from error
        if actual[:3] != snapshot[relative][:3]:
            raise HookLifecycleError(
                f"managed vendor directory changed before removal: {relative}"
            )
        path.rmdir()


@serialized_lifecycle
def disable(root: Path) -> None:
    ensure_safe_lifecycle_paths(
        root,
        (CLAUDE_SHARED, CLAUDE_LOCAL, CODEX_LOCAL, FLAG, GITIGNORE),
    )
    ensure_corrections_only_intent(root)
    file_removal_targets = [
        *SCRIPT_TARGETS.values(),
        *LOCAL_COMPANIONS,
    ]
    removal_targets = [*file_removal_targets, VENDOR_DIRECTORY]
    for target in removal_targets:
        ensure_no_symlink_components(root, target)
    file_snapshot = inspect_regular_removal_files(root, file_removal_targets)

    configurations: list[tuple[Path, dict[str, Any] | None, bytes | None]] = []
    for relative in (CLAUDE_SHARED, CLAUDE_LOCAL, CODEX_LOCAL):
        settings, contents = read_json_snapshot(
            root,
            relative,
            required=relative == CLAUDE_SHARED,
        )
        configurations.append((relative, settings, contents))

    rewritten: list[tuple[Path, dict[str, Any], bytes, dict[str, Any]]] = []
    expected_configurations: dict[Path, dict[str, Any] | None] = {}
    for relative, settings, contents in configurations:
        if settings is None or contents is None:
            expected_configurations[relative] = None
            continue
        without_ievo, _ = strip_ievo_entries(settings)
        ensure_no_managed_target_references(relative, without_ievo)
        rewritten.append((relative, settings, contents, without_ievo))
        expected_configurations[relative] = without_ievo
    vendor_snapshot = ensure_machine_local_paths_ignored(root)
    for relative, original, original_contents, updated in rewritten:
        if updated != original:
            # The lifecycle lock serializes this helper, not arbitrary editors or
            # upstream tools. This detects changes already visible at the latest
            # portable pre-replacement check; it is not a filesystem-level CAS.
            if current_file_bytes(root, relative) != original_contents:
                raise HookLifecycleError(
                    f"configuration changed during disable: {relative}"
                )
            atomic_write_json(root, relative, updated)

    for relative, expected in expected_configurations.items():
        current = read_json(root, relative, required=relative == CLAUDE_SHARED)
        if current != expected:
            raise HookLifecycleError(
                f"configuration changed before target removal: {relative}"
            )
    ensure_snapshot_unchanged(
        "generated removal targets",
        file_snapshot,
        inspect_regular_removal_files(root, file_removal_targets),
    )
    ensure_snapshot_unchanged(
        "managed vendor tree",
        vendor_snapshot,
        ensure_machine_local_paths_ignored(root),
    )
    ensure_safe_lifecycle_paths(root, removal_targets)
    remove_validated_files(root, file_snapshot)
    remove_validated_vendor(root, vendor_snapshot)


def parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--root",
        type=Path,
        default=ROOT,
        help="repository root (defaults to the checkout containing this script)",
    )
    subparsers = parser.add_subparsers(dest="command", required=True)
    subparsers.add_parser(
        "localize",
        help="move generated Claude iEvo hook entries from shared to local settings",
    )
    check_parser = subparsers.add_parser(
        "check",
        help="validate that enabled iEvo hooks are local and resolvable",
    )
    check_parser.add_argument(
        "--smoke",
        action="store_true",
        help="invoke each configured iEvo hook once with a synthetic no-op payload",
    )
    subparsers.add_parser(
        "disable",
        help="remove local iEvo hook wiring/files while preserving shared intent",
    )
    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    arguments = parse_args(sys.argv[1:] if argv is None else argv)
    root = arguments.root.resolve()
    try:
        if arguments.command == "localize":
            localize(root)
            print("iEvo hook wiring localized")
        elif arguments.command == "check":
            check(root, smoke=arguments.smoke)
            print("iEvo hook lifecycle: valid")
        else:
            disable(root)
            print("local iEvo hooks disabled; shared project intent preserved")
    except (HookLifecycleError, OSError, subprocess.SubprocessError) as error:
        print(f"error: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
