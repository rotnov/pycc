#!/usr/bin/env python3
"""Lifecycle tests for scripts/manage_ci_bypass.py."""

from __future__ import annotations

import io
import json
import re
import runpy
import sys
import tempfile
import unittest
from contextlib import redirect_stderr
from datetime import datetime, timezone
from pathlib import Path
from unittest import mock

import manage_ci_bypass as mcb

# Realistic branch-protection payload fields beyond `required_status_checks`,
# matching this repository's actual `main` branch protection (see
# protection_snapshot() in manage_ci_bypass.py). Tests that exercise relax()
# or restore() -- i.e. that call protection_snapshot() on a fake protection
# response -- need all of these keys present or protection_snapshot() raises
# KeyError. Tests that only exercise status() (which never calls
# protection_snapshot()) do not need them.
REQUIRED_PULL_REQUEST_REVIEWS = {
    "dismiss_stale_reviews": False,
    "require_code_owner_reviews": False,
    "require_last_push_approval": False,
    "required_approving_review_count": 0,
}

FULL_PROTECTION_RESPONSE = {
    "required_status_checks": {"strict": True, "contexts": ["audit", "ci-gate"]},
    "enforce_admins": {"enabled": True},
    "required_pull_request_reviews": REQUIRED_PULL_REQUEST_REVIEWS,
    "required_conversation_resolution": {"enabled": True},
    "allow_force_pushes": {"enabled": False},
    "allow_deletions": {"enabled": False},
}

