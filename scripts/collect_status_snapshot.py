"""Refresh the status hero's commit-bound required-check snapshot (D-241).

Offline agent tool, never run in CI.  It reads GitHub through the
authenticated ``gh api`` CLI (read-only endpoints only: commits, pulls and
check-runs), builds the ``status`` record through
``site_status_evidence.expected_shape``/``derive_state`` and rewrites only that
record in ``site/evidence-heroes.json``.  Anything missing, inaccessible,
ambiguous or non-success makes the observation ``unavailable``: the collector
then reports why, exits non-zero and leaves the manifest untouched.

Sanitization: only the enumerated fields are written.  No tokens, actor logins
or timestamps other than the provider's ``completed_at`` and the collector's
own ``collected_at`` reach the record.

Usage:
    python3 scripts/collect_status_snapshot.py [--subject SHA]
        [--manifest site/evidence-heroes.json] [--collected-at RFC3339-UTC]
"""

import argparse
import datetime
import json
import re
import subprocess
import sys
from pathlib import Path

import site_status_evidence as status


API = "repos/rotnov/pycc"
PER_PAGE = 100


class Unavailable(Exception):
    """The observation cannot be recorded as anything but unavailable."""


def gh_api(path):
    """One ``gh api --include`` call; returns (headers, parsed JSON body)."""
    result = subprocess.run(["gh", "api", "--include", path], capture_output=True, text=True)
    if result.returncode:
        raise Unavailable(f"gh api {path} failed: {result.stderr.strip() or result.returncode}")
    separator = re.search(r"\r?\n\r?\n", result.stdout)
    if not separator:
        raise Unavailable(f"gh api {path} returned no body")
    head, body = result.stdout[:separator.start()], result.stdout[separator.end():]
    headers = {}
    for line in head.splitlines()[1:]:
        name, _, value = line.partition(":")
        headers[name.strip().lower()] = value.strip()
    try:
        return headers, json.loads(body)
    except json.JSONDecodeError as error:
        raise Unavailable(f"gh api {path} returned invalid JSON: {error}")


def paginated_check_runs(sha):
    """Every check-run of ``sha`` (latest per name), or Unavailable when a page is missing."""
    runs = []
    page = 1
    total = None
    while True:
        headers, body = gh_api(f"{API}/commits/{sha}/check-runs?per_page={PER_PAGE}&page={page}")
        if not isinstance(body, dict) or not isinstance(body.get("check_runs"), list) \
                or not all(isinstance(run, dict) for run in body["check_runs"]):
            raise Unavailable(f"check-runs payload for {sha} is malformed")
        total = body.get("total_count") if total is None else total
        runs.extend(body["check_runs"])
        if not re.search(r'<[^>]+>;\s*rel="next"', headers.get("link", "")):
            break
        page += 1
    if not isinstance(total, int) or len(runs) != total:
        raise Unavailable(f"check-runs collection for {sha} is incomplete ({len(runs)} of {total!r})")
    return runs


def commit_facts(sha):
    _, body = gh_api(f"{API}/commits/{sha}")
    try:
        full = body["sha"]
        parents = body["parents"]
        tree = body["commit"]["tree"]["sha"]
    except (KeyError, TypeError):
        raise Unavailable(f"commit payload for {sha} is malformed")
    if not isinstance(parents, list) or not status.SHA_RE.match(str(full)) or not status.SHA_RE.match(str(tree)):
        raise Unavailable(f"commit payload for {sha} is malformed")
    return full, len(parents), tree


def merged_pull_request(sha):
    _, body = gh_api(f"{API}/commits/{sha}/pulls")
    if not isinstance(body, list):
        raise Unavailable(f"pulls payload for {sha} is malformed")
    merged = [item for item in body if isinstance(item, dict) and item.get("merged_at")
              and item.get("merge_commit_sha") == sha]
    if len(merged) != 1:
        raise Unavailable(f"expected exactly one merged pull request for {sha}, found {len(merged)}")
    item = merged[0]
    try:
        number, head_sha = item["number"], item["head"]["sha"]
    except (KeyError, TypeError):
        raise Unavailable(f"pull request payload for {sha} is malformed")
    if not isinstance(number, int) or not status.SHA_RE.match(str(head_sha)):
        raise Unavailable(f"pull request payload for {sha} is malformed")
    return number, head_sha


