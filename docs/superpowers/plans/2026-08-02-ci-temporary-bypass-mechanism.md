# CI Temporary-Bypass Mechanism Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build `scripts/manage_ci_bypass.py` (a testable `status`/`relax`/`restore`
lifecycle script), a new `.claude/skills/ci-temporary-bypass/SKILL.md` orchestrating
skill, and the governance documentation that authorizes and bounds them, so a
session (attended or the standing autopilot loop, unattended) can temporarily
relax exactly one required CI check that is provably stuck due to external
repository state, then verifiably restore it.

**Architecture:** A single Python script owns every GitHub API interaction
through one `run_gh()` seam (mockable in tests). Three subcommands —
`status` (read-only drift check), `relax` (snapshot, verify, create public
incident, narrow `PATCH`), `restore` (read snapshot back from the incident,
`PATCH` back, verify readback, close) — compose into the full lifecycle. A
new skill orchestrates two independent, fresh `Agent()`-dispatched adversarial
gates around the script's `relax`/`restore` calls. Governance docs record the
explicit, narrow supersession of D-024/D-054 this mechanism represents.

**Tech Stack:** Python 3 stdlib only (`argparse`, `json`, `subprocess`,
`datetime`, `pathlib`), `gh` CLI, `unittest`/`unittest.mock` for tests —
matching `scripts/manage_ievo_hooks.py`'s existing shape and test conventions.

## Global Constraints

- 100% line and region coverage is a hard merge invariant (D-014) — every
  branch in `scripts/manage_ci_bypass.py` needs a covering test.
- All `gh` interaction goes through one `run_gh()` function — no other code
  path may call `subprocess` directly, so every test can mock exactly one seam.
