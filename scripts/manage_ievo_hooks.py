#!/usr/bin/env python3
"""Keep iEvo's generated hook wiring machine-local for this repository."""

from __future__ import annotations

import argparse
import copy
import json
import os
import shutil
import stat
import subprocess
import sys
import tempfile
from collections.abc import Iterable
from pathlib import Path
from typing import Any


ROOT = Path(__file__).resolve().parents[1]
CLAUDE_SHARED = Path(".claude/settings.json")
CLAUDE_LOCAL = Path(".claude/settings.local.json")
CODEX_LOCAL = Path(".codex/hooks.json")
FLAG = Path(".ievo/evo-auto.flag")
SCRIPT_DIRECTORY = Path(".ievo/hooks/scripts")
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


def ensure_machine_local_paths_ignored(root: Path) -> None:
    paths = [
        *REQUIRED_IGNORE_LINES[:2],
        *(target.as_posix() for target in SCRIPT_TARGETS.values()),
        *(target.as_posix() for target in LOCAL_COMPANIONS),
        VENDOR_DIRECTORY.as_posix(),
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


def managed_target_references(settings: dict[str, Any]) -> list[ManagedReference]:
    hooks = settings.get("hooks")
    if not isinstance(hooks, dict):
        return []

    references: list[ManagedReference] = []
    managed_targets = (
        *SCRIPT_TARGETS.values(),
        *LOCAL_COMPANIONS,
        VENDOR_DIRECTORY,
    )
    target_texts = {target.as_posix(): target for target in managed_targets}
    for event, value in hooks.items():
        if not isinstance(event, str):
            continue
        values = tuple(string_values(value))
        for target_text, target in target_texts.items():
            if any(target_text in candidate for candidate in values):
                references.append((event, target))
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
    if root.is_symlink():
        raise HookLifecycleError(f"managed path root must not be a symlink: {root}")

    current = root
    for index, component in enumerate(relative.parts):
        current /= component
        if current.is_symlink():
            raise HookLifecycleError(
                "managed path contains a symlink component: "
                f"{current.relative_to(root)}"
            )
        if current.exists() and index < len(relative.parts) - 1:
            if not current.is_dir():
                raise HookLifecycleError(
                    "managed path ancestor must be a directory: "
                    f"{current.relative_to(root)}"
                )


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


def localize(root: Path) -> None:
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
    if not flag_enabled(root):
        raise HookLifecycleError(".ievo/evo-auto.flag does not state enabled: true")
    rewritten_local = add_records(stripped_local, claude_records)
    original_gitignore = (root / GITIGNORE).read_text(encoding="utf-8")
    rewritten_gitignore = normalized_gitignore(root)

    if rewritten_gitignore != original_gitignore:
        atomic_write_text(root, GITIGNORE, rewritten_gitignore)
    if rewritten_local != local and (local_value is not None or claude_records):
        atomic_write_json(root, CLAUDE_LOCAL, rewritten_local)
    if rewritten_shared != shared:
        atomic_write_json(root, CLAUDE_SHARED, rewritten_shared)
    ensure_machine_local_paths_ignored(root)


def flag_enabled(root: Path) -> bool:
    path = root / FLAG
    if not path.exists():
        return False
    for line in path.read_text(encoding="utf-8").splitlines():
        key, separator, value = line.partition(":")
        if separator and key.strip() == "enabled":
            return value.strip() == "true"
    return False


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


def check(root: Path, *, smoke: bool) -> None:
    shared = read_json(root, CLAUDE_SHARED, required=True)
    assert shared is not None
    without_shared_ievo, shared_records = strip_ievo_entries(shared)
    ensure_no_managed_target_references(CLAUDE_SHARED, without_shared_ievo)
    if shared_records:
        raise HookLifecycleError(
            "iEvo hook entries remain in shared .claude/settings.json; run localize"
        )
    if not flag_enabled(root):
        raise HookLifecycleError(".ievo/evo-auto.flag does not state enabled: true")

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


def remove_path(path: Path) -> None:
    if path.is_symlink() or path.is_file():
        path.unlink()
    elif path.is_dir():
        shutil.rmtree(path)


def disable(root: Path) -> None:
    removal_targets = [
        *SCRIPT_TARGETS.values(),
        *LOCAL_COMPANIONS,
        VENDOR_DIRECTORY,
    ]
    for target in removal_targets:
        ensure_no_symlink_components(root, target)

    configurations: list[tuple[Path, dict[str, Any]]] = []
    for relative in (CLAUDE_SHARED, CLAUDE_LOCAL, CODEX_LOCAL):
        settings = read_json(root, relative, required=relative == CLAUDE_SHARED)
        if settings is not None:
            configurations.append((relative, settings))

    rewritten: list[tuple[Path, dict[str, Any], dict[str, Any]]] = []
    for relative, settings in configurations:
        without_ievo, _ = strip_ievo_entries(settings)
        ensure_no_managed_target_references(relative, without_ievo)
        rewritten.append((relative, settings, without_ievo))
    for relative, original, updated in rewritten:
        if updated != original:
            atomic_write_json(root, relative, updated)

    for target in removal_targets:
        path = root / target
        if path.exists():
            remove_path(path)


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
