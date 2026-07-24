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
FAILURE_TERMS = (
    "create an environment for this repo",
    "not configured",
    "could not start",
    "couldn't start",
    "did not start",
    "failed",
    "unable",
    "rate limit",
    "rejected",
    "error",
)


def timestamp(value: str) -> dt.datetime:
    return dt.datetime.fromisoformat(value.replace("Z", "+00:00"))


def classify(payload: dict[str, Any], now: dt.datetime) -> tuple[str, str]:
    head = payload["headRefOid"]
    commits = payload.get("commits", [])
    head_time = (
        max(timestamp(commit["committedDate"]) for commit in commits)
        if commits
        else dt.datetime.min.replace(tzinfo=dt.UTC)
    )

    reviews = [
        review
        for review in payload.get("reviews", [])
        if review.get("author", {}).get("login") == CODEX_LOGIN
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
        and timestamp(comment["createdAt"]) >= head_time
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
    codex_responses = [
        comment
        for comment in comments
        if timestamp(comment["createdAt"]) > request_time
        and comment.get("author", {}).get("login") == CODEX_LOGIN
    ]
    for response in codex_responses:
        body = response.get("body", "").lower()
        if any(term in body for term in FAILURE_TERMS):
            evidence = response.get("url") or response.get("createdAt")
            return "RETRY_ALLOWED", f"explicit pre-start failure: {evidence}"

    if codex_responses:
        evidence = codex_responses[0].get("url") or codex_responses[0].get("createdAt")
        return "ARTIFACT_EXISTS", f"Codex responded to the request: {evidence}"

    if now - request_time >= dt.timedelta(minutes=15):
        evidence = request.get("url") or request.get("createdAt")
        return "RETRY_ALLOWED", f"15-minute no-artifact timeout after {evidence}"
    return "WAIT", f"request exists for {head}; no retry evidence yet"


def load_pr(repository: str, number: int) -> dict[str, Any]:
    result = subprocess.run(
        [
            "gh",
            "pr",
            "view",
            str(number),
            "--repo",
            repository,
            "--json",
            "headRefOid,commits,comments,reviews,statusCheckRollup",
        ],
        check=True,
        capture_output=True,
        text=True,
    )
    return json.loads(result.stdout)


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