- Never embed **multi-line or arbitrarily large** text (an incident body, an
  evidence dump, a JSON snapshot) directly into a CLI argument; write it to a
  file first and pass the file path (`-F`/`--input -`), matching
  `correction-capture.local.sh`'s established CWE-78-avoidance pattern. This
  constraint targets *shell-command-string construction* specifically (the
  real risk `correction-capture.local.sh` avoids, since a bash script
  interpolates text into a command string a shell then re-parses) -- it does
  not forbid passing a short, single-line, operator-supplied string (e.g.
  `--reason`'s value) as one clean `argv` element to `subprocess.run([...])`
  with no `shell=True`, which this script uses throughout and which carries
  no shell-metacharacter risk regardless of the string's content. Confirmed
  during Task 2's review: `reason` embedded in the incident issue's `--title`
  argument is not a defect under this constraint, precisely because of this
  distinction -- the constraint's target is shell reparsing, not argument
  length or content in general.
- Every new or changed skill needs both a `.claude/skills/...` canonical file
  and a thin `.agents/skills/...` Codex entrypoint pointing at it, in the same
  PR (`AGENTS.md`, "Support Codex and Claude Code").
- Confirmed real API shapes (verified empirically against `rotnov/pycc` during
  planning, not assumed):
  - `gh pr view <n> --repo <repo> --json statusCheckRollup` returns
    `{"statusCheckRollup": [{"name": "audit", "conclusion": "FAILURE", "status": "COMPLETED", ...}, ...]}`.
  - `gh issue list --repo <repo> --search '"[ci-bypass]" in:title' --state open --json number,title` works as expected.
  - `gh issue create --title ... -F <path>` prints the new issue's URL as its
    last line of stdout; the issue number is the URL's final path segment.
  - `gh issue close <n> --repo <repo> -r completed` and
    `gh issue comment <n> --repo <repo> -F <path>` both work as documented.
  - The correct mutating call is
    `gh api -X PATCH repos/{repo}/branches/main/protection/required_status_checks --input -`
    with a JSON body `{"strict": bool, "contexts": [...]}` on stdin — **not**
    the top-level `PUT .../protection` (see design doc's "Components §2").

---

### Task 1: Core GitHub helpers and the `status` subcommand

**Files:**
- Create: `scripts/manage_ci_bypass.py`
- Create: `scripts/test_manage_ci_bypass.py`

**Interfaces:**
- Produces: `CiBypassError(Exception)`; `run_gh(args: list[str], input_text: str | None = None) -> str`; `get_protection(repo: str) -> dict`; `required_contexts(protection: dict) -> list[str]`; `status(repo: str) -> tuple[bool, str]`; module constants `REPO = "rotnov/pycc"`, `BASELINE_CONTEXTS = ["audit", "ci-gate"]`, `INCIDENT_TITLE_PREFIX = "[ci-bypass]"`, `DEFAULT_EXPIRY_MINUTES = 60`.

- [ ] **Step 1: Write the failing tests**

```python
#!/usr/bin/env python3
"""Lifecycle tests for scripts/manage_ci_bypass.py."""

from __future__ import annotations

import json
import unittest
from unittest import mock

import manage_ci_bypass as mcb


class RunGhTests(unittest.TestCase):
    @mock.patch("subprocess.run")
    def test_run_gh_returns_stdout_on_success(self, mock_run):
        mock_run.return_value = mock.Mock(returncode=0, stdout="hello\n", stderr="")
        result = mcb.run_gh(["api", "some/path"])
        self.assertEqual(result, "hello\n")
        mock_run.assert_called_once_with(
            ["gh", "api", "some/path"], input=None, capture_output=True, text=True
        )

    @mock.patch("subprocess.run")
    def test_run_gh_passes_input_text(self, mock_run):
        mock_run.return_value = mock.Mock(returncode=0, stdout="{}", stderr="")
        mcb.run_gh(["api", "-X", "PATCH", "path", "--input", "-"], input_text='{"a": 1}')
        mock_run.assert_called_once_with(
            ["gh", "api", "-X", "PATCH", "path", "--input", "-"],
            input='{"a": 1}',
            capture_output=True,
            text=True,
        )

    @mock.patch("subprocess.run")
    def test_run_gh_raises_on_nonzero_exit(self, mock_run):
        mock_run.return_value = mock.Mock(returncode=1, stdout="", stderr="boom")
        with self.assertRaises(mcb.CiBypassError) as ctx:
            mcb.run_gh(["api", "bad/path"])
        self.assertIn("boom", str(ctx.exception))
        self.assertIn("gh api bad/path", str(ctx.exception))


class GetProtectionTests(unittest.TestCase):
    @mock.patch("manage_ci_bypass.run_gh")
    def test_get_protection_parses_json(self, mock_run_gh):
        mock_run_gh.return_value = json.dumps(
            {"required_status_checks": {"strict": True, "contexts": ["audit", "ci-gate"]}}
        )
        protection = mcb.get_protection("owner/repo")
        self.assertEqual(mcb.required_contexts(protection), ["audit", "ci-gate"])
        mock_run_gh.assert_called_once_with(
            ["api", "repos/owner/repo/branches/main/protection"]
        )


class StatusTests(unittest.TestCase):
    @mock.patch("manage_ci_bypass.run_gh")
    def test_status_ok_when_matches_baseline(self, mock_run_gh):
        mock_run_gh.return_value = json.dumps(
            {"required_status_checks": {"strict": True, "contexts": ["ci-gate", "audit"]}}
        )
        ok, message = mcb.status("owner/repo")
        self.assertTrue(ok)
        self.assertIn("matches baseline", message)

    @mock.patch("manage_ci_bypass.run_gh")
    def test_status_reports_drift(self, mock_run_gh):
        mock_run_gh.return_value = json.dumps(
            {"required_status_checks": {"strict": True, "contexts": ["ci-gate"]}}
        )
        ok, message = mcb.status("owner/repo")
        self.assertFalse(ok)
        self.assertIn("DRIFT", message)
        self.assertIn("['ci-gate']", message)

    @mock.patch("manage_ci_bypass.run_gh")
    def test_status_propagates_run_gh_failure(self, mock_run_gh):
        mock_run_gh.side_effect = mcb.CiBypassError("gh api ... failed (exit 1): not found")
        with self.assertRaises(mcb.CiBypassError):
            mcb.status("owner/repo")


class CliStatusTests(unittest.TestCase):
    @mock.patch("manage_ci_bypass.run_gh")
    def test_cli_status_exits_zero_when_ok(self, mock_run_gh):
        mock_run_gh.return_value = json.dumps(
            {"required_status_checks": {"strict": True, "contexts": ["ci-gate", "audit"]}}
        )
        with self.assertRaises(SystemExit) as ctx:
            mcb.main(["status"])
        self.assertEqual(ctx.exception.code, 0)

    @mock.patch("manage_ci_bypass.run_gh")
    def test_cli_status_exits_one_on_drift(self, mock_run_gh):
        mock_run_gh.return_value = json.dumps(
            {"required_status_checks": {"strict": True, "contexts": ["ci-gate"]}}
        )
        with self.assertRaises(SystemExit) as ctx:
            mcb.main(["status"])
        self.assertEqual(ctx.exception.code, 1)


if __name__ == "__main__":
    unittest.main()
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cd scripts && python3 -m pytest test_manage_ci_bypass.py -v`
Expected: `ModuleNotFoundError: No module named 'manage_ci_bypass'`

- [ ] **Step 3: Write the minimal implementation**

```python
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


def main(argv: list[str] | None = None) -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo", default=REPO)
    subparsers = parser.add_subparsers(dest="command", required=True)
    subparsers.add_parser("status")
    args = parser.parse_args(argv)

    try:
        if args.command == "status":
            ok, message = status(args.repo)
            print(message)
            sys.exit(0 if ok else 1)
    except CiBypassError as error:
        print(f"error: {error}", file=sys.stderr)
        sys.exit(1)


if __name__ == "__main__":
    main()
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cd scripts && python3 -m pytest test_manage_ci_bypass.py -v`
Expected: all `RunGhTests`, `GetProtectionTests`, `StatusTests`, `CliStatusTests` PASS.

- [ ] **Step 5: Commit**

```bash
git add scripts/manage_ci_bypass.py scripts/test_manage_ci_bypass.py
git commit -m "Add manage_ci_bypass.py's run_gh core and status subcommand"
```

---

### Task 2: The `relax` subcommand

**Files:**
- Modify: `scripts/manage_ci_bypass.py`
- Modify: `scripts/test_manage_ci_bypass.py`
- Modify: `.gitignore`

**Interfaces:**
- Consumes: `run_gh`, `get_protection`, `required_contexts`, `CiBypassError`, `REPO`, `INCIDENT_TITLE_PREFIX`, `DEFAULT_EXPIRY_MINUTES` (Task 1).
- Produces: `FAILING_CONCLUSIONS: set[str]`; `check_conclusion(repo: str, pr_number: int, check_name: str) -> str | None`; `find_open_bypass_issue(repo: str) -> int | None`; `build_incident_body(check_name: str, reason: str, evidence_text: str, snapshot: dict, expiry_minutes: int, expiry_timestamp: str) -> str`; `create_incident_issue(repo: str, check_name: str, reason: str, evidence_text: str, snapshot: dict, expiry_minutes: int, expiry_timestamp: str, body_path: Path) -> int`; `patch_required_status_checks(repo: str, strict: bool, contexts: list[str]) -> None`; `relax(repo: str, check_name: str, reason: str, evidence_path: Path, pr_number: int, expiry_minutes: int, state_path: Path, body_path: Path, now: datetime | None = None) -> int`.

- [ ] **Step 1: Write the failing tests**

```python
# Add to the top of scripts/test_manage_ci_bypass.py:
import tempfile
from datetime import datetime, timezone
from pathlib import Path

# --- append these test classes ---

class CheckConclusionTests(unittest.TestCase):
    @mock.patch("manage_ci_bypass.run_gh")
    def test_finds_named_check(self, mock_run_gh):
        mock_run_gh.return_value = json.dumps(
            {"statusCheckRollup": [
                {"name": "audit", "conclusion": "FAILURE", "status": "COMPLETED"},
                {"name": "ci-gate", "conclusion": "SUCCESS", "status": "COMPLETED"},
            ]}
        )
        self.assertEqual(mcb.check_conclusion("owner/repo", 279, "audit"), "FAILURE")

    @mock.patch("manage_ci_bypass.run_gh")
    def test_returns_none_for_missing_check(self, mock_run_gh):
        mock_run_gh.return_value = json.dumps({"statusCheckRollup": []})
        self.assertIsNone(mcb.check_conclusion("owner/repo", 279, "audit"))


class FindOpenBypassIssueTests(unittest.TestCase):
    @mock.patch("manage_ci_bypass.run_gh")
    def test_returns_number_when_open(self, mock_run_gh):
        mock_run_gh.return_value = json.dumps(
            [{"number": 292, "title": "[ci-bypass] audit relaxed -- reason"}]
        )
        self.assertEqual(mcb.find_open_bypass_issue("owner/repo"), 292)

    @mock.patch("manage_ci_bypass.run_gh")
    def test_returns_none_when_no_open_issue(self, mock_run_gh):
        mock_run_gh.return_value = "[]"
        self.assertIsNone(mcb.find_open_bypass_issue("owner/repo"))


class RelaxTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.tmp_path = Path(self.tmp.name)
        self.evidence_path = self.tmp_path / "evidence.txt"
        self.evidence_path.write_text("Gate 1 verdict: CONFIRMED\n", encoding="utf-8")
        self.state_path = self.tmp_path / "state.json"
        self.body_path = self.tmp_path / "incident-body.md"
        self.fixed_now = datetime(2026, 8, 2, 12, 0, 0, tzinfo=timezone.utc)

    def _run_gh_dispatch(self, script):
        """Return a side_effect function dispatching on the first two gh args."""
        def dispatch(args, input_text=None):
            key = tuple(args[:2])
            if key not in script:
                raise AssertionError(f"unexpected gh call: {args}")
            response = script[key]
            if isinstance(response, Exception):
                raise response
            return response
        return dispatch

    @mock.patch("manage_ci_bypass.run_gh")
    def test_relax_happy_path(self, mock_run_gh):
        mock_run_gh.side_effect = self._run_gh_dispatch({
            ("issue", "list"): "[]",
            ("pr", "view"): json.dumps(
                {"statusCheckRollup": [{"name": "audit", "conclusion": "FAILURE"}]}
            ),
            ("api", "repos/owner/repo/branches/main/protection"): json.dumps(
                {"required_status_checks": {"strict": True, "contexts": ["audit", "ci-gate"]}}
            ),
            ("issue", "create"): "https://github.com/owner/repo/issues/300\n",
            ("api", "-X"): "{}",
        })
        issue_number = mcb.relax(
            "owner/repo", "audit", "external state stuck", self.evidence_path,
            pr_number=279, expiry_minutes=60,
            state_path=self.state_path, body_path=self.body_path, now=self.fixed_now,
        )
        self.assertEqual(issue_number, 300)
        state = json.loads(self.state_path.read_text(encoding="utf-8"))
        self.assertEqual(state["incident"], 300)
        self.assertEqual(state["snapshot"], {"strict": True, "contexts": ["audit", "ci-gate"]})
        self.assertIn("CONFIRMED", self.body_path.read_text(encoding="utf-8"))
        self.assertIn("2026-08-02T13:00:00Z", self.body_path.read_text(encoding="utf-8"))

    @mock.patch("manage_ci_bypass.run_gh")
    def test_relax_refuses_when_incident_already_open(self, mock_run_gh):
        mock_run_gh.side_effect = self._run_gh_dispatch({
            ("issue", "list"): json.dumps([{"number": 292, "title": "[ci-bypass] existing"}]),
        })
        with self.assertRaises(mcb.CiBypassError) as ctx:
            mcb.relax(
                "owner/repo", "audit", "reason", self.evidence_path,
                pr_number=279, expiry_minutes=60,
                state_path=self.state_path, body_path=self.body_path, now=self.fixed_now,
            )
        self.assertIn("already open", str(ctx.exception))

    @mock.patch("manage_ci_bypass.run_gh")
    def test_relax_refuses_when_check_not_failing(self, mock_run_gh):
        mock_run_gh.side_effect = self._run_gh_dispatch({
            ("issue", "list"): "[]",
            ("pr", "view"): json.dumps(
                {"statusCheckRollup": [{"name": "audit", "conclusion": "SUCCESS"}]}
            ),
        })
        with self.assertRaises(mcb.CiBypassError) as ctx:
            mcb.relax(
                "owner/repo", "audit", "reason", self.evidence_path,
                pr_number=279, expiry_minutes=60,
                state_path=self.state_path, body_path=self.body_path, now=self.fixed_now,
            )
        self.assertIn("is not currently failing", str(ctx.exception))

    @mock.patch("manage_ci_bypass.run_gh")
    def test_relax_refuses_when_check_not_required(self, mock_run_gh):
        mock_run_gh.side_effect = self._run_gh_dispatch({
            ("issue", "list"): "[]",
            ("pr", "view"): json.dumps(
                {"statusCheckRollup": [{"name": "audit", "conclusion": "FAILURE"}]}
            ),
            ("api", "repos/owner/repo/branches/main/protection"): json.dumps(
                {"required_status_checks": {"strict": True, "contexts": ["ci-gate"]}}
            ),
        })
        with self.assertRaises(mcb.CiBypassError) as ctx:
            mcb.relax(
                "owner/repo", "audit", "reason", self.evidence_path,
                pr_number=279, expiry_minutes=60,
                state_path=self.state_path, body_path=self.body_path, now=self.fixed_now,
            )
        self.assertIn("is not currently a required check", str(ctx.exception))

    @mock.patch("manage_ci_bypass.run_gh")
    def test_relax_propagates_patch_failure(self, mock_run_gh):
        mock_run_gh.side_effect = self._run_gh_dispatch({
            ("issue", "list"): "[]",
            ("pr", "view"): json.dumps(
                {"statusCheckRollup": [{"name": "audit", "conclusion": "FAILURE"}]}
            ),
            ("api", "repos/owner/repo/branches/main/protection"): json.dumps(
                {"required_status_checks": {"strict": True, "contexts": ["audit", "ci-gate"]}}
            ),
            ("issue", "create"): "https://github.com/owner/repo/issues/300\n",
            ("api", "-X"): mcb.CiBypassError("gh api ... failed (exit 1): rate limited"),
        })
        with self.assertRaises(mcb.CiBypassError) as ctx:
            mcb.relax(
                "owner/repo", "audit", "reason", self.evidence_path,
                pr_number=279, expiry_minutes=60,
                state_path=self.state_path, body_path=self.body_path, now=self.fixed_now,
            )
        self.assertIn("rate limited", str(ctx.exception))
        # The incident issue was still created (visible/auditable) even though
        # the PATCH failed afterwards -- state file must not claim success.
        self.assertFalse(self.state_path.exists())
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cd scripts && python3 -m pytest test_manage_ci_bypass.py -v -k Relax`
Expected: `AttributeError: module 'manage_ci_bypass' has no attribute 'relax'` (and similarly for `check_conclusion`/`find_open_bypass_issue`).

- [ ] **Step 3: Write the minimal implementation**

Add `from datetime import datetime, timedelta, timezone` and
`from pathlib import Path` to the file's existing top-of-file import block
(alongside `argparse, json, subprocess, sys` from Task 1). Then add the
following after the `status` function, before `main`:

```python
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
```

Wire `relax` into `main()`'s subparsers and dispatch (replace the whole `main` function):

```python
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
    except CiBypassError as error:
        print(f"error: {error}", file=sys.stderr)
        sys.exit(1)
```

Add the ephemeral state directory to `.gitignore` (append near the other machine-local entries, e.g. after the `.ievo` block):

```
# CI temporary-bypass ephemeral state (scripts/manage_ci_bypass.py)
.ci-bypass-state/
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cd scripts && python3 -m pytest test_manage_ci_bypass.py -v -k "Relax or CheckConclusion or FindOpenBypassIssue"`
Expected: all PASS.

- [ ] **Step 5: Commit**

```bash
git add scripts/manage_ci_bypass.py scripts/test_manage_ci_bypass.py .gitignore
git commit -m "Add manage_ci_bypass.py's relax subcommand"
```

---

### Task 3: The `restore` subcommand

**Files:**
- Modify: `scripts/manage_ci_bypass.py`
- Modify: `scripts/test_manage_ci_bypass.py`

**Interfaces:**
- Consumes: `run_gh`, `get_protection`, `required_contexts`, `patch_required_status_checks`, `CiBypassError`, `SNAPSHOT_MARKER_START`, `SNAPSHOT_MARKER_END` (Task 2).
- Produces: `parse_snapshot_from_body(body: str) -> dict`; `get_incident_body(repo: str, issue_number: int) -> str`; `restore(repo: str, issue_number: int, comment_path: Path) -> dict`.

- [ ] **Step 1: Write the failing tests**

```python
# Append to scripts/test_manage_ci_bypass.py:

class ParseSnapshotFromBodyTests(unittest.TestCase):
    def test_parses_embedded_snapshot(self):
        body = (
            "some text\n"
            f"{mcb.SNAPSHOT_MARKER_START}\n"
            '{"strict": true, "contexts": ["audit", "ci-gate"]}\n'
            f"{mcb.SNAPSHOT_MARKER_END}\n"
        )
        snapshot = mcb.parse_snapshot_from_body(body)
        self.assertEqual(snapshot, {"strict": True, "contexts": ["audit", "ci-gate"]})

    def test_raises_when_marker_missing(self):
        with self.assertRaises(mcb.CiBypassError) as ctx:
            mcb.parse_snapshot_from_body("no marker here")
        self.assertIn("no embedded snapshot marker", str(ctx.exception))

    def test_raises_when_marker_unclosed(self):
        body = f"{mcb.SNAPSHOT_MARKER_START}\n{{not closed"
        with self.assertRaises(mcb.CiBypassError) as ctx:
            mcb.parse_snapshot_from_body(body)
        self.assertIn("not closed", str(ctx.exception))

    def test_raises_on_invalid_json(self):
        body = f"{mcb.SNAPSHOT_MARKER_START}\nnot json\n{mcb.SNAPSHOT_MARKER_END}\n"
        with self.assertRaises(mcb.CiBypassError) as ctx:
            mcb.parse_snapshot_from_body(body)
        self.assertIn("not valid JSON", str(ctx.exception))


class RestoreTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.comment_path = Path(self.tmp.name) / "comment.md"

    def _snapshot_body(self, strict=True, contexts=("audit", "ci-gate")):
        snapshot = json.dumps({"strict": strict, "contexts": list(contexts)})
        return f"body text\n{mcb.SNAPSHOT_MARKER_START}\n{snapshot}\n{mcb.SNAPSHOT_MARKER_END}\n"

    def _dispatch(self, script):
        def fn(args, input_text=None):
            key = tuple(args[:2])
            if key not in script:
                raise AssertionError(f"unexpected gh call: {args}")
            response = script[key]
            if isinstance(response, Exception):
                raise response
            return response
        return fn

    @mock.patch("manage_ci_bypass.run_gh")
    def test_restore_happy_path(self, mock_run_gh):
        mock_run_gh.side_effect = self._dispatch({
            ("issue", "view"): json.dumps({"state": "OPEN", "body": self._snapshot_body()}),
            ("api", "-X"): "{}",
            ("api", "repos/owner/repo/branches/main/protection"): json.dumps(
                {"required_status_checks": {"strict": True, "contexts": ["audit", "ci-gate"]}}
            ),
            ("issue", "comment"): "",
            ("issue", "close"): "",
        })
        readback = mcb.restore("owner/repo", 300, comment_path=self.comment_path)
        self.assertEqual(readback, {"strict": True, "contexts": ["audit", "ci-gate"]})
        self.assertIn("Readback matches", self.comment_path.read_text(encoding="utf-8"))

    @mock.patch("manage_ci_bypass.run_gh")
    def test_restore_raises_when_incident_not_open(self, mock_run_gh):
        mock_run_gh.side_effect = self._dispatch({
            ("issue", "view"): json.dumps({"state": "CLOSED", "body": self._snapshot_body()}),
        })
        with self.assertRaises(mcb.CiBypassError) as ctx:
            mcb.restore("owner/repo", 300, comment_path=self.comment_path)
        self.assertIn("is not open", str(ctx.exception))

    @mock.patch("manage_ci_bypass.run_gh")
    def test_restore_raises_on_drift(self, mock_run_gh):
        mock_run_gh.side_effect = self._dispatch({
            ("issue", "view"): json.dumps({"state": "OPEN", "body": self._snapshot_body()}),
            ("api", "-X"): "{}",
            # Readback shows a DIFFERENT context list than the snapshot -- DRIFT.
            ("api", "repos/owner/repo/branches/main/protection"): json.dumps(
                {"required_status_checks": {"strict": True, "contexts": ["ci-gate"]}}
            ),
        })
        with self.assertRaises(mcb.CiBypassError) as ctx:
            mcb.restore("owner/repo", 300, comment_path=self.comment_path)
        self.assertIn("DRIFT after restore", str(ctx.exception))
        self.assertIn("release-blocking governance incident", str(ctx.exception))
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cd scripts && python3 -m pytest test_manage_ci_bypass.py -v -k "Restore or ParseSnapshot"`
Expected: `AttributeError: module 'manage_ci_bypass' has no attribute 'restore'` (and similarly).

- [ ] **Step 3: Write the minimal implementation**

Add to `scripts/manage_ci_bypass.py` (after `relax`, before `main`):

```python
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
```

Wire `restore` into `main()` — add before the `except CiBypassError` block, and add its subparser next to `relax`'s:

```python
    restore_parser = subparsers.add_parser("restore")
    restore_parser.add_argument("--incident", required=True, type=int)
```

```python
        elif args.command == "restore":
            readback = restore(
                args.repo, args.incident,
                comment_path=args.state_dir / "restore-comment.md",
            )
            print(f"Restored and verified: {readback}")
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cd scripts && python3 -m pytest test_manage_ci_bypass.py -v`
Expected: every test in the file PASSES.

- [ ] **Step 5: Run full coverage to confirm 100%**

Run: `cd scripts && python3 -m coverage run -m pytest test_manage_ci_bypass.py && python3 -m coverage report -m --include="manage_ci_bypass.py"`
Expected: `manage_ci_bypass.py` at 100% (no missed lines). If any line/branch is
missed, add the covering test before moving on — do not proceed with a gap.

- [ ] **Step 6: Commit**

```bash
git add scripts/manage_ci_bypass.py scripts/test_manage_ci_bypass.py
git commit -m "Add manage_ci_bypass.py's restore subcommand; script complete"
```

---

### Task 4: The `ci-temporary-bypass` skill (Claude Code + Codex)

**Files:**
- Create: `.claude/skills/ci-temporary-bypass/SKILL.md`
- Create: `.agents/skills/ci-temporary-bypass/SKILL.md`

**Interfaces:**
- Consumes: `scripts/manage_ci_bypass.py`'s three subcommands and their exact flags (Tasks 1-3); the design doc's scope-boundary conditions and hard exclusions (`docs/superpowers/specs/2026-08-02-ci-temporary-bypass-mechanism-design.md`).
- Produces: nothing consumed by a later task in this plan — this is a leaf skill.

- [ ] **Step 1: Write `.claude/skills/ci-temporary-bypass/SKILL.md`**

```markdown
---
name: ci-temporary-bypass
description: Use this skill when a required CI check is failing on a pull request for reasons that appear entirely unrelated to that pull request's own diff -- e.g. every open PR shows the same failure simultaneously. Verifies the failure is provably caused by external repository state (not the PR's own defect) through two independent adversarial checks, then temporarily relaxes exactly that one required check via a public, time-bounded, auditable incident, and restores it immediately afterward with a second independent verification. Never use it to work around a check that is failing because of the current PR's own content.
---

# ci-temporary-bypass (Alpha)

Temporarily relax exactly one required CI check that is provably stuck due
to external repository state, then restore it -- publicly, narrowly, and
verifiably. This supersedes D-024's "not delegated to routine tasks" and
D-054's "grants no reusable permission" for this one mechanism only, per a
decision recorded in `docs/DECISIONS.md`; every other principle in those
decisions (public incident, minimal scope, time-bounded, immediately
restored, fully auditable) still applies without exception.

Full design and rationale:
`docs/superpowers/specs/2026-08-02-ci-temporary-bypass-mechanism-design.md`.
The manual "Emergency path" in `docs/REPOSITORY_GOVERNANCE.md` still exists
separately for anything this skill does not cover (administrator-only,
broader-scope relaxations).

This skill may be invoked by an attended session on the owner's explicit
instruction, or unattended by the standing autopilot loop
(`issue-select`/`issue-implement`) when it independently encounters a
qualifying stuck check -- both are authorized, per the repository owner's
explicit 2026-08-02 decision recorded in the design doc.

## Scope boundary -- read before invoking anything

A required check qualifies **only** when all three hold, verified fresh,
never assumed:

1. Its exact failure text matches an **already-documented** failure class
   in `docs/DECISIONS.md` or `docs/AGENT_RETROSPECTIVE.md`. A failure class
   seen for the first time can never go through this skill -- understand
   and document it in a separate session first, then this skill can be used
   for a later recurrence.
2. The same failure **reproduces fresh** (not a stale cached CI result --
   see `docs/SESSION_LOG.md`'s 2026-08-02 entries for why a `pull_request_target`
   check's last recorded result can be stale after the base branch moves)
   on another open pull request that has nothing to do with the one
   motivating this relaxation, **and** the reproduction genuinely isolates
   external state from the motivating PR's own content -- this is not
   satisfied by finding a second failing PR whose error text superficially
   matches. Actively construct and refute the alternative hypothesis "this
   candidate's own diff explains the failure" -- do not merely fail to
   think of it. Concrete near-miss this must reject: a PR proposing a
   genuinely new, not-yet-recognized manifest transition can fail
   `check_ci_permissions.rb`'s `validate_policy_successor_transition` with
   error text that pattern-matches an already-documented class, purely
   because *that PR's own content* introduces a digest or target the base
   checker does not yet recognize -- a correct, single-PR-fixable defect,
   not external state, even though the error text looks identical to a
   genuine cross-PR deadlock. Check whether the specific file(s) implicated
   in the failure are ones the motivating PR itself modifies; if so, the
   failure's cause cannot be external to that PR by construction, and this
   condition fails regardless of what a second PR shows.
3. The causal mechanism is read directly in the checker's own source (e.g.
   `scripts/check_ci_permissions.rb`), not inferred from the error text.

Never eligible, regardless of the above: `ci-gate` itself or any check
reflecting the PR's own build/test/coverage result; any check whose failure
cannot be traced to an unambiguous cause in the checker's source; any check
that fails only on the motivating PR and not on the independent comparison
PR (that is evidence of a real, PR-specific defect); any check whose
failure implicates a file the motivating PR itself modifies, even when a
second PR also currently fails the same check (two independently broken
PRs are not evidence of one shared external cause -- each must be
diagnosed on its own); any check when a `[ci-bypass]`-prefixed incident
issue is already open.

**Even once the named check is relaxed, that does not mean the pull request
is safe to merge.** Before merging, independently re-check the PR's overall
mergeability -- other blockers (an unresolved human or bot review thread,
being behind the base branch, a different failing check) are not addressed
by relaxing one named check, and merging through one of those anyway is
exactly the failure this skill's own incident-issue discipline exists to
prevent. If anything else blocks the merge, stop, restore immediately
(this skill's own Step 4, below), and report -- do not treat the relaxed
check as a green light for the whole PR.

## Workflow

### 1. Gate 1 -- pre-relax, adversarial

Dispatch a fresh, isolated `Agent()` -- never `advisor()`, which shares this
session's own transcript and could inherit its framing. Give it only raw
evidence: the exact failure text, the checker's source file path, the
motivating PR's own number and changed-file list, and the other PR number
to reproduce against. Its explicit brief: try to prove the claim false. It
must re-run the reproduction itself (not trust this session's report),
read the checker code itself (not trust this session's explanation), and
explicitly check whether the failure's implicated file(s) are ones the
*motivating* PR itself modifies -- if so, verdict REFUTED regardless of
what the other PR shows, per the Scope Boundary section's own worked
near-miss. If it cannot be dispatched, or returns anything other than an
unambiguous CONFIRMED verdict on all three scope-boundary conditions,
treat that as REFUTED -- fail closed, do not proceed.

On REFUTED: stop, report through the normal path (exactly as this
repository did for issue #243/PR #278 before this skill existed). Do not
retry Gate 1 for the same claim.

### 2. Relax

On CONFIRMED, write Gate 1's full verdict text to a file, then:

```
python3 scripts/manage_ci_bypass.py relax \
  --check <check-name> --reason "<short reason>" \
  --evidence <path-to-gate-1-verdict> --pr <motivating-pr-number> \
  [--expiry-minutes N]
```

`--expiry-minutes` defaults to 60. State its own chosen value explicitly if
overridden -- the incident issue always shows the effective expiry either
way. This step refuses (exit 1, `CiBypassError`) if the named check is not
currently failing on the given PR, or if a `[ci-bypass]` incident is
already open -- both are stop conditions, not retryable in-place.

### 3. Do the triggering work

Proceed with whatever the relaxation was for (typically: merge the
motivating PR). Before merging, re-verify overall mergeability per the
Scope Boundary section's closing paragraph above -- stop and restore
immediately if anything else blocks it.

### 4. Restore

Immediately after the triggering work completes (successfully or not):

```
python3 scripts/manage_ci_bypass.py restore --incident <issue-number>
```

This reads the snapshot back from the incident issue (authoritative),
`PATCH`es protection back to it, reads back the result, and raises
`CiBypassError` on any mismatch rather than silently closing the incident.

### 5. Gate 2 -- post-restore verification

Dispatch a second fresh, isolated `Agent()`. Give it the pre-relax snapshot
and the post-restore readback (both already in `restore`'s own output and
the incident issue's closing comment). Its brief: compare them field by
field and flag any drift beyond the one check that was deliberately
relaxed and restored -- not just the required-checks list, every other
protection field too (`enforce_admins`, `required_pull_request_reviews`,
`required_conversation_resolution`, `allow_force_pushes`, `allow_deletions`).

MATCH: done, incident issue is already closed by `restore`.
DRIFT, or Gate 2 cannot be dispatched: treat as a release-blocking
governance incident -- do not let this pass silently. Reopen the incident
issue with the drift details and escalate; this is not a condition this
skill resolves on its own.

## Stop conditions

- Gate 1 returns REFUTED, or cannot be dispatched.
- `relax` raises `CiBypassError` for any reason.
- Anything other than the named check blocks the actual merge after
  `relax` succeeds (unresolved review thread, behind base, another failing
  check) -- restore immediately, do not merge anyway.
- `restore` raises `CiBypassError` (including DRIFT).
- Gate 2 returns DRIFT, or cannot be dispatched.

Every stop condition above ends with restoring protection (if it was ever
relaxed) and reporting -- never with leaving protection relaxed and moving
on to something else.
```

- [ ] **Step 2: Write `.agents/skills/ci-temporary-bypass/SKILL.md`**

```markdown
---
name: ci-temporary-bypass
description: Use this skill when a required CI check is failing on a pull request for reasons that appear entirely unrelated to that pull request's own diff -- e.g. every open PR shows the same failure simultaneously. Verifies the failure is provably caused by external repository state (not the PR's own defect) through two independent adversarial checks, then temporarily relaxes exactly that one required check via a public, time-bounded, auditable incident, and restores it immediately afterward with a second independent verification. Never use it to work around a check that is failing because of the current PR's own content.
---

# ci-temporary-bypass (Alpha)

Resolve the current repository root. Before applying this skill, read
`.claude/skills/ci-temporary-bypass/SKILL.md` from that repository
completely and follow it as the canonical workflow. If the file is
missing, stop and report the missing project instruction instead of
substituting a cached copy.
```

- [ ] **Step 3: Validate both files**

Run: `cd .. && python3 scripts/validate_agent_assets.py && python3 scripts/validate_agent_policies.py`
(run from the repository root)
Expected: both print their `valid` message and exit 0.

- [ ] **Step 4: Commit**

```bash
git add .claude/skills/ci-temporary-bypass/SKILL.md .agents/skills/ci-temporary-bypass/SKILL.md
git commit -m "Add the ci-temporary-bypass orchestrating skill (Claude Code + Codex)"
```

---

### Task 5: Governance documentation

**Files:**
- Modify: `docs/DECISIONS.md`
- Modify: `docs/REPOSITORY_GOVERNANCE.md`
- Modify: `AGENTS.md`
- Modify: `docs/SPEC.md` (only if it indexes governance documents -- check first)

**Interfaces:**
- Consumes: the finished script (Tasks 1-3) and skill (Task 4) to reference by exact path/subcommand.

- [ ] **Step 1: Resolve the next free decision number**

Run: `grep -n "^## D-" docs/DECISIONS.md | tail -5`
Use the next unclaimed number after the highest one currently in the file
**and** after any number claimed by a currently-open pull request (check
`gh pr list --repo rotnov/pycc --search "D-1" --state open` and read each
candidate's diff) -- do not assume the number found here is still free by
the time this task's commit actually lands; re-check immediately before
committing this step.

- [ ] **Step 2: Add the new decision to `docs/DECISIONS.md`**

Append (using the resolved number from Step 1 in place of `D-1XX` below):

```markdown
## D-1XX: Session-driven temporary CI-check relaxation, narrowly superseding D-024/D-054

- Status: accepted
- Context: issue #109's D-112 `ci.yml` activation (PR #278) sat blocked on
  a maintainer-only emergency-bypass authorization for an extended period,
  during which every open pull request in the repository -- including two
  entirely unrelated ones (#279, #280) -- showed a failing required
  `audit` check for reasons that had nothing to do with their own diffs.
  `docs/REPOSITORY_GOVERNANCE.md`'s existing manual "Emergency path"
  (D-054/incident #125/PR #119, the only prior use) requires a human
  administrator to personally operate GitHub's UI/API every time, and
  D-054 explicitly states it "grants no reusable permission"; D-024
  states this authority "is not delegated to routine tasks." The
  repository owner decided, during a 2026-08-02 brainstorming session
  (`docs/superpowers/specs/2026-08-02-ci-temporary-bypass-mechanism-design.md`),
  to narrowly supersede that specific stance rather than continue
  absorbing this recurring cost manually.
- Decision: `scripts/manage_ci_bypass.py` and the
  `.claude/skills/ci-temporary-bypass/SKILL.md` /
  `.agents/skills/ci-temporary-bypass/SKILL.md` skill it backs may
  temporarily relax exactly one required status check, using whichever
  session's own authenticated `gh` access invokes them -- no new
  credential is provisioned or stored. Every use requires, in order: a
  fresh, isolated adversarial `Agent()` dispatch (never `advisor()`)
  independently confirming the failure matches an already-documented
  class, reproduces fresh on an unrelated open PR while genuinely
  isolating external state from the motivating PR's own content -- not
  satisfied by a superficial text match; a concurrent D-114/PR #291
  incident on this same repository surfaced a real near-miss the
  mechanism must reject (PR #290's own `validate_policy_successor_transition`
  failure, caused entirely by content #290 itself introduced, would have
  pattern-matched this mechanism's trigger class exactly as convincingly
  as a genuine cross-PR deadlock) -- and has an unambiguous cause read
  directly in the checker's own source; a public
  `[ci-bypass]`-prefixed incident issue created *before* the relaxation,
  containing the pre-relax snapshot, reason, evidence, and an explicit
  expiry; the scoped `PATCH .../protection/required_status_checks` call
  (never the whole-object `PUT .../protection`, which requires every
  field to be specified and risks silently resetting an omitted one);
  and, after the triggering work, a `PATCH` restore, a byte-exact
  readback verification, and a second independent adversarial `Agent()`
  dispatch confirming no other protection field drifted. The mechanism
  refuses to stack -- a new relaxation cannot begin while a `[ci-bypass]`
  incident is already open. The standing autopilot loop
  (`issue-select`/`issue-implement`) is explicitly authorized to invoke
  this mechanism unattended, without a live in-the-moment instruction --
  a deliberate choice the repository owner made after considering and
  rejecting the more conservative attended-only alternative.
- Alternatives: an independent GitHub Actions workflow timer with its own
  permanently-stored admin-scoped repository secret (rejected -- while
  it gives a hard time-bound guarantee closer to D-054's own shell exit
  trap, it requires a new, permanently-held, very-high-privilege
  credential to exist in the repository indefinitely, a standing risk
  the owner chose not to accept). Leaving the manual Emergency path as
  the only path (rejected -- does not address the actual, recurring cost
  that prompted this decision). A fully automated, unattended,
  credential-bearing execution path independent of any session's own
  `gh` access (rejected as out of scope for this decision; explicitly
  not what was built).
- Consequences: `AGENTS.md`'s "Protect main" section gains a new preflight
  rule -- deliberately a probabilistic fail-safe (some future session
  eventually notices and restores via `manage_ci_bypass.py status`), not
  a hard infrastructure time bound, in exchange for introducing no new
  standing secret. `docs/REPOSITORY_GOVERNANCE.md`'s existing manual
  Emergency path is unchanged and remains available for anything this
  narrower mechanism does not cover.
```

- [ ] **Step 3: Add a new section to `docs/REPOSITORY_GOVERNANCE.md`**

Run: `grep -n "^## Emergency path" docs/REPOSITORY_GOVERNANCE.md` to find
the insertion point, then add a new `##` section immediately after the
existing "Emergency path" section ends (before the next `##` heading):

```markdown
## Session-driven temporary bypass

A second, narrower relaxation path exists alongside the Emergency path
above, for exactly one situation: a required CI check that is provably
stuck due to external repository state, not the current pull request's
own defect. Recorded in `docs/DECISIONS.md` (D-1XX -- see Step 1 for the
resolved number), narrowly superseding D-024's "not delegated to routine
tasks" and D-054's "grants no reusable permission" for this mechanism
only. Full workflow: `.claude/skills/ci-temporary-bypass/SKILL.md`.

Unlike the Emergency path above, this one does not require a human
administrator to personally operate GitHub's UI/API for each use -- any
session (attended, or the standing autopilot loop unattended) may invoke
it using its own authenticated `gh` access, provided every step in the
linked skill's workflow is followed: two independent adversarial
`Agent()` verifications (before relaxing, and after restoring), a public
`[ci-bypass]`-prefixed incident issue created before any protection edit,
relaxation of exactly the one named check via the scoped `PATCH
.../protection/required_status_checks` endpoint, and a byte-exact
restore verification. `scripts/manage_ci_bypass.py status` reports any
drift between current protection and this document's own baseline;
`AGENTS.md`'s "Protect main" section requires every session's preflight
to run it and restore immediately if drift is found with no live
tracking incident.

Every other requirement from the Emergency path above still applies
without exception: exactly one control relaxed at a time, immediate
restoration, full public auditability. The Emergency path itself is
unchanged and remains the path for anything this narrower mechanism does
not cover (broader relaxations, or when no session with the owner's own
`gh` access is available to run it).
```

- [ ] **Step 4: Add the preflight fail-safe rule to `AGENTS.md`**

Run: `grep -n "release-blocking governance incident" AGENTS.md` to find
the existing `main-history-audit` sentence in the "Protect main" section,
then add immediately after it:

```markdown
- Every session's D-021 preflight also runs `python3 scripts/manage_ci_bypass.py status`.
  If branch protection differs from the documented baseline
  (`docs/REPOSITORY_GOVERNANCE.md`), search for an open `[ci-bypass]`-prefixed
  issue tracking it. If none exists, or the one that does is open past its
  own recorded expiry with no restore recorded, this is a release-blocking
  governance incident: run `python3 scripts/manage_ci_bypass.py restore
  --incident <issue-number>` immediately (or escalate if restore itself
  fails) before any other work in this session.
```

`AGENTS.md` has no enumerated per-skill list to add to -- its "Support
Codex and Claude Code" section states the general policy (every new or
changed skill needs both a `.claude/skills/...` file and a thin
`.agents/skills/...` entrypoint in the same PR), which Task 4 already
satisfies by creating both files together. No further `AGENTS.md` edit is
needed for skill discoverability specifically -- confirm this by
re-reading that section, rather than searching for a list that does not
exist.

- [ ] **Step 5: Check `docs/SPEC.md` for a governance-document index**

Run: `grep -n "REPOSITORY_GOVERNANCE" docs/SPEC.md`
If `docs/SPEC.md` lists `docs/REPOSITORY_GOVERNANCE.md` as an indexed
specification, no new entry is needed (the file itself, not each of its
sections, is what's indexed) -- confirm this explicitly rather than
skipping the check silently, per this repository's own documentation-impact
discipline (`AGENTS.md`: "If a code change genuinely has no documentation
impact, explicitly verify that conclusion rather than skipping the docs
review by default").

- [ ] **Step 6: Commit**

```bash
git add docs/DECISIONS.md docs/REPOSITORY_GOVERNANCE.md AGENTS.md
git commit -m "Document the session-driven CI temporary-bypass mechanism (D-1XX)"
```

---

### Task 6: Final integration -- full local gate set and pinned review

**Files:** none new; this task verifies Tasks 1-5 together.

- [ ] **Step 1: Run the full script test suite with coverage**

Run: `cd scripts && python3 -m coverage run -m pytest test_manage_ci_bypass.py test_validate_agent_assets.py test_validate_agent_policies.py -v && python3 -m coverage report -m --include="manage_ci_bypass.py"`
Expected: all tests pass; `manage_ci_bypass.py` at 100% lines and regions.

- [ ] **Step 2: Run the repository-wide gates**

Run, from the repository root, capturing each command's own exit status
explicitly (never through a pipe that would hide it):
```bash
python3 scripts/validate_agent_assets.py; echo "assets exit=$?"
python3 scripts/validate_agent_policies.py; echo "policies exit=$?"
LANG=en_US.UTF-8 LC_ALL=en_US.UTF-8 ruby scripts/check_roadmap_evidence.rb; echo "roadmap-evidence exit=$?"
LANG=en_US.UTF-8 LC_ALL=en_US.UTF-8 ruby scripts/check_ci_permissions.rb; echo "ci-permissions exit=$?"
```
Expected: every exit code is `0`. This diff touches no
`tests/fixtures/policy-successor-manifest.json`-listed path and no
`.github/workflows/` file, so neither Ruby checker's manifest logic should
be affected -- if either fails, stop and investigate before proceeding
(per this plan's own scope, that would indicate an unexpected interaction,
not an expected outcome).

- [ ] **Step 3: Dispatch the pinned D-068 reviewer**

Stage every file from Tasks 1-5 (including untracked new files -- the
pinned reviewer omits untracked files from a working-tree review) and
dispatch the pinned `ievo:deep-reviewer` in an isolated worktree, per
`AGENTS.md`'s D-068 review-loop requirement. Verify each finding against
its source before fixing (e.g. if a finding claims a `gh` call's exact
argument shape is wrong, re-check it against this plan's Global
Constraints section's empirically-verified shapes, not just the finding's
own claim). Loop until a round reports no actionable findings, per the
standard D-068 discipline this repository has used throughout this
session's other work.

- [ ] **Step 4: Open the pull request**

Push the branch, open the PR with `Fixes` referencing this plan's origin
(if a tracking issue exists) or a plain summary otherwise, and proceed
through this repository's normal D-078 CI-monitoring and merge steps —
identical to every other pull request in this repository, since this
mechanism's own authority does not extend to bypassing the *normal* merge
path for its own first landing.
