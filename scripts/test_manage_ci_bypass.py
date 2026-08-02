#!/usr/bin/env python3
"""Lifecycle tests for scripts/manage_ci_bypass.py."""

from __future__ import annotations

import json
import tempfile
import unittest
from datetime import datetime, timezone
from pathlib import Path
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


if __name__ == "__main__":
    unittest.main()