FULL_PROTECTION_SNAPSHOT = {
    "strict": True,
    "contexts": ["audit", "ci-gate"],
    "enforce_admins": True,
    "required_pull_request_reviews": REQUIRED_PULL_REQUEST_REVIEWS,
    "required_conversation_resolution": True,
    "allow_force_pushes": False,
    "allow_deletions": False,
}


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
    def test_status_ok_when_matches_baseline(self, mock_run_gh):
        mock_run_gh.side_effect = self._dispatch({
            ("api", "repos/owner/repo/branches/main/protection"): json.dumps(
                FULL_PROTECTION_RESPONSE
            ),
            ("issue", "list"): "[]",
        })
        ok, message = mcb.status("owner/repo")
        self.assertTrue(ok)
        self.assertIn("matches baseline", message)

    @mock.patch("manage_ci_bypass.run_gh")
    def test_status_reports_drift_in_required_checks(self, mock_run_gh):
        # Only `contexts` differs from baseline -- every other field in the
        # fixture still matches BASELINE_PROTECTION exactly, isolating the
        # DRIFT signal to the required-checks list alone.
        drifted = {
            **FULL_PROTECTION_RESPONSE,
            "required_status_checks": {"strict": True, "contexts": ["ci-gate"]},
        }
        mock_run_gh.side_effect = self._dispatch({
            ("api", "repos/owner/repo/branches/main/protection"): json.dumps(drifted),
            ("issue", "list"): "[]",
        })
        ok, message = mcb.status("owner/repo")
        self.assertFalse(ok)
        self.assertIn("Branch protection DRIFT", message)
        self.assertIn("ci-gate", message)

    @mock.patch("manage_ci_bypass.run_gh")
    def test_status_reports_drift_in_non_status_check_field(self, mock_run_gh):
        # status() now compares the FULL 7-field snapshot against
        # BASELINE_PROTECTION, not just `contexts` -- a field outside
        # required_status_checks flipping (e.g. an admin disabling
        # enforce_admins) must be caught too, with contexts/strict matching.
        drifted = {**FULL_PROTECTION_RESPONSE, "enforce_admins": {"enabled": False}}
        mock_run_gh.side_effect = self._dispatch({
            ("api", "repos/owner/repo/branches/main/protection"): json.dumps(drifted),
            ("issue", "list"): "[]",
        })
        ok, message = mcb.status("owner/repo")
        self.assertFalse(ok)
        self.assertIn("Branch protection DRIFT", message)

    @mock.patch("manage_ci_bypass.run_gh")
    def test_status_ignores_incident_with_unparseable_expiry(self, mock_run_gh):
        # An open [ci-bypass] issue whose body has no parseable Expiry line
        # (hand-edited, or predating this field) must not crash status() or
        # be mistaken for a stale incident -- parse_expiry_from_body's
        # CiBypassError is caught and that issue is silently skipped.
        mock_run_gh.side_effect = self._dispatch({
            ("api", "repos/owner/repo/branches/main/protection"): json.dumps(
                FULL_PROTECTION_RESPONSE
            ),
            ("issue", "list"): json.dumps([
                {
                    "number": 250,
                    "title": "[ci-bypass] audit relaxed -- reason",
                    "body": "no expiry line in this body",
                },
            ]),
        })
        ok, message = mcb.status("owner/repo")
        self.assertTrue(ok)
        self.assertIn("matches baseline", message)

    @mock.patch("manage_ci_bypass.run_gh")
    def test_status_reports_drift_for_stale_incident_even_when_protection_matches(
        self, mock_run_gh
    ):
        # A [ci-bypass] incident open past its own recorded expiry, with no
        # restore recorded, is DRIFT on its own -- even when branch
        # protection itself currently matches the baseline exactly (e.g. a
        # second, unrelated relax/restore cycle already happened while this
        # stale incident sat open, or the restore is simply overdue).
        stale_body = "**Expiry:** 2026-08-01T00:00:00Z (60 minutes from creation)\n"
        mock_run_gh.side_effect = self._dispatch({
            ("api", "repos/owner/repo/branches/main/protection"): json.dumps(
                FULL_PROTECTION_RESPONSE
            ),
            ("issue", "list"): json.dumps([
                {
                    "number": 400,
                    "title": "[ci-bypass] audit relaxed -- reason",
                    "body": stale_body,
                },
            ]),
        })
        now = datetime(2026, 8, 3, 0, 0, 0, tzinfo=timezone.utc)
        ok, message = mcb.status("owner/repo", now=now)
        self.assertFalse(ok)
        self.assertNotIn("Branch protection DRIFT", message)
        self.assertIn("Incident #400", message)
        self.assertIn("past its recorded expiry", message)

    @mock.patch("manage_ci_bypass.run_gh")
    def test_status_ok_when_open_incident_is_not_yet_past_its_expiry(self, mock_run_gh):
        # The mirror image of the stale-incident test above: an open
        # [ci-bypass] incident that has NOT yet reached its recorded expiry
        # is actively-worked, not DRIFT. This is the exact branch AGENTS.md's
        # preflight rule relies on ("If one exists and is not past its
        # recorded expiry, no action is needed") -- if status() ever flagged
        # a live in-flight incident as stale, the preflight would force a
        # restore out from under an actively-working session.
        live_body = "**Expiry:** 2026-08-05T00:00:00Z (60 minutes from creation)\n"
        mock_run_gh.side_effect = self._dispatch({
            ("api", "repos/owner/repo/branches/main/protection"): json.dumps(
                FULL_PROTECTION_RESPONSE
            ),
            ("issue", "list"): json.dumps([
                {
                    "number": 402,
                    "title": "[ci-bypass] audit relaxed -- reason",
                    "body": live_body,
                },
            ]),
        })
        now = datetime(2026, 8, 3, 0, 0, 0, tzinfo=timezone.utc)
        ok, message = mcb.status("owner/repo", now=now)
        self.assertTrue(ok)
        self.assertIn("matches baseline", message)

    @mock.patch("manage_ci_bypass.run_gh")
    def test_status_reports_combined_drift_and_stale_incident(self, mock_run_gh):
        drifted = {**FULL_PROTECTION_RESPONSE, "enforce_admins": {"enabled": False}}
        stale_body = "**Expiry:** 2026-08-01T00:00:00Z (60 minutes from creation)\n"
        mock_run_gh.side_effect = self._dispatch({
            ("api", "repos/owner/repo/branches/main/protection"): json.dumps(drifted),
            ("issue", "list"): json.dumps([
                {
                    "number": 401,
                    "title": "[ci-bypass] audit relaxed -- reason",
                    "body": stale_body,
                },
            ]),
        })
        now = datetime(2026, 8, 3, 0, 0, 0, tzinfo=timezone.utc)
        ok, message = mcb.status("owner/repo", now=now)
        self.assertFalse(ok)
        self.assertIn("Branch protection DRIFT", message)
        self.assertIn("Incident #401", message)
        self.assertIn("past its recorded expiry", message)

    @mock.patch("manage_ci_bypass.run_gh")
    def test_status_propagates_run_gh_failure(self, mock_run_gh):
        mock_run_gh.side_effect = mcb.CiBypassError("gh api ... failed (exit 1): not found")
        with self.assertRaises(mcb.CiBypassError):
            mcb.status("owner/repo")


class CliStatusTests(unittest.TestCase):
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
    def test_cli_status_exits_zero_when_ok(self, mock_run_gh):
        mock_run_gh.side_effect = self._dispatch({
            ("api", f"repos/{mcb.REPO}/branches/main/protection"): json.dumps(
                FULL_PROTECTION_RESPONSE
            ),
            ("issue", "list"): "[]",
        })
        with self.assertRaises(SystemExit) as ctx:
            mcb.main(["status"])
        self.assertEqual(ctx.exception.code, 0)

    @mock.patch("manage_ci_bypass.run_gh")
    def test_cli_status_exits_one_on_drift(self, mock_run_gh):
        drifted = {
            **FULL_PROTECTION_RESPONSE,
            "required_status_checks": {"strict": True, "contexts": ["ci-gate"]},
        }
        mock_run_gh.side_effect = self._dispatch({
            ("api", f"repos/{mcb.REPO}/branches/main/protection"): json.dumps(drifted),
            ("issue", "list"): "[]",
        })
        with self.assertRaises(SystemExit) as ctx:
            mcb.main(["status"])
        self.assertEqual(ctx.exception.code, 1)


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

    @mock.patch("manage_ci_bypass.run_gh")
    def test_finds_named_check_when_not_first_in_rollup(self, mock_run_gh):
        # The loop must keep scanning past a non-matching entry instead of
        # stopping or matching the wrong check -- exercises the "name
        # mismatch, keep looking" loop-continuation path that a check listed
        # first in the rollup never reaches.
        mock_run_gh.return_value = json.dumps(
            {"statusCheckRollup": [
                {"name": "ci-gate", "conclusion": "SUCCESS", "status": "COMPLETED"},
                {"name": "audit", "conclusion": "FAILURE", "status": "COMPLETED"},
            ]}
        )
        self.assertEqual(mcb.check_conclusion("owner/repo", 279, "audit"), "FAILURE")


