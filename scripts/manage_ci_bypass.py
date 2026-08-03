#!/usr/bin/env python3
"""Session-driven CI temporary-bypass lifecycle management.

See docs/superpowers/specs/2026-08-02-ci-temporary-bypass-mechanism-design.md
and docs/REPOSITORY_GOVERNANCE.md's "Session-driven temporary bypass" section
for the full design and the safety properties this script must preserve.
"""

from __future__ import annotations

import argparse
import json
import subprocess
import sys
from datetime import datetime, timedelta, timezone
from pathlib import Path

REPO = "rotnov/pycc"
BASELINE_CONTEXTS = ["audit", "ci-gate"]
INCIDENT_TITLE_PREFIX = "[ci-bypass]"
DEFAULT_EXPIRY_MINUTES = 60


class CiBypassError(Exception):
    """Raised for any fail-closed condition in this lifecycle."""


def run_gh(args: list[str], input_text: str | None = None) -> str:
    result = subprocess.run(
        ["gh"] + args, input=input_text, capture_output=True, text=True
    )
    if result.returncode != 0:
        raise CiBypassError(
            f"gh {' '.join(args)} failed (exit {result.returncode}): "
            f"{result.stderr.strip()}"
        )
    return result.stdout


def get_protection(repo: str = REPO) -> dict:
    output = run_gh(["api", f"repos/{repo}/branches/main/protection"])
    return json.loads(output)


def required_contexts(protection: dict) -> list[str]:
    return protection["required_status_checks"]["contexts"]


def status(repo: str = REPO) -> tuple[bool, str]:
    protection = get_protection(repo)
    current = sorted(required_contexts(protection))
    baseline = sorted(BASELINE_CONTEXTS)
    if current == baseline:
        return True, f"Branch protection matches baseline: {current}"
    return False, (
        f"Branch protection DRIFT: current required checks {current} "
        f"!= baseline {baseline}"
    )


FAILING_CONCLUSIONS = {"FAILURE", "ACTION_REQUIRED", "TIMED_OUT"}
SNAPSHOT_MARKER_START = "<!-- ci-bypass-snapshot"
SNAPSHOT_MARKER_END = "-->"


def check_conclusion(repo: str, pr_number: int, check_name: str) -> str | None:
    output = run_gh(
        ["pr", "view", str(pr_number), "--repo", repo, "--json", "statusCheckRollup"]
    )
    data = json.loads(output)
    for entry in data.get("statusCheckRollup", []):
        if entry.get("name") == check_name:
            return entry.get("conclusion")
    return None


def find_open_bypass_issue(repo: str = REPO) -> int | None:
    output = run_gh(
        [
            "issue", "list", "--repo", repo,
            "--search", f'"{INCIDENT_TITLE_PREFIX}" in:title',
            "--state", "open", "--json", "number,title",
        ]
    )
    for issue in json.loads(output):
        if issue["title"].startswith(INCIDENT_TITLE_PREFIX):
            return issue["number"]
    return None


def build_incident_body(
    check_name: str, reason: str, evidence_text: str, snapshot: dict,
    expiry_minutes: int, expiry_timestamp: str,
) -> str:
    snapshot_json = json.dumps(snapshot, indent=2, sort_keys=True)
    return (
        f"**Check relaxed:** `{check_name}`\n\n"
        f"**Reason:** {reason}\n\n"
        f"**Expiry:** {expiry_timestamp} ({expiry_minutes} minutes from creation)\n\n"
        f"**Gate 1 verdict (CONFIRMED):**\n\n{evidence_text}\n\n"
        f"{SNAPSHOT_MARKER_START}\n{snapshot_json}\n{SNAPSHOT_MARKER_END}\n"
    )


def create_incident_issue(
    repo: str, check_name: str, reason: str, evidence_text: str, snapshot: dict,
    expiry_minutes: int, expiry_timestamp: str, body_path: Path,
) -> int:
    title = f"{INCIDENT_TITLE_PREFIX} {check_name} relaxed — {reason}"
    body = build_incident_body(
        check_name, reason, evidence_text, snapshot, expiry_minutes, expiry_timestamp
    )
    body_path.write_text(body, encoding="utf-8")
    output = run_gh(
        ["issue", "create", "--repo", repo, "--title", title, "-F", str(body_path)]
    )
    url = output.strip().splitlines()[-1]
    return int(url.rstrip("/").rsplit("/", 1)[-1])


def patch_required_status_checks(repo: str, strict: bool, contexts: list[str]) -> None:
    body = json.dumps({"strict": strict, "contexts": contexts})
    run_gh(
        [
            "api", "-X", "PATCH",
            f"repos/{repo}/branches/main/protection/required_status_checks",
            "--input", "-",
        ],
        input_text=body,
    )


