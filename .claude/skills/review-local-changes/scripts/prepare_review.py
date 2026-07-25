#!/usr/bin/env python3
"""Prepare complete, read-only local review scopes as JSON."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import stat
import subprocess
import sys
from pathlib import Path


PINNED_IEVO_COMMIT = "7d5f3e12d0556cb6c5df2974e2babe0433674186"
PINNED_REVIEWER_SHA256 = (
    "b5e11469ba8144686d07eccc3d0759662b9c1bc4c3a6f3d79961dc82f5e53ab2"
)
PINNED_REVIEWER_ID = "ievo@ievo-skills/deep-reviewer"


class ReviewPreparationError(RuntimeError):
    """Raised when a complete safe review cannot be prepared."""


def run_git(repo: Path, *args: str, allow_failure: bool = False) -> bytes:
    result = subprocess.run(
        ["git", "-C", os.fspath(repo), *args],
        check=False,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    if result.returncode != 0 and not allow_failure:
        message = result.stderr.decode("utf-8", errors="replace").strip()
        raise ReviewPreparationError(message or f"git {' '.join(args)} failed")
    return result.stdout if result.returncode == 0 else b""


def safe_relative_path(relative_path: str) -> Path:
    relative = Path(relative_path)
    if (
        not relative.parts
        or relative == Path(".")
        or relative.is_absolute()
        or ".." in relative.parts
    ):
        raise ReviewPreparationError(
            f"unsafe repository-relative path: {relative_path!r}"
        )
    return relative


def read_regular_file_without_symlinks(repo: Path, relative_path: str) -> bytes:
    relative = safe_relative_path(relative_path)
    if (
        os.open not in os.supports_dir_fd
        or not hasattr(os, "O_DIRECTORY")
        or not hasattr(os, "O_NOFOLLOW")
    ):
        raise ReviewPreparationError(
            "this platform cannot safely open the repository pin without "
            "following path components"
        )

    directory_flags = os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW
    directory_descriptor = -1
    file_descriptor = -1
    try:
        directory_descriptor = os.open(repo, directory_flags)
        for component in relative.parts[:-1]:
            child_descriptor = -1
            try:
                child_descriptor = os.open(
                    component,
                    directory_flags,
                    dir_fd=directory_descriptor,
                )
                if not stat.S_ISDIR(os.fstat(child_descriptor).st_mode):
                    raise ReviewPreparationError(
                        f"path component is not a directory: {component!r}"
                    )
            except Exception:
                if child_descriptor >= 0:
                    os.close(child_descriptor)
                raise
            os.close(directory_descriptor)
            directory_descriptor = child_descriptor

        file_descriptor = os.open(
            relative.parts[-1],
            os.O_RDONLY | os.O_NOFOLLOW,
            dir_fd=directory_descriptor,
        )
        if not stat.S_ISREG(os.fstat(file_descriptor).st_mode):
            os.close(file_descriptor)
            file_descriptor = -1
            raise ReviewPreparationError(
                f"path must be a regular non-symlink file: {relative_path!r}"
            )
        file = os.fdopen(file_descriptor, "rb")
        file_descriptor = -1
        with file:
            return file.read()
    except OSError as error:
        raise ReviewPreparationError(
            f"could not safely read {relative_path!r}: {error}"
        ) from error
    finally:
        if file_descriptor >= 0:
            os.close(file_descriptor)
        if directory_descriptor >= 0:
            os.close(directory_descriptor)


def validate_repository_pin(repo: Path) -> None:
    try:
        raw = read_regular_file_without_symlinks(
            repo,
            ".agents/plugins/marketplace.json",
        )
        marketplace = json.loads(raw.decode("utf-8"))
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        raise ReviewPreparationError(
            f"could not validate the repository iEvo pin: {error}"
        ) from error

    matches = [
        plugin
        for plugin in marketplace.get("plugins", [])
        if isinstance(plugin, dict) and plugin.get("name") == "ievo"
    ]
    if len(matches) != 1:
        raise ReviewPreparationError(
            "repository marketplace must contain exactly one iEvo pin"
        )
    source = matches[0].get("source")
    if not isinstance(source, dict) or source.get("ref") != PINNED_IEVO_COMMIT:
        raise ReviewPreparationError(
            "repository iEvo pin does not match the reviewed reviewer artifact"
        )


def validate_reviewer_manifest(
    path: Path,
    expected_sha256: str = PINNED_REVIEWER_SHA256,
) -> str:
    try:
        if path.is_symlink() or not path.is_file():
            raise ReviewPreparationError(
                "reviewer manifest must be a regular non-symlink file"
            )
        digest = hashlib.sha256(path.read_bytes()).hexdigest()
    except OSError as error:
        raise ReviewPreparationError(
            f"reviewer manifest is unavailable: {error}"
        ) from error
    if digest != expected_sha256:
        raise ReviewPreparationError(
            "reviewer manifest does not match the pinned reviewed artifact"
        )
    return digest


def decode_path_list(raw: bytes) -> list[str]:
    return [os.fsdecode(item) for item in raw.split(b"\0") if item]


def text_diff(raw: bytes) -> str:
    return raw.decode("utf-8", errors="replace")


def git_blob_text(repo: Path, object_id: str) -> str:
    return run_git(repo, "cat-file", "blob", object_id).decode(
        "utf-8",
        errors="replace",
    )


def path_and_ancestors(relative_paths: list[str]) -> list[str]:
    candidates: set[str] = set()
    for relative_path in relative_paths:
        parts = safe_relative_path(relative_path).parts
        candidates.add(relative_path)
        for end in range(1, len(parts)):
            candidates.add(Path(*parts[:end]).as_posix())
    return sorted(candidates)


def tree_component_metadata(
    repo: Path,
    relative_path: str,
    entries: dict[str, tuple[str, str, str]],
) -> dict[str, str] | None:
    parts = safe_relative_path(relative_path).parts
    for end in range(1, len(parts)):
        component = Path(*parts[:end]).as_posix()
        entry = entries.get(component)
        if entry is None:
            continue
        mode, object_type, object_id = entry
        if mode == "040000" and object_type == "tree":
            continue
        if mode == "120000" and object_type == "blob":
            return {
                "path": relative_path,
                "kind": "symlink-component",
                "component": component,
                "target": git_blob_text(repo, object_id),
            }
        if mode == "160000" and object_type == "commit":
            return {
                "path": relative_path,
                "kind": "gitlink-component",
                "component": component,
                "target": object_id,
            }
        return {
            "path": relative_path,
            "kind": "non-directory-component",
            "component": component,
        }
    return None


def classify_tree_paths(
    repo: Path,
    relative_paths: list[str],
    treeish: str,
) -> tuple[list[str], list[dict[str, str]]]:
    if not relative_paths:
        return [], []
    records = run_git(
        repo,
        "--literal-pathspecs",
        "ls-tree",
        "-z",
        treeish,
        "--",
        *path_and_ancestors(relative_paths),
    )
    entries: dict[str, tuple[str, str, str]] = {}
    for record in records.split(b"\0"):
        if not record:
            continue
        header, raw_path = record.split(b"\t", 1)
        mode, object_type, object_id = header.decode("ascii").split()
        entries[os.fsdecode(raw_path)] = (mode, object_type, object_id)

    regular: list[str] = []
    excluded: list[dict[str, str]] = []
    for relative_path in relative_paths:
        safe_relative_path(relative_path)
        component_metadata = tree_component_metadata(
            repo,
            relative_path,
            entries,
        )
        if component_metadata is not None:
            excluded.append(component_metadata)
            continue
        entry = entries.get(relative_path)
        if entry is None:
            excluded.append({"path": relative_path, "kind": "deleted"})
            continue
        mode, object_type, object_id = entry
        if mode in {"100644", "100755"} and object_type == "blob":
            regular.append(relative_path)
        elif mode == "120000" and object_type == "blob":
            excluded.append(
                {
                    "path": relative_path,
                    "kind": "symlink",
                    "target": git_blob_text(repo, object_id),
                }
            )
        elif mode == "160000" and object_type == "commit":
            excluded.append(
                {
                    "path": relative_path,
                    "kind": "gitlink",
                    "target": object_id,
                }
            )
        else:
            raise ReviewPreparationError(
                f"unsupported Git tree mode {mode!r} for {relative_path!r}"
            )
    return regular, excluded


def classify_index_paths(
    repo: Path,
    relative_paths: list[str],
) -> tuple[list[str], list[dict[str, str]]]:
    if not relative_paths:
        return [], []
    records = run_git(
        repo,
        "--literal-pathspecs",
        "ls-files",
        "--stage",
        "-z",
        "--",
        *path_and_ancestors(relative_paths),
    )
    entries: dict[str, tuple[str, str]] = {}
    for record in records.split(b"\0"):
        if not record:
            continue
        header, raw_path = record.split(b"\t", 1)
        mode, object_id, stage = header.decode("ascii").split()
        if stage != "0":
            raise ReviewPreparationError(
                f"unmerged index entry for {os.fsdecode(raw_path)!r}"
            )
        entries[os.fsdecode(raw_path)] = (mode, object_id)

    regular: list[str] = []
    excluded: list[dict[str, str]] = []
    component_entries = {
        path: (mode, "commit" if mode == "160000" else "blob", object_id)
        for path, (mode, object_id) in entries.items()
    }
    for relative_path in relative_paths:
        safe_relative_path(relative_path)
        component_metadata = tree_component_metadata(
            repo,
            relative_path,
            component_entries,
        )
        if component_metadata is not None:
            excluded.append(component_metadata)
            continue
        entry = entries.get(relative_path)
        if entry is None:
            excluded.append({"path": relative_path, "kind": "deleted"})
            continue
        mode, object_id = entry
        if mode in {"100644", "100755"}:
            regular.append(relative_path)
        elif mode == "120000":
            excluded.append(
                {
                    "path": relative_path,
                    "kind": "symlink",
                    "target": git_blob_text(repo, object_id),
                }
            )
        elif mode == "160000":
            excluded.append(
                {
                    "path": relative_path,
                    "kind": "gitlink",
                    "target": object_id,
                }
            )
        else:
            raise ReviewPreparationError(
                f"unsupported Git index mode {mode!r} for {relative_path!r}"
            )
    return regular, excluded


def classify_worktree_path(
    repo: Path,
    relative_path: str,
) -> tuple[bool, dict[str, str] | None]:
    relative = safe_relative_path(relative_path)
    path = repo
    for index, component in enumerate(relative.parts[:-1]):
        path /= component
        try:
            mode = path.lstat().st_mode
        except FileNotFoundError:
            return False, {"path": relative_path, "kind": "deleted"}
        except OSError as error:
            raise ReviewPreparationError(
                f"could not inspect {relative_path!r}: {error}"
            ) from error
        if stat.S_ISLNK(mode):
            return False, {
                "path": relative_path,
                "kind": "symlink-component",
                "component": os.fspath(Path(*relative.parts[: index + 1])),
                "target": os.readlink(path),
            }
        if not stat.S_ISDIR(mode):
            raise ReviewPreparationError(
                f"non-directory path component for {relative_path!r}"
            )

    path /= relative.parts[-1]
    try:
        mode = path.lstat().st_mode
    except FileNotFoundError:
        return False, {"path": relative_path, "kind": "deleted"}
    except OSError as error:
        raise ReviewPreparationError(
            f"could not inspect working-tree path {relative_path!r}: {error}"
        ) from error
    if stat.S_ISREG(mode):
        return True, None
    if stat.S_ISLNK(mode):
        return False, {
            "path": relative_path,
            "kind": "symlink",
            "target": os.readlink(path),
        }
    if stat.S_ISDIR(mode):
        return False, {"path": relative_path, "kind": "directory"}
    raise ReviewPreparationError(
        f"unsupported working-tree file type at {relative_path!r}"
    )


def classify_worktree_paths(
    repo: Path,
    relative_paths: list[str],
) -> tuple[list[str], list[dict[str, str]]]:
    regular: list[str] = []
    excluded: list[dict[str, str]] = []
    for relative_path in sorted(set(relative_paths)):
        is_regular, entry = classify_worktree_path(repo, relative_path)
        if is_regular:
            regular.append(relative_path)
        elif entry is not None:
            excluded.append(entry)
    return regular, excluded


def resolve_default_ref(repo: Path, explicit: str | None) -> str:
    if explicit is not None:
        return explicit
    raw = run_git(
        repo,
        "symbolic-ref",
        "--quiet",
        "--short",
        "refs/remotes/origin/HEAD",
        allow_failure=True,
    )
    resolved = text_diff(raw).strip()
    if not resolved:
        raise ReviewPreparationError(
            "could not resolve the remote default branch from origin/HEAD"
        )
    return resolved


def append_scope(
    scopes: list[dict[str, object]],
    name: str,
    diff: str,
    files: list[str],
    excluded_entries: list[dict[str, str]] | None = None,
) -> None:
    entries = excluded_entries or []
    if diff or files or entries:
        scopes.append(
            {
                "name": name,
                "changed_files": sorted(set(files)),
                "diff": diff,
                "excluded_entries": entries,
            }
        )


def prepare_review(
    repo: Path,
    reviewer_manifest: Path,
    default_ref: str | None = None,
    expected_reviewer_sha256: str = PINNED_REVIEWER_SHA256,
) -> dict[str, object]:
    repo = repo.resolve()
    top_level = Path(
        os.fsdecode(run_git(repo, "rev-parse", "--show-toplevel")).strip()
    ).resolve()
    if top_level != repo:
        raise ReviewPreparationError(
            f"review repository must be its Git top level: {top_level}"
        )
    validate_repository_pin(repo)
    reviewer_digest = validate_reviewer_manifest(
        reviewer_manifest,
        expected_reviewer_sha256,
    )

    scopes: list[dict[str, object]] = []
    resolved_default = resolve_default_ref(repo, default_ref)
    merge_base = text_diff(
        run_git(
            repo,
            "merge-base",
            "HEAD",
            resolved_default,
            allow_failure=True,
        )
    ).strip()
    if not merge_base:
        raise ReviewPreparationError(
            f"could not resolve a merge base with {resolved_default!r}"
        )
    committed_diff = text_diff(
        run_git(
            repo,
            "diff",
            "--no-ext-diff",
            "--no-textconv",
            "--binary",
            f"{merge_base}..HEAD",
        )
    )
    committed_files = decode_path_list(
        run_git(
            repo,
            "diff",
            "--no-ext-diff",
            "--no-textconv",
            "--name-only",
            "-z",
            f"{merge_base}..HEAD",
        )
    )
    committed_regular, committed_excluded = classify_tree_paths(
        repo,
        committed_files,
        "HEAD",
    )
    append_scope(
        scopes,
        "committed",
        committed_diff,
        committed_regular,
        committed_excluded,
    )

    staged_diff = text_diff(
        run_git(
            repo,
            "diff",
            "--staged",
            "--no-ext-diff",
            "--no-textconv",
            "--binary",
        )
    )
    staged_files = decode_path_list(
        run_git(
            repo,
            "diff",
            "--staged",
            "--no-ext-diff",
            "--no-textconv",
            "--name-only",
            "-z",
        )
    )
    staged_regular, staged_excluded = classify_index_paths(repo, staged_files)
    append_scope(
        scopes,
        "staged",
        staged_diff,
        staged_regular,
        staged_excluded,
    )

    working_diff = text_diff(
        run_git(
            repo,
            "diff",
            "--no-ext-diff",
            "--no-textconv",
            "--binary",
        )
    )
    working_files = decode_path_list(
        run_git(
            repo,
            "diff",
            "--no-ext-diff",
            "--no-textconv",
            "--name-only",
            "-z",
        )
    )
    untracked_files = decode_path_list(
        run_git(repo, "ls-files", "--others", "--exclude-standard", "-z")
    )
    working_regular, working_excluded = classify_worktree_paths(
        repo,
        [*working_files, *untracked_files],
    )
    append_scope(
        scopes,
        "working",
        working_diff,
        working_regular,
        working_excluded,
    )

    return {
        "reviewer": {
            "id": PINNED_REVIEWER_ID,
            "plugin_commit": PINNED_IEVO_COMMIT,
            "sha256": reviewer_digest,
        },
        "default_ref": resolved_default,
        "scopes": scopes,
    }


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--repo", type=Path, required=True)
    parser.add_argument("--reviewer-manifest", type=Path, required=True)
    parser.add_argument("--default-ref")
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    try:
        result = prepare_review(
            args.repo,
            args.reviewer_manifest,
            args.default_ref,
        )
    except ReviewPreparationError as error:
        print(f"error: {error}", file=sys.stderr)
        return 2
    json.dump(result, sys.stdout, ensure_ascii=True)
    sys.stdout.write("\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
