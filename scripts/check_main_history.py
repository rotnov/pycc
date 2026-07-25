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
        return [], (
            "Unexpected main branch creation",
            "A zero before SHA cannot prove the complete introduced history",
        )

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


def merged_main_pr_merge_shas(
    repository: str,
    commit_sha: str,
    runner: Runner = subprocess.run,
) -> tuple[set[str] | None, AuditError | None]:
    result = run_command(
        [
            "gh",
            "api",
            "--paginate",
            "--slurp",
            "-H",
            "Accept: application/vnd.github+json",
            f"repos/{repository}/commits/{commit_sha}/pulls?per_page=100",
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
    if isinstance(payload, list) and all(isinstance(page, list) for page in payload):
        payload = [item for page in payload for item in page]
    valid_payload = isinstance(payload, list) and all(
        isinstance(item, dict)
        and isinstance(item.get("base"), dict)
        and isinstance(item["base"].get("ref"), str)
        and (
            item.get("merged_at") is None
            or (
                isinstance(item.get("merged_at"), str)
                and bool(item["merged_at"])
                and isinstance(item.get("merge_commit_sha"), str)
                and bool(item["merge_commit_sha"])
            )
        )
        for item in payload
    )
    if not valid_payload:
        return None, (
            "Invalid main history audit response",
            f"Expected pull-request associations for {commit_sha}",
        )
    merge_shas = {
        item["merge_commit_sha"]
        for item in payload
        if item.get("merged_at") is not None
        and item["base"].get("ref") == "main"
    }
    return merge_shas, None


def audit_main_history(
    repository: str,
    before: str,
    after: str,
    runner: Runner = subprocess.run,
    *,
    created: bool = False,
    forced: bool = False,
) -> list[AuditError]:
    event_failures: list[AuditError] = []
    if created:
        event_failures.append(
            (
                "Unexpected main branch creation",
                "The protected main branch was reported as newly created",
            )
        )
    if forced:
        event_failures.append(
            (
                "Forced main update",
                "The protected main branch was updated non-fast-forward",
            )
        )
    if event_failures:
        return event_failures

    commits, error = pushed_commits(before, after, runner)
    if error is not None:
        return [error]

    failures: list[AuditError] = []
    pushed_commit_set = set(commits)
    for commit_sha in commits:
        merge_shas, api_error = merged_main_pr_merge_shas(
            repository,
            commit_sha,
            runner,
        )
        if api_error is not None:
            failures.append(api_error)
        elif merge_shas is not None and not merge_shas & pushed_commit_set:
            failures.append(
                (
                    "Uncorrelated main commit",
                    (
                        f"{commit_sha} has no merged-main pull request whose "
                        "merge commit was introduced by this push"
                    ),
                )
            )
    return failures


def main() -> int:
    repository = os.environ.get("AUDIT_REPOSITORY", "")
    before = os.environ.get("AUDIT_BEFORE", "")
    after = os.environ.get("AUDIT_AFTER", "")
    created = os.environ.get("AUDIT_CREATED", "")
    forced = os.environ.get("AUDIT_FORCED", "")
    if (
        not repository
        or not before
        or not after
        or created not in {"true", "false"}
        or forced not in {"true", "false"}
    ):
        failures = [
            (
                "Main history audit unavailable",
                "AUDIT_REPOSITORY, AUDIT_BEFORE, AUDIT_AFTER, AUDIT_CREATED, "
                "and AUDIT_FORCED are required with boolean event flags",
            )
        ]
    else:
        failures = audit_main_history(
            repository,
            before,
            after,
            created=created == "true",
            forced=forced == "true",
        )

    for title, message in failures:
        print(f"::error title={title}::{message}")
    return 1 if failures else 0


if __name__ == "__main__":
    raise SystemExit(main())