def check_run(runs, name, sha):
    """The single app-15368 check-run named ``name``, as sanitized fields."""
    matches = [run for run in runs if run.get("name") == name and (run.get("app") or {}).get("id") == status.APP_ID]
    if len(matches) != 1:
        raise Unavailable(f"check-run {name!r} on {sha} is missing or ambiguous ({len(matches)} found)")
    run = matches[0]
    match = status.JOB_URL_RE.match(run.get("html_url") or "")
    if not match:
        raise Unavailable(f"check-run {name!r} on {sha} has no immutable job URL")
    conclusion = run.get("conclusion") if run.get("status") == "completed" else None
    completed_at = run.get("completed_at") if conclusion is not None else None
    if completed_at is not None and not status.is_utc_instant(completed_at):
        raise Unavailable(f"check-run {name!r} on {sha} has a non-RFC 3339 completed_at")
    return {
        "conclusion": conclusion,
        "completed_at": completed_at,
        "run_id": int(match[1]),
        "run_url": f"{status.REPO}/actions/runs/{match[1]}",
        "job_url": run["html_url"],
    }


def git_show(repo_root, revision, path):
    result = subprocess.run(["git", "-C", str(repo_root), "show", f"{revision}:{path}"], capture_output=True, text=True)
    if result.returncode:
        raise Unavailable(f"cannot read {path} at {revision}")
    return result.stdout


def test_names(source):
    return re.findall(r"^\s+def (test_\w+)\(", source, re.M)


def build_record(subject, repo_root, collected_at):
    """Observe one revision and return the status record fields (state included)."""
    commit, parent_count, tree = commit_facts(subject)
    number, head_sha = merged_pull_request(commit)
    head_full, _, head_tree = commit_facts(head_sha)
    if head_full == commit:
        raise Unavailable("merged pull request head equals the merge commit")
    main_runs = paginated_check_runs(commit)
    head_runs = paginated_check_runs(head_full)
    gate = check_run(main_runs, "ci-gate", commit)
    audit = check_run(head_runs, "audit", head_full)
    platforms = []
    for name, runner, architecture in status.TIER1:
        row = check_run(main_runs, name, commit)
        if row["run_id"] != gate["run_id"]:
            raise Unavailable(f"Tier-1 job {name!r} belongs to run {row['run_id']}, not the ci-gate run {gate['run_id']}")
        platforms.append({"runner": runner, "architecture": architecture, "check_run_name": name,
                          "conclusion": row["conclusion"], "job_url": row["job_url"]})
    milestone = status.milestone_lead(git_show(repo_root, commit, "docs/ROADMAP.md"))
    if milestone is None:
        raise Unavailable(f"docs/ROADMAP.md at {commit} has no current-milestone line")
    collector = (repo_root / status.COLLECTOR).read_bytes()
    test_source = (repo_root / status.TEST).read_bytes()
    record = {
        "fixture": {"path": status.COLLECTOR, "sha256": status.canonical_sha256(collector)},
        "test": {"path": status.TEST, "sha256": status.canonical_sha256(test_source),
                 "names": test_names(test_source.decode())},
        "command": {"cwd": "repository-root", "argv": ["python3", status.COLLECTOR, "--subject", commit],
                    "requires": status.REQUIRES},
        "snapshot": {"subjects": [
            {"id": status.SUBJECTS[0][0], "label": status.SUBJECTS[0][1], "sha": commit, "check": None,
             "app_id": None, "conclusion": None, "completed_at": None, "run_id": None, "run_url": None,
             "job_url": None, "tree": tree, "parent_count": parent_count,
             "merged_pull_request": {"number": number, "head_sha": head_full, "head_tree": head_tree,
                                     "url": f"{status.REPO}/pull/{number}"}},
            {"id": status.SUBJECTS[1][0], "label": status.SUBJECTS[1][1], "sha": commit,
             "check": "ci-gate", "app_id": status.APP_ID, **gate},
            {"id": status.SUBJECTS[2][0], "label": status.SUBJECTS[2][1], "sha": head_full,
             "check": "audit", "app_id": status.APP_ID, **audit},
        ]},
        "repository": {"commit": commit, "tree": tree, "url": f"{status.REPO}/commit/{commit}"},
        "attestation": {"collected_at": collected_at, "collection_method": status.COLLECTION_METHOD,
                        "sanitized": True, "milestone_line": milestone,
                        "required_contexts": list(status.REQUIRED_CONTEXTS)},
        "environment": {"platforms": platforms},
        "limitations": status.LIMITATIONS,
    }
    record["state"] = status.derive_state(record)
    if record["state"] == "unavailable":
        reasons = [f"{item['check']}={item['conclusion']}" for item in record["snapshot"]["subjects"][1:]]
        reasons.extend(f"{row['runner']}={row['conclusion']}" for row in platforms)
        raise Unavailable("required conclusions are not all success: " + ", ".join(reasons))
    if parent_count != 1:
        raise Unavailable(f"subject {commit} has {parent_count} parents, expected exactly one")
    if head_tree != tree:
        raise Unavailable(f"merged pull request head tree {head_tree} differs from the subject tree {tree}")
    record["stable_links"] = status.expected_links(record)
    return record


