#!/usr/bin/env python3
"""Classify whether a Codex review request/retry is allowed for a PR head."""

from __future__ import annotations

import argparse
import datetime as dt
import json
import subprocess
import sys
from typing import Any


CODEX_LOGIN = "chatgpt-codex-connector"
AUTHORIZED_REQUEST_ASSOCIATIONS = {"OWNER", "MEMBER", "COLLABORATOR"}


def timestamp(value: str) -> dt.datetime:
    return dt.datetime.fromisoformat(value.replace("Z", "+00:00"))


def is_codex_login(login: str | None) -> bool:
    return (login or "").removesuffix("[bot]") == CODEX_LOGIN


def classify(payload: dict[str, Any], now: dt.datetime) -> tuple[str, str]:
    head = payload["headRefOid"]

    reviews = [
        review
        for review in payload.get("reviews", [])
        if is_codex_login(review.get("author", {}).get("login"))
        and review.get("commit", {}).get("oid") == head
    ]
    if reviews:
        return "ARTIFACT_EXISTS", f"Codex review already exists for {head}"

    checks = [
        check
        for check in payload.get("statusCheckRollup", [])
        if "codex" in (check.get("name") or "").lower()
    ]
    if checks:
        if any(
            check.get("status") in {"QUEUED", "IN_PROGRESS", "PENDING"}
            for check in checks
        ):
            return "WAIT", f"Codex check is already active for {head}"
        return "ARTIFACT_EXISTS", f"Codex check already exists for {head}"

    comments = sorted(
        payload.get("comments", []),
        key=lambda comment: timestamp(comment["createdAt"]),
    )
    requests = [
        comment
        for comment in comments
        if comment.get("body", "").strip() == "@codex review"
        and comment.get("headRefOid") == head
        and comment.get("authorAssociation") in AUTHORIZED_REQUEST_ASSOCIATIONS
    ]
    if not requests:
        return "REQUEST_ALLOWED", f"no request exists for {head}"
    if len(requests) >= 2:
        return (
            "RETRY_LIMIT_REACHED",
            f"{len(requests)} attempts already exist for {head}",
        )

    request = requests[0]
    request_time = timestamp(request["createdAt"])
    if now - request_time >= dt.timedelta(minutes=15):
        evidence = request.get("url") or request.get("createdAt")
        return "RETRY_ALLOWED", f"15-minute no-artifact timeout after {evidence}"
    return "WAIT", f"request exists for {head}; no retry evidence yet"


def run_gh_json(arguments: list[str]) -> Any:
    result = subprocess.run(
        ["gh", *arguments],
        check=True,
        capture_output=True,
        text=True,
    )
    return json.loads(result.stdout)


def flatten_pages(pages: list[Any], object_key: str | None = None) -> list[Any]:
    items: list[Any] = []
    for page in pages:
        page_items = page.get(object_key, []) if object_key else page
        if not isinstance(page_items, list):
            raise ValueError("unexpected paginated GitHub response")
        items.extend(page_items)
    return items


def paginated_rest(endpoint: str, object_key: str | None = None) -> list[Any]:
    pages = run_gh_json(
        [
            "api",
            "--paginate",
            "--slurp",
            "-H",
            "Accept: application/vnd.github+json",
            endpoint,
        ]
    )
    if not isinstance(pages, list):
        raise ValueError("paginated GitHub response must be a list")
    return flatten_pages(pages, object_key)


def timeline_comments(events: list[dict[str, Any]]) -> list[dict[str, Any]]:
    """Attach each issue comment to the PR head current at that event."""
    current_head: str | None = None
    comments: list[dict[str, Any]] = []
    for event in events:
        event_type = event.get("event")
        if event_type == "committed":
            current_head = event.get("sha")
        elif event_type == "head_ref_force_pushed":
            current_head = event.get("after_commit", {}).get("sha")
        elif event_type == "commented":
            comments.append(
                {
                    "author": {"login": event.get("user", {}).get("login")},
                    "authorAssociation": event.get("author_association"),
                    "body": event.get("body", ""),
                    "createdAt": event["created_at"],
                    "url": event.get("html_url"),
                    "headRefOid": current_head,
                }
            )
    return comments


def load_pr(repository: str, number: int) -> dict[str, Any]:
    payload = run_gh_json(
        [
            "pr",
            "view",
            str(number),
            "--repo",
            repository,
            "--json",
            "headRefOid,statusCheckRollup",
        ],
    )
    timeline = paginated_rest(
        f"repos/{repository}/issues/{number}/timeline?per_page=100"
    )
    payload["comments"] = timeline_comments(timeline)
    reviews = paginated_rest(f"repos/{repository}/pulls/{number}/reviews?per_page=100")
    payload["reviews"] = [
        {
            "author": {"login": review.get("user", {}).get("login")},
            "commit": {"oid": review.get("commit_id")},
        }
        for review in reviews
    ]
    check_runs = paginated_rest(
        f"repos/{repository}/commits/{payload['headRefOid']}/check-runs?per_page=100",
        "check_runs",
    )
    status_rollup = payload.get("statusCheckRollup")
    if not isinstance(status_rollup, list):
        status_rollup = []
        payload["statusCheckRollup"] = status_rollup
    status_rollup.extend(
        {
            "name": check.get("name"),
            "status": check.get("status", "").upper(),
        }
        for check in check_runs
    )
    return payload


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("repository")
    parser.add_argument("pull_request", type=int)
    arguments = parser.parse_args()
    try:
        payload = load_pr(arguments.repository, arguments.pull_request)
        state, evidence = classify(payload, dt.datetime.now(dt.UTC))
    except (OSError, subprocess.CalledProcessError, KeyError, ValueError) as error:
        print(f"ERROR: {error}", file=sys.stderr)
        return 2
    print(f"{state}: {evidence}")
    return 0 if state in {"REQUEST_ALLOWED", "RETRY_ALLOWED"} else 1


if __name__ == "__main__":
    raise SystemExit(main())
