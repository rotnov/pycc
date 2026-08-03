#!/usr/bin/env python3
"""Lifecycle tests for scripts/manage_ci_bypass.py."""

from __future__ import annotations

import io
import json
import runpy
import sys
import tempfile
import unittest
from contextlib import redirect_stderr
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
                {"required_status_checks": {"strict": True, "contexts": ["audit", "ci-gate"]}}
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
                {"required_status_checks": {"strict": True, "contexts": ["audit", "ci-gate"]}}
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
        self.assertEqual(
            state["snapshot"], {"strict": True, "contexts": ["audit", "ci-gate"]}
        )

    @mock.patch("manage_ci_bypass.run_gh")
    def test_main_restore_dispatches_and_writes_verified_comment(self, mock_run_gh):
        snapshot = json.dumps({"strict": True, "contexts": ["audit", "ci-gate"]})
        body = f"body\n{mcb.SNAPSHOT_MARKER_START}\n{snapshot}\n{mcb.SNAPSHOT_MARKER_END}\n"
        mock_run_gh.side_effect = self._dispatch({
            ("issue", "view"): json.dumps({"state": "OPEN", "body": body}),
            ("api", "-X"): "{}",
            ("api", "repos/owner/repo/branches/main/protection"): json.dumps(
                {"required_status_checks": {"strict": True, "contexts": ["audit", "ci-gate"]}}
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
        mock_subprocess_run.return_value = mock.Mock(
            returncode=0,
            stdout=json.dumps(
                {"required_status_checks": {"strict": True, "contexts": ["audit", "ci-gate"]}}
            ),
            stderr="",
        )
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
        mock_subprocess_run.assert_called_once_with(
            ["gh", "api", "repos/owner/repo/branches/main/protection"],
            input=None, capture_output=True, text=True,
        )


if __name__ == "__main__":
    unittest.main()
