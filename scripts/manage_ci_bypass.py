#!/usr/bin/env python3
"""Session-driven CI temporary-bypass lifecycle management.

See docs/superpowers/specs/2026-08-02-ci-temporary-bypass-mechanism-design.md
and docs/REPOSITORY_GOVERNANCE.md's "Session-driven temporary bypass" section
for the full design and the safety properties this script must preserve.
"""

from __future__ import annotations

import argparse
import json
import re
import subprocess
import sys
from datetime import datetime, timedelta, timezone
from pathlib import Path

REPO = "rotnov/pycc"
BASELINE_CONTEXTS = ["audit", "ci-gate"]
INCIDENT_TITLE_PREFIX = "[ci-bypass]"
DEFAULT_EXPIRY_MINUTES = 60
BASELINE_PROTECTION = {
    "strict": True,
    "contexts": sorted(BASELINE_CONTEXTS),
    "enforce_admins": True,
    "required_pull_request_reviews": {
        "dismiss_stale_reviews": False,
        "require_code_owner_reviews": False,
        "require_last_push_approval": False,
        "required_approving_review_count": 0,
    },
    "required_conversation_resolution": True,
    "allow_force_pushes": False,
    "allow_deletions": False,
}


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


def protection_snapshot(protection: dict) -> dict:
    return {
        "strict": protection["required_status_checks"]["strict"],
        "contexts": sorted(protection["required_status_checks"]["contexts"]),
        "enforce_admins": protection["enforce_admins"]["enabled"],
        "required_pull_request_reviews": protection.get("required_pull_request_reviews"),
        "required_conversation_resolution": protection["required_conversation_resolution"]["enabled"],
        "allow_force_pushes": protection["allow_force_pushes"]["enabled"],
        "allow_deletions": protection["allow_deletions"]["enabled"],
    }


def status(repo: str = REPO, now: datetime | None = None) -> tuple[bool, str]:
    if now is None:
        now = datetime.now(timezone.utc)
    protection = get_protection(repo)
    current = protection_snapshot(protection)
    drift = current != BASELINE_PROTECTION
    stale_issue = None
    for issue in list_open_bypass_issues(repo):
        try:
            expiry = parse_expiry_from_body(issue["body"])
        except CiBypassError:
            continue
        if expiry < now:
            stale_issue = issue["number"]
            break
    if not drift and stale_issue is None:
        return True, f"Branch protection matches baseline: {current}"
    parts = []
    if drift:
        parts.append(
            f"Branch protection DRIFT: current {current} != baseline {BASELINE_PROTECTION}"
        )
    if stale_issue is not None:
        parts.append(
            f"Incident #{stale_issue} is open past its recorded expiry with no restore recorded"
        )
    return False, "; ".join(parts)


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


def list_open_bypass_issues(repo: str = REPO) -> list[dict]:
    output = run_gh(
        [
            "issue", "list", "--repo", repo,
            "--search", f'"{INCIDENT_TITLE_PREFIX}" in:title',
            "--state", "open", "--json", "number,title,body",
        ]
    )
    return [
        issue for issue in json.loads(output)
        if issue["title"].startswith(INCIDENT_TITLE_PREFIX)
    ]


def parse_expiry_from_body(body: str) -> datetime:
    match = re.search(r"\*\*Expiry:\*\* (\S+)", body)
    if not match:
        raise CiBypassError("incident issue body has no parseable Expiry line")
    try:
        return datetime.strptime(match.group(1), "%Y-%m-%dT%H:%M:%SZ").replace(
            tzinfo=timezone.utc
        )
    except ValueError as error:
        raise CiBypassError(
            f"incident issue's Expiry timestamp is not parseable: {error}"
        ) from error


def build_incident_body(
    check_name: str, reason: str, evidence_text: str, snapshot: dict,
    expiry_minutes: int, expiry_timestamp: str, pr_number: int,
) -> str:
    snapshot_json = json.dumps(snapshot, indent=2, sort_keys=True)
    return (
        f"**Check relaxed:** `{check_name}`\n\n"
        f"**Motivating PR:** #{pr_number}\n\n"
        f"**Reason:** {reason}\n\n"
        f"**Expiry:** {expiry_timestamp} ({expiry_minutes} minutes from creation)\n\n"
        f"**Gate 1 verdict (CONFIRMED):**\n\n{evidence_text}\n\n"
        f"{SNAPSHOT_MARKER_START}\n{snapshot_json}\n{SNAPSHOT_MARKER_END}\n"
    )


def _create_issue(repo: str, title: str, body_path: Path) -> int:
    output = run_gh(
        ["issue", "create", "--repo", repo, "--title", title, "-F", str(body_path)]
    )
    try:
        url = output.strip().splitlines()[-1]
        return int(url.rstrip("/").rsplit("/", 1)[-1])
    except (IndexError, ValueError) as error:
        raise CiBypassError(
            f"could not parse an issue number from `gh issue create`'s output: "
            f"{output!r} ({error})"
        ) from error