def parse_args(argv):
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--subject", help="default-branch commit to observe (default: origin/main)")
    parser.add_argument("--manifest", default="site/evidence-heroes.json")
    parser.add_argument("--collected-at", help="RFC 3339 UTC capture time (default: now)")
    parser.add_argument("--repo-root", default=".")
    args = parser.parse_args(argv)
    if args.collected_at is not None and not status.is_utc_instant(args.collected_at):
        parser.error("--collected-at must be a real RFC 3339 UTC instant such as 2026-09-11T00:00:00Z")
    return args


def main(argv=None):
    args = parse_args(sys.argv[1:] if argv is None else argv)
    repo_root = Path(args.repo_root).resolve()
    manifest_path = Path(args.manifest)
    if not manifest_path.is_absolute():
        manifest_path = repo_root / manifest_path
    subject = args.subject
    if subject is None:
        resolved = subprocess.run(["git", "-C", str(repo_root), "rev-parse", "origin/main"], capture_output=True, text=True)
        if resolved.returncode:
            print("collect_status_snapshot: cannot resolve origin/main; pass --subject", file=sys.stderr)
            return 1
        subject = resolved.stdout.strip()
    collected_at = args.collected_at or datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")
    try:
        document = json.loads(manifest_path.read_text())
        heroes = document["heroes"]
        hero = next(item for item in heroes if item.get("page_id") == "status")
    except (OSError, ValueError, KeyError, StopIteration, AttributeError, TypeError) as error:
        print(f"collect_status_snapshot: cannot read the status hero from {manifest_path}: {error}", file=sys.stderr)
        return 1
    try:
        record = build_record(subject, repo_root, collected_at)
        hero.update(record)
        status.validate(hero, repo_root, repo_root)
    except Unavailable as error:
        print(f"collect_status_snapshot: state unavailable, manifest untouched: {error}", file=sys.stderr)
        return 1
    except SystemExit as error:
        print(f"collect_status_snapshot: built record rejected, manifest untouched: {error}", file=sys.stderr)
        return 1
    manifest_path.write_text(json.dumps(document, indent=2, ensure_ascii=False) + "\n")
    print(f"collect_status_snapshot: recorded {record['state']} for {record['repository']['commit']} "
          f"(PR #{record['snapshot']['subjects'][0]['merged_pull_request']['number']}) at {collected_at}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