class FindOpenBypassIssueTests(unittest.TestCase):
    @mock.patch("manage_ci_bypass.run_gh")
    def test_returns_number_when_open(self, mock_run_gh):
        mock_run_gh.return_value = json.dumps(
            [{"number": 292, "title": "[ci-bypass] audit relaxed -- reason"}]
        )
        self.assertEqual(mcb.find_open_bypass_issue("owner/repo"), 292)

    @mock.patch("manage_ci_bypass.run_gh")
    def test_skips_fuzzy_search_hit_that_does_not_start_with_prefix(self, mock_run_gh):
        # `gh issue list --search '"[ci-bypass]" in:title'` is a fuzzy text
        # search, not an exact prefix match, so it can return issues that
        # merely mention "ci-bypass" somewhere in the title. The startswith
        # filter exists to reject those -- this proves a fuzzy-matched, wrongly
        # skipped hit doesn't cause relax() to falsely detect an open incident.
        mock_run_gh.return_value = json.dumps(
            [
                {"number": 9, "title": "unrelated issue mentioning ci-bypass in passing"},
                {"number": 292, "title": "[ci-bypass] audit relaxed -- reason"},
            ]
        )
        self.assertEqual(mcb.find_open_bypass_issue("owner/repo"), 292)

    @mock.patch("manage_ci_bypass.run_gh")
    def test_returns_none_when_no_open_issue(self, mock_run_gh):
        mock_run_gh.return_value = "[]"
        self.assertIsNone(mcb.find_open_bypass_issue("owner/repo"))


class ListOpenBypassIssuesTests(unittest.TestCase):
    @mock.patch("manage_ci_bypass.run_gh")
    def test_returns_empty_list_when_no_open_issues(self, mock_run_gh):
        mock_run_gh.return_value = "[]"
        self.assertEqual(mcb.list_open_bypass_issues("owner/repo"), [])

    @mock.patch("manage_ci_bypass.run_gh")
    def test_returns_matching_issue_with_body(self, mock_run_gh):
        issue = {
            "number": 292,
            "title": "[ci-bypass] audit relaxed -- reason",
            "body": "**Expiry:** 2026-08-02T13:00:00Z (60 minutes from creation)\n",
        }
        mock_run_gh.return_value = json.dumps([issue])
        self.assertEqual(mcb.list_open_bypass_issues("owner/repo"), [issue])

    @mock.patch("manage_ci_bypass.run_gh")
    def test_filters_out_fuzzy_search_hit_that_does_not_start_with_prefix(self, mock_run_gh):
        # Same fuzzy-search caveat as find_open_bypass_issue: `gh issue list
        # --search '"[ci-bypass]" in:title'` can return issues that merely
        # mention "ci-bypass" somewhere in the title -- the startswith
        # filter must reject those here too.
        matching = {
            "number": 292,
            "title": "[ci-bypass] audit relaxed -- reason",
            "body": "**Expiry:** 2026-08-02T13:00:00Z (60 minutes from creation)\n",
        }
        mock_run_gh.return_value = json.dumps([
            {
                "number": 9,
                "title": "unrelated issue mentioning ci-bypass in passing",
                "body": "irrelevant",
            },
            matching,
        ])
        self.assertEqual(mcb.list_open_bypass_issues("owner/repo"), [matching])


