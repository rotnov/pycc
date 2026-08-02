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
