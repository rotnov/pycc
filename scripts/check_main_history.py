#!/usr/bin/env python3
"""Fail closed when a pushed main commit is not associated with a merged PR."""

from __future__ import annotations

import json
import os
import subprocess
from collections.abc import Callable
from typing import Any


ZERO_SHA = "0" * 40
Runner = Callable[..., subprocess.CompletedProcess[str]]
AuditError = tuple[str, str]


def run_command(
    arguments: list[str],
    runner: Runner = subprocess.run,
) -> subprocess.CompletedProcess[str]:
    return runner(
        arguments,
        check=False,
        capture_output=True,
        text=True,
    )


def pushed_commits(
    before: str,
    after: str,
    runner: Runner = subprocess.run,
) -> tuple[list[str], AuditError | None]:
    if before == ZERO_SHA:
        return [after], None

    result = run_command(["git", "rev-list", f"{before}..{after}"], runner)
    if result.returncode != 0:
        return [], (
            "Main history audit unavailable",
            "Could not enumerate pushed commits",
        )
    commits = result.stdout.split()
    if not commits:
        return [], (
            "Main history audit unavailable",
            "No pushed commits were found to audit",
        )
    return commits, None


def merged_main_pr_count(
    repository: str,
    commit_sha: str,
    runner: Runner = subprocess.run,
) -> tuple[int | None, AuditError | None]:
    result = run_command(
        [
            "gh",
            "api",
            "-H",
            "Accept: application/vnd.github+json",
            f"repos/{repository}/commits/{commit_sha}/pulls",
        ],
        runner,
    )
    if result.returncode != 0:
        return None, (
            "Main history audit unavailable",
            f"GitHub API failed while checking {commit_sha}",
        )
    try:
        payload: Any = json.loads(result.stdout)
    except json.JSONDecodeError:
        payload = None
    if not isinstance(payload, list) or not all(
        isinstance(item, dict) and isinstance(item.get("base"), dict)
        for item in payload
    ):
        return None, (
            "Invalid main history audit response",
            f"Expected pull-request associations for {commit_sha}",
        )
    count = sum(
        item.get("merged_at") is not None and item["base"].get("ref") == "main"
        for item in payload
    )
    return count, None


def audit_main_history(
    repository: str,
    before: str,
    after: str,
    runner: Runner = subprocess.run,
) -> list[AuditError]:
    commits, error = pushed_commits(before, after, runner)
    if error is not None:
        return [error]

    failures: list[AuditError] = []
    for commit_sha in commits:
        count, api_error = merged_main_pr_count(repository, commit_sha, runner)
        if api_error is not None:
            failures.append(api_error)
        elif count == 0:
            failures.append(
                (
                    "Unassociated main commit",
                    (
                        f"{commit_sha} is not associated with a merged pull "
                        "request targeting main"
                    ),
                )
            )
    return failures


def main() -> int:
    repository = os.environ.get("AUDIT_REPOSITORY", "")
    before = os.environ.get("AUDIT_BEFORE", "")
    after = os.environ.get("AUDIT_AFTER", "")
    if not repository or not before or not after:
        failures = [
            (
                "Main history audit unavailable",
                "AUDIT_REPOSITORY, AUDIT_BEFORE, and AUDIT_AFTER are required",
            )
        ]
    else:
        failures = audit_main_history(repository, before, after)

    for title, message in failures:
        print(f"::error title={title}::{message}")
    return 1 if failures else 0


if __name__ == "__main__":
    raise SystemExit(main())