class ParseExpiryFromBodyTests(unittest.TestCase):
    def test_parses_valid_expiry(self):
        body = (
            "**Check relaxed:** `audit`\n\n"
            "**Expiry:** 2026-08-02T13:00:00Z (60 minutes from creation)\n\n"
        )
        expiry = mcb.parse_expiry_from_body(body)
        self.assertEqual(expiry, datetime(2026, 8, 2, 13, 0, 0, tzinfo=timezone.utc))

    def test_raises_when_expiry_line_missing(self):
        with self.assertRaises(mcb.CiBypassError) as ctx:
            mcb.parse_expiry_from_body("no expiry line anywhere in this body")
        self.assertIn("no parseable Expiry line", str(ctx.exception))

    def test_raises_when_expiry_timestamp_malformed(self):
        body = "**Expiry:** not-a-timestamp (60 minutes from creation)\n"
        with self.assertRaises(mcb.CiBypassError) as ctx:
            mcb.parse_expiry_from_body(body)
        self.assertIn("not parseable", str(ctx.exception))


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
                FULL_PROTECTION_RESPONSE
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
        self.assertEqual(state["snapshot"], FULL_PROTECTION_SNAPSHOT)
        self.assertIn("CONFIRMED", self.body_path.read_text(encoding="utf-8"))
        self.assertIn("2026-08-02T13:00:00Z", self.body_path.read_text(encoding="utf-8"))
        self.assertIn("**Motivating PR:** #279", self.body_path.read_text(encoding="utf-8"))

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
                FULL_PROTECTION_RESPONSE
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
        self.assertIn("**Motivating PR:** #279", self.body_path.read_text(encoding="utf-8"))

    @mock.patch("manage_ci_bypass.run_gh")
    def test_relax_defaults_now_to_current_utc_time_when_omitted(self, mock_run_gh):
        # No `now=` kwarg is passed here (unlike every other RelaxTests case),
        # so relax() must exercise its `now is None` branch and call
        # datetime.now(timezone.utc) itself. We patch manage_ci_bypass.datetime
        # to a fixed instant so the resulting expiry timestamp is still
        # deterministic and assertable, not just "didn't crash".
        mock_run_gh.side_effect = self._run_gh_dispatch({
            ("issue", "list"): "[]",
            ("pr", "view"): json.dumps(
                {"statusCheckRollup": [{"name": "audit", "conclusion": "FAILURE"}]}
            ),
            ("api", "repos/owner/repo/branches/main/protection"): json.dumps(
                FULL_PROTECTION_RESPONSE
            ),
            ("issue", "create"): "https://github.com/owner/repo/issues/301\n",
            ("api", "-X"): "{}",
        })
        fixed_now = datetime(2026, 8, 3, 9, 0, 0, tzinfo=timezone.utc)
        with mock.patch("manage_ci_bypass.datetime") as mock_datetime:
            mock_datetime.now.return_value = fixed_now
            issue_number = mcb.relax(
                "owner/repo", "audit", "external state stuck", self.evidence_path,
                pr_number=279, expiry_minutes=60,
                state_path=self.state_path, body_path=self.body_path,
            )
        mock_datetime.now.assert_called_once_with(timezone.utc)
        self.assertEqual(issue_number, 301)
        # 60 minutes after the fixed 09:00:00 stand-in for "now" -> 10:00:00.
        self.assertIn("2026-08-03T10:00:00Z", self.body_path.read_text(encoding="utf-8"))
        self.assertIn("**Motivating PR:** #279", self.body_path.read_text(encoding="utf-8"))

    @mock.patch("manage_ci_bypass.run_gh")
    def test_relax_wraps_missing_evidence_file_as_ci_bypass_error(self, mock_run_gh):
        mock_run_gh.side_effect = self._run_gh_dispatch({
            ("issue", "list"): "[]",
            ("pr", "view"): json.dumps(
                {"statusCheckRollup": [{"name": "audit", "conclusion": "FAILURE"}]}
            ),
            ("api", "repos/owner/repo/branches/main/protection"): json.dumps(
                FULL_PROTECTION_RESPONSE
            ),
        })
        missing_evidence = self.tmp_path / "does-not-exist.txt"
        with self.assertRaises(mcb.CiBypassError) as ctx:
            mcb.relax(
                "owner/repo", "audit", "reason", missing_evidence,
                pr_number=279, expiry_minutes=60,
                state_path=self.state_path, body_path=self.body_path, now=self.fixed_now,
            )
        self.assertIn("could not read --evidence file", str(ctx.exception))
        self.assertFalse(self.state_path.exists())

    @mock.patch("manage_ci_bypass.run_gh")
    def test_relax_aborts_when_a_different_incident_appears_before_the_patch(self, mock_run_gh):
        # The TOCTOU re-check: find_open_bypass_issue is called twice inside
        # relax() -- once at the very top, once again right before the
        # mutating PATCH. Here the first call finds nothing (so the initial
        # stacking guard passes and this incident gets created), but the
        # second call -- simulating a concurrent session's relax() that won
        # the race in between -- finds a DIFFERENT open incident. relax()
        # must abort before the PATCH rather than mutate protection out from
        # under the other session.
        issue_list_responses = iter([
            "[]",
            json.dumps([{"number": 999, "title": "[ci-bypass] something else"}]),
        ])
        script = {
            ("pr", "view"): json.dumps(
                {"statusCheckRollup": [{"name": "audit", "conclusion": "FAILURE"}]}
            ),
            ("api", "repos/owner/repo/branches/main/protection"): json.dumps(
                FULL_PROTECTION_RESPONSE
            ),
            ("issue", "create"): "https://github.com/owner/repo/issues/300\n",
        }

        def dispatch(args, input_text=None):
            key = tuple(args[:2])
            if key == ("issue", "list"):
                return next(issue_list_responses)
            if key not in script:
                raise AssertionError(f"unexpected gh call: {args}")
            response = script[key]
            if isinstance(response, Exception):
                raise response
            return response

        mock_run_gh.side_effect = dispatch
        with self.assertRaises(mcb.CiBypassError) as ctx:
            mcb.relax(
                "owner/repo", "audit", "external state stuck", self.evidence_path,
                pr_number=279, expiry_minutes=60,
                state_path=self.state_path, body_path=self.body_path, now=self.fixed_now,
            )
        message = str(ctx.exception)
        self.assertIn("#999", message)
        self.assertIn("already created and needs manual cleanup", message)
        self.assertIn("restore --incident 300", message)
        # PATCH must never have been called -- protection was not mutated,
        # and the state file (which would claim a successful relax) is absent.
        for call in mock_run_gh.call_args_list:
            self.assertNotEqual(tuple(call.args[0][:2]), ("api", "-X"))
        self.assertFalse(self.state_path.exists())


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

    def _snapshot_body(
        self,
        strict=True,
        contexts=("audit", "ci-gate"),
        enforce_admins=True,
        required_pull_request_reviews=None,
        required_conversation_resolution=True,
        allow_force_pushes=False,
        allow_deletions=False,
    ):
        if required_pull_request_reviews is None:
            required_pull_request_reviews = REQUIRED_PULL_REQUEST_REVIEWS
        snapshot = json.dumps({
            "strict": strict,
            "contexts": list(contexts),
            "enforce_admins": enforce_admins,
            "required_pull_request_reviews": required_pull_request_reviews,
            "required_conversation_resolution": required_conversation_resolution,
            "allow_force_pushes": allow_force_pushes,
            "allow_deletions": allow_deletions,
        })
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
                FULL_PROTECTION_RESPONSE
            ),
            ("issue", "comment"): "",
            ("issue", "close"): "",
        })
        readback = mcb.restore("owner/repo", 300, comment_path=self.comment_path)
        self.assertEqual(readback, FULL_PROTECTION_SNAPSHOT)
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
            # Every other field matches the (non-drifted) snapshot so the DRIFT
            # signal stays isolated to `contexts` alone.
            ("api", "repos/owner/repo/branches/main/protection"): json.dumps({
                **FULL_PROTECTION_RESPONSE,
                "required_status_checks": {"strict": True, "contexts": ["ci-gate"]},
            }),
        })
        with self.assertRaises(mcb.CiBypassError) as ctx:
            mcb.restore("owner/repo", 300, comment_path=self.comment_path)
        self.assertIn("DRIFT after restore", str(ctx.exception))
        self.assertIn("release-blocking governance incident", str(ctx.exception))

    @mock.patch("manage_ci_bypass.run_gh")
    def test_restore_raises_on_drift_in_non_status_check_field(self, mock_run_gh):
        # DRIFT is not limited to required_status_checks -- a readback that
        # flips enforce_admins (e.g. an admin disabled it while the bypass was
        # open) while contexts/strict match must also be caught.
        mock_run_gh.side_effect = self._dispatch({
            ("issue", "view"): json.dumps({"state": "OPEN", "body": self._snapshot_body()}),
            ("api", "-X"): "{}",
            ("api", "repos/owner/repo/branches/main/protection"): json.dumps({
                **FULL_PROTECTION_RESPONSE,
                "enforce_admins": {"enabled": False},
            }),
        })
        with self.assertRaises(mcb.CiBypassError) as ctx:
            mcb.restore("owner/repo", 300, comment_path=self.comment_path)
        self.assertIn("DRIFT after restore", str(ctx.exception))
        self.assertIn("release-blocking governance incident", str(ctx.exception))