def create_incident_issue(
    repo: str, check_name: str, reason: str, evidence_text: str, snapshot: dict,
    expiry_minutes: int, expiry_timestamp: str, pr_number: int, body_path: Path,
) -> int:
    title = f"{INCIDENT_TITLE_PREFIX} {check_name} relaxed — {reason}"
    body = build_incident_body(
        check_name, reason, evidence_text, snapshot,
        expiry_minutes, expiry_timestamp, pr_number,
    )
    body_path.write_text(body, encoding="utf-8")
    return _create_issue(repo, title, body_path)


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
    snapshot = protection_snapshot(protection)
    try:
        evidence_text = evidence_path.read_text(encoding="utf-8")
    except OSError as error:
        raise CiBypassError(
            f"could not read --evidence file {evidence_path}: {error}"
        ) from error
    if now is None:
        now = datetime.now(timezone.utc)
    expiry_timestamp = (now + timedelta(minutes=expiry_minutes)).strftime(
        "%Y-%m-%dT%H:%M:%SZ"
    )
    issue_number = create_incident_issue(
        repo, check_name, reason, evidence_text, snapshot,
        expiry_minutes, expiry_timestamp, pr_number, body_path,
    )
    remaining = [c for c in contexts if c != check_name]
    # Re-check immediately before the mutating PATCH to narrow (not eliminate)
    # the stacking race: two sessions can both pass the first check above
    # before either creates its incident. This second check catches the case
    # where a second incident appeared during evidence-gathering/issue
    # creation above. A residual window remains between this check and the
    # PATCH call itself -- see D-116's Consequences in docs/DECISIONS.md.
    other_open = find_open_bypass_issue(repo)
    if other_open is not None and other_open != issue_number:
        raise CiBypassError(
            f"a different [ci-bypass] incident (#{other_open}) appeared "
            f"between this incident's creation and the PATCH; aborting "
            f"before mutating protection -- incident #{issue_number} was "
            f"already created and needs manual cleanup via `restore --incident {issue_number}`"
        )
    patch_required_status_checks(repo, snapshot["strict"], remaining)
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
    readback = protection_snapshot(protection)
    expected = dict(snapshot)
    expected["contexts"] = sorted(snapshot["contexts"])
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


def restore_to_baseline(repo: str, body_path: Path, comment_path: Path) -> dict:
    if find_open_bypass_issue(repo) is not None:
        raise CiBypassError(
            "a [ci-bypass] incident is already open; refusing to create a "
            "second one -- investigate and clean up the existing incident first"
        )
    protection = get_protection(repo)
    before = protection_snapshot(protection)
    out_of_scope_drift = {
        key: before[key] for key in before
        if key not in ("strict", "contexts") and before[key] != BASELINE_PROTECTION[key]
    }
    title = f"{INCIDENT_TITLE_PREFIX} forced restore to baseline (no tracking incident found)"
    body_parts = [
        "No open `[ci-bypass]` incident was found while branch protection "
        "showed drift from the documented baseline (AGENTS.md's preflight "
        "escalation path, for exactly this case).",
        "**Captured drifted state (before any change made by this run):**\n\n"
        f"```json\n{json.dumps(before, indent=2, sort_keys=True)}\n```\n",
    ]
    if out_of_scope_drift:
        body_parts.append(
            "**Action needed from a human administrator.** This tool can "
            "only PATCH `required_status_checks` (`strict`/`contexts`) -- "
            f"drift was also found outside that scope, in: "
            f"{sorted(out_of_scope_drift)}. This run will restore "
            "`required_status_checks` only, then leave this incident open. "
            "Fix the remaining field(s) directly via GitHub's branch "
            "protection settings, confirm with "
            "`python3 scripts/manage_ci_bypass.py status`, then close this "
            "issue manually -- it is deliberately not restorable via "
            "`restore --incident` (no snapshot marker is embedded here, "
            "since this issue's own captured 'before' state is the drift "
            "itself, not something safe to restore back to)."
        )
    body_path.write_text("\n\n".join(body_parts), encoding="utf-8")
    issue_number = _create_issue(repo, title, body_path)
    patch_required_status_checks(repo, True, sorted(BASELINE_CONTEXTS))
    protection = get_protection(repo)
    readback = protection_snapshot(protection)
    if readback != BASELINE_PROTECTION:
        raise CiBypassError(
            f"restore_to_baseline PATCHed required_status_checks but "
            f"protection still does not match baseline -- incident "
            f"#{issue_number} is left open with the captured pre-mutation "
            f"state for a human administrator; expected {BASELINE_PROTECTION}, "
            f"readback {readback}"
        )
    comment = (
        "Forced restore verified. Readback matches documented baseline "
        f"exactly:\n\n```json\n{json.dumps(readback, indent=2, sort_keys=True)}\n```\n"
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
    restore_parser.add_argument("--incident", type=int, default=None)
    restore_parser.add_argument(
        "--to-baseline", action="store_true",
        help=(
            "force-restore to the documented baseline with no tracking "
            "incident (the no-open-issue escalation path); mutually "
            "exclusive with --incident"
        ),
    )

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
            if args.to_baseline and args.incident is not None:
                raise CiBypassError("--to-baseline and --incident are mutually exclusive")
            if args.to_baseline:
                readback = restore_to_baseline(
                    args.repo,
                    body_path=args.state_dir / "escalation-body.md",
                    comment_path=args.state_dir / "restore-comment.md",
                )
            else:
                incident = args.incident
                if incident is None:
                    state_file = args.state_dir / "state.json"
                    if not state_file.exists():
                        raise CiBypassError(
                            "restore requires --incident, --to-baseline, or a "
                            f"prior relax's state file at {state_file}"
                        )
                    incident = json.loads(state_file.read_text(encoding="utf-8"))["incident"]
                readback = restore(
                    args.repo, incident,
                    comment_path=args.state_dir / "restore-comment.md",
                )
            print(f"Restored and verified: {readback}")
    except CiBypassError as error:
        print(f"error: {error}", file=sys.stderr)
        sys.exit(1)


if __name__ == "__main__":
    main()