def relax(
    repo: str, check_name: str, reason: str, evidence_path: Path, pr_number: int,
    expiry_minutes: int, state_path: Path, body_path: Path,
    now: datetime | None = None,
) -> int:
    if find_open_bypass_issue(repo) is not None:
        raise CiBypassError(
            "a [ci-bypass] incident is already open; this mechanism cannot stack"
        )
    conclusion = check_conclusion(repo, pr_number, check_name)
    if conclusion not in FAILING_CONCLUSIONS:
        raise CiBypassError(
            f"check {check_name!r} on PR #{pr_number} is not currently failing "
            f"(conclusion={conclusion!r}); refusing to relax a check that isn't stuck"
        )
    protection = get_protection(repo)
    contexts = required_contexts(protection)
    if check_name not in contexts:
        raise CiBypassError(f"check {check_name!r} is not currently a required check")
    strict = protection["required_status_checks"]["strict"]
    snapshot = {"strict": strict, "contexts": list(contexts)}
    evidence_text = evidence_path.read_text(encoding="utf-8")
    if now is None:
        now = datetime.now(timezone.utc)
    expiry_timestamp = (now + timedelta(minutes=expiry_minutes)).strftime(
        "%Y-%m-%dT%H:%M:%SZ"
    )
    issue_number = create_incident_issue(
        repo, check_name, reason, evidence_text, snapshot,
        expiry_minutes, expiry_timestamp, body_path,
    )
    remaining = [c for c in contexts if c != check_name]
    patch_required_status_checks(repo, strict, remaining)
    state_path.write_text(
        json.dumps({"incident": issue_number, "snapshot": snapshot}), encoding="utf-8"
    )
    return issue_number


def parse_snapshot_from_body(body: str) -> dict:
    start = body.find(SNAPSHOT_MARKER_START)
    if start == -1:
        raise CiBypassError("incident issue body has no embedded snapshot marker")
    json_start = start + len(SNAPSHOT_MARKER_START)
    end = body.find(SNAPSHOT_MARKER_END, json_start)
    if end == -1:
        raise CiBypassError("incident issue body's snapshot marker is not closed")
    raw = body[json_start:end].strip()
    try:
        return json.loads(raw)
    except json.JSONDecodeError as error:
        raise CiBypassError(
            f"incident issue's embedded snapshot is not valid JSON: {error}"
        ) from error


def get_incident_body(repo: str, issue_number: int) -> str:
    output = run_gh(
        ["issue", "view", str(issue_number), "--repo", repo, "--json", "body,state"]
    )
    data = json.loads(output)
    if data["state"] != "OPEN":
        raise CiBypassError(
            f"incident #{issue_number} is not open (state={data['state']!r})"
        )
    return data["body"]


def restore(repo: str, issue_number: int, comment_path: Path) -> dict:
    body = get_incident_body(repo, issue_number)
    snapshot = parse_snapshot_from_body(body)
    patch_required_status_checks(repo, snapshot["strict"], snapshot["contexts"])
    protection = get_protection(repo)
    readback = {
        "strict": protection["required_status_checks"]["strict"],
        "contexts": sorted(required_contexts(protection)),
    }
    expected = {"strict": snapshot["strict"], "contexts": sorted(snapshot["contexts"])}
    if readback != expected:
        raise CiBypassError(
            f"DRIFT after restore: expected {expected}, readback {readback} "
            f"-- this is a release-blocking governance incident, do not close "
            f"#{issue_number}"
        )
    comment = (
        "Restore verified. Readback matches pre-relax snapshot exactly:\n\n"
        f"```json\n{json.dumps(readback, indent=2, sort_keys=True)}\n```\n"
    )
    comment_path.write_text(comment, encoding="utf-8")
    run_gh(["issue", "comment", str(issue_number), "--repo", repo, "-F", str(comment_path)])
    run_gh(["issue", "close", str(issue_number), "--repo", repo, "-r", "completed"])
    return readback


def main(argv: list[str] | None = None) -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo", default=REPO)
    parser.add_argument(
        "--state-dir", type=Path, default=Path(".ci-bypass-state"),
        help="local ephemeral working directory (gitignored)",
    )
    subparsers = parser.add_subparsers(dest="command", required=True)
    subparsers.add_parser("status")

    relax_parser = subparsers.add_parser("relax")
    relax_parser.add_argument("--check", required=True)
    relax_parser.add_argument("--reason", required=True)
    relax_parser.add_argument("--evidence", required=True, type=Path)
    relax_parser.add_argument("--pr", required=True, type=int)
    relax_parser.add_argument(
        "--expiry-minutes", type=int, default=DEFAULT_EXPIRY_MINUTES
    )

    restore_parser = subparsers.add_parser("restore")
    restore_parser.add_argument("--incident", required=True, type=int)

    args = parser.parse_args(argv)
    args.state_dir.mkdir(parents=True, exist_ok=True)

    try:
        if args.command == "status":
            ok, message = status(args.repo)
            print(message)
            sys.exit(0 if ok else 1)
        elif args.command == "relax":
            issue_number = relax(
                args.repo, args.check, args.reason, args.evidence, args.pr,
                args.expiry_minutes,
                state_path=args.state_dir / "state.json",
                body_path=args.state_dir / "incident-body.md",
            )
            print(f"Relaxed {args.check!r}; incident #{issue_number}")
        elif args.command == "restore":
            readback = restore(
                args.repo, args.incident,
                comment_path=args.state_dir / "restore-comment.md",
            )
            print(f"Restored and verified: {readback}")
    except CiBypassError as error:
        print(f"error: {error}", file=sys.stderr)
        sys.exit(1)


if __name__ == "__main__":
    main()