class RestoreToBaselineTests(unittest.TestCase):
    """The no-tracking-issue escalation path AGENTS.md's D-021 preflight uses.

    Distinct from restore(): there is no incident issue to read a snapshot
    from, so this creates its own forced-restore issue, PATCHes directly to
    BASELINE_PROTECTION, and verifies the readback matches exactly.
    """

    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.tmp_path = Path(self.tmp.name)
        self.body_path = self.tmp_path / "escalation-body.md"
        self.comment_path = self.tmp_path / "restore-comment.md"

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
    def test_restore_to_baseline_happy_path(self, mock_run_gh):
        # FULL_PROTECTION_RESPONSE already matches BASELINE_PROTECTION in
        # every field, so there is no out-of-scope drift and the "Action
        # needed" section is absent from the created issue body.
        mock_run_gh.side_effect = self._dispatch({
            ("issue", "list"): "[]",
            ("issue", "create"): "https://github.com/owner/repo/issues/500\n",
            ("api", "-X"): "{}",
            ("api", "repos/owner/repo/branches/main/protection"): json.dumps(
                FULL_PROTECTION_RESPONSE
            ),
            ("issue", "comment"): "",
            ("issue", "close"): "",
        })
        readback = mcb.restore_to_baseline(
            "owner/repo", body_path=self.body_path, comment_path=self.comment_path,
        )
        self.assertEqual(readback, mcb.BASELINE_PROTECTION)
        body_text = self.body_path.read_text(encoding="utf-8")
        self.assertIn("No open `[ci-bypass]` incident", body_text)
        self.assertIn("Captured drifted state", body_text)
        self.assertNotIn("Action needed from a human administrator", body_text)
        self.assertNotIn(mcb.SNAPSHOT_MARKER_START, body_text)
        self.assertIn("Forced restore verified", self.comment_path.read_text(encoding="utf-8"))
        mock_run_gh.assert_any_call(
            [
                "api", "-X", "PATCH",
                "repos/owner/repo/branches/main/protection/required_status_checks",
                "--input", "-",
            ],
            input_text=json.dumps(
                {"strict": True, "contexts": sorted(mcb.BASELINE_CONTEXTS)}
            ),
        )
        mock_run_gh.assert_any_call(
            ["issue", "close", "500", "--repo", "owner/repo", "-r", "completed"]
        )
        mock_run_gh.assert_any_call(
            [
                "issue", "create", "--repo", "owner/repo",
                "--title",
                "[ci-bypass] forced restore to baseline (no tracking incident found)",
                "-F", str(self.body_path),
            ]
        )

    @mock.patch("manage_ci_bypass.run_gh")
    def test_restore_to_baseline_raises_on_drift_after_forced_restore(self, mock_run_gh):
        # Drift is in enforce_admins, a field this function can never PATCH --
        # it must capture that as out-of-scope, refuse to embed a restorable
        # snapshot marker, and still fail closed when the (unfixable) drift
        # persists on readback.
        drifted = {**FULL_PROTECTION_RESPONSE, "enforce_admins": {"enabled": False}}
        mock_run_gh.side_effect = self._dispatch({
            ("issue", "list"): "[]",
            ("issue", "create"): "https://github.com/owner/repo/issues/501\n",
            ("api", "-X"): "{}",
            ("api", "repos/owner/repo/branches/main/protection"): json.dumps(drifted),
        })
        with self.assertRaises(mcb.CiBypassError) as ctx:
            mcb.restore_to_baseline(
                "owner/repo", body_path=self.body_path, comment_path=self.comment_path,
            )
        message = str(ctx.exception)
        self.assertIn("does not match baseline", message)
        self.assertIn("#501", message)
        body_text = self.body_path.read_text(encoding="utf-8")
        self.assertIn("Action needed from a human administrator", body_text)
        self.assertIn("enforce_admins", body_text)
        # Never embed a restorable snapshot marker for a drifted (bad) state
        # -- `restore --incident` blindly reissues whatever is embedded there,
        # and this issue's captured "before" is the drift itself, not
        # something safe to restore back to.
        self.assertNotIn(mcb.SNAPSHOT_MARKER_START, body_text)
        # No comment/close call should have happened -- the issue must stay
        # open for a human to investigate the still-drifted state.
        for call in mock_run_gh.call_args_list:
            self.assertNotIn(tuple(call.args[0][:2]), (("issue", "comment"), ("issue", "close")))

    @mock.patch("manage_ci_bypass.run_gh")
    def test_restore_to_baseline_refuses_when_incident_already_open(self, mock_run_gh):
        mock_run_gh.side_effect = self._dispatch({
            ("issue", "list"): json.dumps([{"number": 292, "title": "[ci-bypass] existing"}]),
        })
        with self.assertRaises(mcb.CiBypassError) as ctx:
            mcb.restore_to_baseline(
                "owner/repo", body_path=self.body_path, comment_path=self.comment_path,
            )
        self.assertIn("already open", str(ctx.exception))
        # Must refuse before touching protection or creating a second issue --
        # the only gh call made is the open-incident check itself.
        self.assertEqual(
            [tuple(call.args[0][:2]) for call in mock_run_gh.call_args_list],
            [("issue", "list")],
        )

    @mock.patch("manage_ci_bypass.run_gh")
    def test_restore_to_baseline_captures_actual_drifted_state_not_baseline_target(
        self, mock_run_gh
    ):
        # The "before" snapshot embedded in the issue body must be what was
        # actually found (contexts missing "audit"), not BASELINE_PROTECTION's
        # target contexts -- otherwise the diagnostic is useless to a human.
        drifted = {
            **FULL_PROTECTION_RESPONSE,
            "required_status_checks": {"strict": True, "contexts": ["ci-gate"]},
        }
        mock_run_gh.side_effect = self._dispatch({
            ("issue", "list"): "[]",
            ("issue", "create"): "https://github.com/owner/repo/issues/502\n",
            ("api", "-X"): "{}",
            ("api", "repos/owner/repo/branches/main/protection"): json.dumps(drifted),
        })
        with self.assertRaises(mcb.CiBypassError):
            mcb.restore_to_baseline(
                "owner/repo", body_path=self.body_path, comment_path=self.comment_path,
            )
        body_text = self.body_path.read_text(encoding="utf-8")
        match = re.search(
            r"Captured drifted state.*?```json\n(.*?)\n```", body_text, re.DOTALL
        )
        self.assertIsNotNone(match, f"no captured-state JSON block found in: {body_text!r}")
        captured = json.loads(match.group(1))
        self.assertEqual(captured["contexts"], ["ci-gate"])
        self.assertNotEqual(captured["contexts"], sorted(mcb.BASELINE_CONTEXTS))


class CreateIssueTests(unittest.TestCase):
    @mock.patch("manage_ci_bypass.run_gh")
    def test_raises_when_gh_output_has_no_parseable_issue_number(self, mock_run_gh):
        mock_run_gh.return_value = ""
        with self.assertRaises(mcb.CiBypassError) as ctx:
            mcb._create_issue("owner/repo", "some title", Path("/dev/null"))
        self.assertIn("could not parse an issue number", str(ctx.exception))

    @mock.patch("manage_ci_bypass.run_gh")
    def test_raises_when_gh_output_last_line_is_not_a_url(self, mock_run_gh):
        mock_run_gh.return_value = "creating issue...\nnot a url\n"
        with self.assertRaises(mcb.CiBypassError) as ctx:
            mcb._create_issue("owner/repo", "some title", Path("/dev/null"))
        self.assertIn("could not parse an issue number", str(ctx.exception))


class MainDispatchTests(unittest.TestCase):
    """Covers main()'s CLI wiring: argument parsing and subcommand dispatch.

    Every other test in this file calls the underlying functions (status(),
    relax(), restore()) directly, so main()'s own subparser wiring and
    if/elif/except dispatch branches are otherwise never executed. These
    tests drive main() itself, the way a real invocation of the script would.
    """

    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.tmp_path = Path(self.tmp.name)
        self.state_dir = self.tmp_path / "state"
        self.evidence_path = self.tmp_path / "evidence.txt"
        self.evidence_path.write_text("Gate 1 verdict: CONFIRMED\n", encoding="utf-8")

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
    def test_main_relax_dispatches_and_persists_incident_state(self, mock_run_gh):
        mock_run_gh.side_effect = self._dispatch({
            ("issue", "list"): "[]",
            ("pr", "view"): json.dumps(
                {"statusCheckRollup": [{"name": "audit", "conclusion": "FAILURE"}]}
            ),
            ("api", "repos/owner/repo/branches/main/protection"): json.dumps(
                FULL_PROTECTION_RESPONSE
            ),
            ("issue", "create"): "https://github.com/owner/repo/issues/301\n",
            ("api", "-X"): "{}",
        })
        mcb.main([
            "--repo", "owner/repo", "--state-dir", str(self.state_dir),
            "relax", "--check", "audit", "--reason", "external state stuck",
            "--evidence", str(self.evidence_path), "--pr", "279",
        ])
        state = json.loads((self.state_dir / "state.json").read_text(encoding="utf-8"))
        self.assertEqual(state["incident"], 301)
        self.assertEqual(state["snapshot"], FULL_PROTECTION_SNAPSHOT)

    @mock.patch("manage_ci_bypass.run_gh")
    def test_main_restore_dispatches_and_writes_verified_comment(self, mock_run_gh):
        snapshot = json.dumps(FULL_PROTECTION_SNAPSHOT)
        body = f"body\n{mcb.SNAPSHOT_MARKER_START}\n{snapshot}\n{mcb.SNAPSHOT_MARKER_END}\n"
        mock_run_gh.side_effect = self._dispatch({
            ("issue", "view"): json.dumps({"state": "OPEN", "body": body}),
            ("api", "-X"): "{}",
            ("api", "repos/owner/repo/branches/main/protection"): json.dumps(
                FULL_PROTECTION_RESPONSE
            ),
            ("issue", "comment"): "",
            ("issue", "close"): "",
        })
        mcb.main([
            "--repo", "owner/repo", "--state-dir", str(self.state_dir),
            "restore", "--incident", "301",
        ])
        comment = (self.state_dir / "restore-comment.md").read_text(encoding="utf-8")
        self.assertIn("Readback matches", comment)
        # The close call is part of the dispatch script above; run_gh raising
        # AssertionError on any unscripted call is proof it happened as expected.
        mock_run_gh.assert_any_call(
            ["issue", "close", "301", "--repo", "owner/repo", "-r", "completed"]
        )

    @mock.patch("manage_ci_bypass.run_gh")
    def test_main_reports_ci_bypass_error_to_stderr_and_exits_one(self, mock_run_gh):
        mock_run_gh.side_effect = mcb.CiBypassError(
            "gh api repos/owner/repo/branches/main/protection failed (exit 1): not found"
        )
        stderr = io.StringIO()
        with redirect_stderr(stderr):
            with self.assertRaises(SystemExit) as ctx:
                mcb.main([
                    "--repo", "owner/repo", "--state-dir", str(self.state_dir), "status",
                ])
        self.assertEqual(ctx.exception.code, 1)
        self.assertIn("error:", stderr.getvalue())
        self.assertIn("not found", stderr.getvalue())

    @mock.patch("manage_ci_bypass.run_gh")
    def test_main_restore_to_baseline_dispatches(self, mock_run_gh):
        mock_run_gh.side_effect = self._dispatch({
            ("issue", "list"): "[]",
            ("issue", "create"): "https://github.com/owner/repo/issues/600\n",
            ("api", "-X"): "{}",
            ("api", "repos/owner/repo/branches/main/protection"): json.dumps(
                FULL_PROTECTION_RESPONSE
            ),
            ("issue", "comment"): "",
            ("issue", "close"): "",
        })
        mcb.main([
            "--repo", "owner/repo", "--state-dir", str(self.state_dir),
            "restore", "--to-baseline",
        ])
        comment = (self.state_dir / "restore-comment.md").read_text(encoding="utf-8")
        self.assertIn("Forced restore verified", comment)
        mock_run_gh.assert_any_call(
            ["issue", "close", "600", "--repo", "owner/repo", "-r", "completed"]
        )

    @mock.patch("manage_ci_bypass.run_gh")
    def test_main_restore_defaults_incident_from_state_file_when_omitted(self, mock_run_gh):
        self.state_dir.mkdir(parents=True, exist_ok=True)
        (self.state_dir / "state.json").write_text(
            json.dumps({"incident": 301, "snapshot": FULL_PROTECTION_SNAPSHOT}),
            encoding="utf-8",
        )
        snapshot = json.dumps(FULL_PROTECTION_SNAPSHOT)
        body = f"body\n{mcb.SNAPSHOT_MARKER_START}\n{snapshot}\n{mcb.SNAPSHOT_MARKER_END}\n"
        mock_run_gh.side_effect = self._dispatch({
            ("issue", "view"): json.dumps({"state": "OPEN", "body": body}),
            ("api", "-X"): "{}",
            ("api", "repos/owner/repo/branches/main/protection"): json.dumps(
                FULL_PROTECTION_RESPONSE
            ),
            ("issue", "comment"): "",
            ("issue", "close"): "",
        })
        mcb.main([
            "--repo", "owner/repo", "--state-dir", str(self.state_dir),
            "restore",
        ])
        # No --incident on the CLI -- main() must have read incident #301
        # back out of the prior relax's own state.json.
        mock_run_gh.assert_any_call(
            ["issue", "view", "301", "--repo", "owner/repo", "--json", "body,state"]
        )
        mock_run_gh.assert_any_call(
            ["issue", "close", "301", "--repo", "owner/repo", "-r", "completed"]
        )

    def test_main_restore_raises_when_no_incident_no_to_baseline_and_no_state_file(self):
        stderr = io.StringIO()
        with redirect_stderr(stderr):
            with self.assertRaises(SystemExit) as ctx:
                mcb.main([
                    "--repo", "owner/repo", "--state-dir", str(self.state_dir),
                    "restore",
                ])
        self.assertEqual(ctx.exception.code, 1)
        self.assertIn("requires --incident, --to-baseline", stderr.getvalue())

    def test_main_restore_raises_when_to_baseline_and_incident_both_given(self):
        stderr = io.StringIO()
        with redirect_stderr(stderr):
            with self.assertRaises(SystemExit) as ctx:
                mcb.main([
                    "--repo", "owner/repo", "--state-dir", str(self.state_dir),
                    "restore", "--incident", "301", "--to-baseline",
                ])
        self.assertEqual(ctx.exception.code, 1)
        self.assertIn("mutually exclusive", stderr.getvalue())


class ModuleEntryPointTests(unittest.TestCase):
    """Covers the `if __name__ == "__main__": main()` guard at module scope.

    Importing manage_ci_bypass as `mcb` (as every other test in this file
    does) never executes that guard's body, since __name__ is then
    "manage_ci_bypass", not "__main__". runpy.run_path re-executes the file
    with __name__ forced to "__main__", the same way `python3
    manage_ci_bypass.py ...` would from a shell -- this is the only path
    that exercises that line.
    """

    @mock.patch("subprocess.run")
    def test_running_the_file_as_a_script_invokes_main(self, mock_subprocess_run):
        # status() now issues two gh calls (protection, then the open
        # [ci-bypass] issue list), so the fake subprocess must dispatch on
        # the actual gh subcommand rather than return one static response.
        def fake_run(cmd, input=None, capture_output=None, text=None):
            if cmd[:2] == ["gh", "api"]:
                stdout = json.dumps(
                    {
                        "required_status_checks": {
                            "strict": True, "contexts": ["audit", "ci-gate"],
                        },
                        "enforce_admins": {"enabled": True},
                        "required_pull_request_reviews": {
                            "dismiss_stale_reviews": False,
                            "require_code_owner_reviews": False,
                            "require_last_push_approval": False,
                            "required_approving_review_count": 0,
                        },
                        "required_conversation_resolution": {"enabled": True},
                        "allow_force_pushes": {"enabled": False},
                        "allow_deletions": {"enabled": False},
                    }
                )
            elif cmd[:2] == ["gh", "issue"]:
                stdout = "[]"
            else:
                raise AssertionError(f"unexpected subprocess.run call: {cmd}")
            return mock.Mock(returncode=0, stdout=stdout, stderr="")

        mock_subprocess_run.side_effect = fake_run
        module_path = Path(mcb.__file__)
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        argv = [
            str(module_path), "--repo", "owner/repo", "--state-dir", tmp.name, "status",
        ]
        with mock.patch.object(sys, "argv", argv):
            with self.assertRaises(SystemExit) as ctx:
                runpy.run_path(str(module_path), run_name="__main__")
        self.assertEqual(ctx.exception.code, 0)
        mock_subprocess_run.assert_any_call(
            ["gh", "api", "repos/owner/repo/branches/main/protection"],
            input=None, capture_output=True, text=True,
        )


if __name__ == "__main__":
    unittest.main()
