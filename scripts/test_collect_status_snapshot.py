"""Tests for the offline status-snapshot collector with a stubbed ``gh``.

No network: a fake ``gh`` executable on PATH answers ``gh api --include``
from a JSON response map, so every provider shape (pagination, missing runs,
failures, malformed payloads) is exercised deterministically.
"""

import contextlib
import copy
import io
import json
import os
import stat
import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest import mock

import collect_status_snapshot as collector
import site_status_evidence as status


HEAD = "984eb6f27ca9f4809fa1277e12f8c83283bef1f6"
GATE_RUN = 34552872912
AUDIT_RUN = 34552229293
MILESTONE = "Current milestone: v0.3 — acceptance criteria met; v0.4 in progress."
ROADMAP = "# Roadmap\n\n**" + MILESTONE + "** More prose.\n"
STUB = """#!/usr/bin/env python3
import json, os, sys
responses = json.load(open(os.environ["GH_STUB_FILE"]))
assert sys.argv[1:3] == ["api", "--include"], sys.argv
entry = responses.get(sys.argv[3])
if entry is None:
    sys.stderr.write("stub: unexpected path " + sys.argv[3] + "\\n")
    sys.exit(2)
if entry.get("exit"):
    sys.stderr.write(entry.get("stderr", "stub failure") + "\\n")
    sys.exit(entry["exit"])
headers = entry.get("headers", {})
sys.stdout.write("HTTP/2.0 200 OK\\n")
for name, value in headers.items():
    sys.stdout.write(name + ": " + value + "\\r\\n")
sys.stdout.write("\\r\\n")
sys.stdout.write(entry["raw"] if "raw" in entry else json.dumps(entry["body"]))
"""


def check_run(name, run, job, conclusion="success", completed="2026-09-11T02:10:46Z", app=status.APP_ID, status_value="completed"):
    return {"name": name, "status": status_value, "conclusion": conclusion, "completed_at": completed,
            "app": {"id": app}, "html_url": f"{status.REPO}/actions/runs/{run}/job/{job}"}


def main_runs():
    runs = [check_run("ci-gate", GATE_RUN, 103121414776), check_run("governance", GATE_RUN, 103121414700)]
    for index, (name, _, _) in enumerate(status.TIER1):
        runs.append(check_run(name, GATE_RUN, 103119276900 + index))
    return runs


def responses(commit, tree="d9d6b55888897921f3dba4b14868b9c78b61bb9f"):
    api = collector.API
    runs = main_runs()
    return {
        f"{api}/commits/{commit}": {"body": {"sha": commit, "parents": [{"sha": "1" * 40}], "commit": {"tree": {"sha": tree}}}},
        f"{api}/commits/{HEAD}": {"body": {"sha": HEAD, "parents": [{"sha": "2" * 40}], "commit": {"tree": {"sha": tree}}}},
        f"{api}/commits/{commit}/pulls": {"body": [
            {"number": 1005, "merged_at": "2026-09-11T01:59:50Z", "merge_commit_sha": commit, "head": {"sha": HEAD}},
            {"number": 999, "merged_at": None, "merge_commit_sha": None, "head": {"sha": "3" * 40}},
        ]},
        f"{api}/commits/{commit}/check-runs?per_page=100&page=1": {"body": {"total_count": len(runs), "check_runs": runs}},
        f"{api}/commits/{HEAD}/check-runs?per_page=100&page=1": {"body": {"total_count": 2, "check_runs": [
            check_run("audit", AUDIT_RUN, 103117345163, completed="2026-09-11T01:50:30Z"),
            check_run("ci-gate", 34552229371, 103117345200),
        ]}},
    }


def git(root, *args):
    return subprocess.run(["git", "-C", str(root), *args], capture_output=True, text=True, check=True).stdout.strip()


class CollectorHarness(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory(prefix="collect-status-")
        self.root = Path(self.directory.name)
        self.repo = self.root / "repo"
        self.repo.mkdir()
        subprocess.run(["git", "init", "-q", "-b", "main", str(self.repo)], check=True)
        git(self.repo, "config", "user.email", "test@example.test")
        git(self.repo, "config", "user.name", "Collector Test")
        git(self.repo, "config", "commit.gpgsign", "false")
        (self.repo / "docs").mkdir()
        (self.repo / "docs" / "ROADMAP.md").write_text(ROADMAP)
        (self.repo / "scripts").mkdir()
        (self.repo / status.COLLECTOR).write_text("print('collector')\n")
        (self.repo / status.TEST).write_text("class T:\n    def test_one(self):\n        pass\n\n    def test_two(self):\n        pass\n")
        (self.repo / "site").mkdir()
        self.manifest = self.repo / "site" / "evidence-heroes.json"
        self.document = {"schema_version": "2.1.0", "heroes": [
            {"page_id": "landing", "state": "all-Tier-1"},
            {"page_id": "status", "evidence_id": "status-snapshot-v1", "kind": status.KIND, "route": "/status/",
             "page_path": "site/status/index.html", "fixture": None, "test": None, "command": None, "snapshot": None,
             "repository": None, "attestation": None, "environment": None, "state": "unavailable",
             "limitations": "old", "stable_links": {"owner": status.OWNER_ISSUE},
             "projections": {"html": "site/status/index.html", "markdown": "site/index.html.md", "llm": "site/llms.txt",
                             "structured_data": "site/status/index.html", "social": "site/status/index.html"}},
        ]}
        self.manifest.write_text(json.dumps(self.document, indent=2) + "\n")
        git(self.repo, "add", ".")
        git(self.repo, "commit", "-qm", "base")
        self.commit = git(self.repo, "rev-parse", "HEAD")
        self.bin = self.root / "bin"
        self.bin.mkdir()
        stub = self.bin / "gh"
        stub.write_text(STUB)
        stub.chmod(stub.stat().st_mode | stat.S_IXUSR)
        self.stub_file = self.root / "responses.json"
        self.responses = responses(self.commit)
        self.environment = mock.patch.dict(os.environ, {"PATH": f"{self.bin}{os.pathsep}{os.environ['PATH']}",
                                                        "GH_STUB_FILE": str(self.stub_file)})
        self.environment.start()

    def tearDown(self):
        self.environment.stop()
        self.directory.cleanup()

    def run_collector(self, *extra, subject="default"):
        subject = self.commit if subject == "default" else subject
        self.stub_file.write_text(json.dumps(self.responses))
        argv = ["--repo-root", str(self.repo), "--collected-at", "2026-09-11T03:00:00Z"]
        if subject is not None:
            argv.extend(["--subject", subject])
        argv.extend(extra)
        out, err = io.StringIO(), io.StringIO()
        with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
            code = collector.main(argv)
        return code, out.getvalue(), err.getvalue()

    def assert_unavailable(self, fragment):
        before = self.manifest.read_text()
        code, _, err = self.run_collector()
        self.assertEqual(code, 1, err)
        self.assertIn("manifest untouched", err)
        self.assertIn(fragment, err)
        self.assertEqual(self.manifest.read_text(), before)


class HappyPathTests(CollectorHarness):
    def test_records_all_tier_1_and_rewrites_only_the_status_record(self):
        code, out, err = self.run_collector()
        self.assertEqual(code, 0, err)
        self.assertIn("recorded all-Tier-1 for " + self.commit + " (PR #1005) at 2026-09-11T03:00:00Z", out)
        document = json.loads(self.manifest.read_text())
        self.assertEqual(document["heroes"][0], self.document["heroes"][0])
        hero = document["heroes"][1]
        self.assertEqual(hero["state"], "all-Tier-1")
        self.assertEqual(hero["limitations"], status.LIMITATIONS)
        self.assertEqual(hero["test"]["names"], ["test_one", "test_two"])
        self.assertEqual(hero["command"]["argv"], ["python3", status.COLLECTOR, "--subject", self.commit])
        self.assertEqual(hero["attestation"]["milestone_line"], MILESTONE)
        self.assertEqual(hero["attestation"]["collected_at"], "2026-09-11T03:00:00Z")
        subjects = hero["snapshot"]["subjects"]
        self.assertEqual([item["id"] for item in subjects], ["published-revision", "post-merge-ci-gate", "pre-merge-audit"])
        self.assertEqual(subjects[0]["merged_pull_request"], {"number": 1005, "head_sha": HEAD,
                                                              "head_tree": hero["repository"]["tree"],
                                                              "url": f"{status.REPO}/pull/1005"})
        self.assertEqual(subjects[1]["run_id"], GATE_RUN)
        self.assertEqual(subjects[2]["run_id"], AUDIT_RUN)
        self.assertEqual(subjects[2]["completed_at"], "2026-09-11T01:50:30Z")
        self.assertEqual([row["runner"] for row in hero["environment"]["platforms"]], [row[1] for row in status.TIER1])
        self.assertEqual(set(hero["stable_links"]), {"commit", "tree", "merged_pull_request", "ci_gate_run", "audit_run",
                                                     *(f"job_{row[1]}" for row in status.TIER1)})
        for forbidden in ("token", "login", "actor", "merged_at"):
            self.assertNotIn(forbidden, json.dumps(hero))
        status.validate(hero, self.repo, self.repo)
        self.assertTrue(self.manifest.read_text().endswith("}\n"))
        self.assertEqual(self.manifest.read_text(), json.dumps(document, indent=2, ensure_ascii=False) + "\n")

    def test_pagination_follows_the_link_header(self):
        runs = main_runs()
        first = self.responses.pop(f"{collector.API}/commits/{self.commit}/check-runs?per_page=100&page=1")
        self.responses[f"{collector.API}/commits/{self.commit}/check-runs?per_page=100&page=1"] = {
            "headers": {"Link": '<https://api.github.com/x?page=2>; rel="next", <https://api.github.com/x?page=2>; rel="last"'},
            "body": {"total_count": len(runs), "check_runs": runs[:3]}}
        self.responses[f"{collector.API}/commits/{self.commit}/check-runs?per_page=100&page=2"] = {
            "body": {"total_count": len(runs), "check_runs": runs[3:]}}
        code, _, err = self.run_collector()
        self.assertEqual(code, 0, err)
        self.assertEqual(first["body"]["total_count"], len(runs))

    def test_default_subject_and_default_collected_at(self):
        git(self.repo, "update-ref", "refs/remotes/origin/main", git(self.repo, "rev-parse", "HEAD"))
        subject = self.commit
        self.stub_file.write_text(json.dumps(self.responses))
        with contextlib.redirect_stdout(io.StringIO()):
            code = collector.main(["--repo-root", str(self.repo)])
        self.assertEqual(code, 0)
        hero = json.loads(self.manifest.read_text())["heroes"][1]
        self.assertEqual(hero["repository"]["commit"], subject)
        self.assertRegex(hero["attestation"]["collected_at"], r"^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z$")


class UnavailableTests(CollectorHarness):
    def api(self, suffix):
        return f"{collector.API}/{suffix}"

    def test_gh_failure_is_unavailable(self):
        self.responses[self.api(f"commits/{self.commit}")] = {"exit": 1, "stderr": "HTTP 403: rate limited"}
        self.assert_unavailable("rate limited")

    def test_invalid_json_is_unavailable(self):
        self.responses[self.api(f"commits/{self.commit}/pulls")] = {"raw": "{not json"}
        self.assert_unavailable("invalid JSON")

    def test_missing_body_is_unavailable(self):
        with mock.patch.object(collector.subprocess, "run", return_value=mock.Mock(returncode=0, stdout="HTTP/2.0 200 OK\n", stderr="")):
            with self.assertRaises(collector.Unavailable) as caught:
                collector.gh_api("anything")
        self.assertIn("returned no body", str(caught.exception))

    def test_malformed_commit_payload_is_unavailable(self):
        self.responses[self.api(f"commits/{self.commit}")] = {"body": {"sha": self.commit}}
        self.assert_unavailable("commit payload")
        self.responses[self.api(f"commits/{self.commit}")] = {"body": {"sha": "short", "parents": [], "commit": {"tree": {"sha": "x"}}}}
        self.assert_unavailable("commit payload")

    def test_pull_request_ambiguity_is_unavailable(self):
        self.responses[self.api(f"commits/{self.commit}/pulls")]["body"][1].update({"merged_at": "2026-09-11T00:00:00Z", "merge_commit_sha": self.commit})
        self.assert_unavailable("exactly one merged pull request")
        self.responses[self.api(f"commits/{self.commit}/pulls")] = {"body": []}
        self.assert_unavailable("found 0")
        self.responses[self.api(f"commits/{self.commit}/pulls")] = {"body": {"not": "a list"}}
        self.assert_unavailable("pulls payload")
        self.responses[self.api(f"commits/{self.commit}/pulls")] = {"body": [{"number": "1005", "merged_at": "x", "merge_commit_sha": self.commit, "head": {"sha": HEAD}}]}
        self.assert_unavailable("pull request payload")
        self.responses[self.api(f"commits/{self.commit}/pulls")] = {"body": [{"merged_at": "x", "merge_commit_sha": self.commit}]}
        self.assert_unavailable("pull request payload")

    def test_head_equal_to_merge_commit_is_unavailable(self):
        self.responses[self.api(f"commits/{self.commit}/pulls")]["body"][0]["head"]["sha"] = self.commit
        self.assert_unavailable("head equals the merge commit")

    def test_two_parent_subject_is_unavailable(self):
        self.responses[self.api(f"commits/{self.commit}")]["body"]["parents"].append({"sha": "4" * 40})
        self.assert_unavailable("2 parents")

    def test_head_tree_mismatch_is_unavailable(self):
        self.responses[self.api(f"commits/{HEAD}")]["body"]["commit"]["tree"]["sha"] = "5" * 40
        self.assert_unavailable("differs from the subject tree")

    def test_incomplete_check_run_collection_is_unavailable(self):
        path = self.api(f"commits/{self.commit}/check-runs?per_page=100&page=1")
        self.responses[path]["body"]["total_count"] = 99
        self.assert_unavailable("incomplete (7 of 99)")
        self.responses[path] = {"body": {"total_count": 1}}
        self.assert_unavailable("check-runs payload")

    def test_missing_ambiguous_or_foreign_app_runs_are_unavailable(self):
        path = self.api(f"commits/{HEAD}/check-runs?per_page=100&page=1")
        body = self.responses[path]["body"]
        body["check_runs"] = [check_run("ci-gate", 1, 2)]
        body["total_count"] = 1
        self.assert_unavailable("'audit' on " + HEAD + " is missing or ambiguous (0 found)")
        body["check_runs"] = [check_run("audit", AUDIT_RUN, 1), check_run("audit", AUDIT_RUN, 2)]
        body["total_count"] = 2
        self.assert_unavailable("(2 found)")
        body["check_runs"] = [check_run("audit", AUDIT_RUN, 1, app=1)]
        body["total_count"] = 1
        self.assert_unavailable("(0 found)")

    def test_non_success_or_in_progress_conclusions_are_unavailable(self):
        path = self.api(f"commits/{self.commit}/check-runs?per_page=100&page=1")
        runs = self.responses[path]["body"]["check_runs"]
        runs[0]["conclusion"] = "failure"
        self.assert_unavailable("ci-gate=failure")
        runs[0]["conclusion"] = "success"
        runs[2]["status"] = "in_progress"
        runs[2]["conclusion"] = None
        self.assert_unavailable("macos-14=None")
        runs[2]["status"] = "completed"
        runs[2]["conclusion"] = "skipped"
        self.assert_unavailable("macos-14=skipped")

    def test_tier_1_row_from_another_run_is_unavailable(self):
        path = self.api(f"commits/{self.commit}/check-runs?per_page=100&page=1")
        runs = self.responses[path]["body"]["check_runs"]
        runs[3] = check_run(status.TIER1[1][0], GATE_RUN + 1, 5)
        self.assert_unavailable("belongs to run")

    def test_missing_job_url_or_bad_timestamp_is_unavailable(self):
        path = self.api(f"commits/{self.commit}/check-runs?per_page=100&page=1")
        runs = self.responses[path]["body"]["check_runs"]
        runs[0]["html_url"] = f"{status.REPO}/actions/runs/{GATE_RUN}"
        self.assert_unavailable("no immutable job URL")
        runs[0]["html_url"] = f"{status.REPO}/actions/runs/{GATE_RUN}/job/1"
        runs[0]["completed_at"] = "2026-09-11 02:10:46"
        self.assert_unavailable("non-RFC 3339")

    def test_missing_milestone_line_is_unavailable(self):
        (self.repo / "docs" / "ROADMAP.md").write_text("# Roadmap\n")
        git(self.repo, "commit", "-qam", "drop milestone")
        subject = git(self.repo, "rev-parse", "HEAD")
        for path, value in list(self.responses.items()):
            self.responses[path.replace(self.commit, subject)] = copy.deepcopy(value)
        self.responses[f"{collector.API}/commits/{subject}"]["body"]["sha"] = subject
        self.responses[f"{collector.API}/commits/{subject}/pulls"]["body"][0]["merge_commit_sha"] = subject
        before = self.manifest.read_text()
        code, _, err = self.run_collector(subject=subject)
        self.assertEqual(code, 1)
        self.assertIn("no current-milestone line", err)
        self.assertEqual(self.manifest.read_text(), before)
        code, _, err = self.run_collector(subject="0" * 40)
        self.assertEqual(code, 1)
        self.assertIn("stub: unexpected path", err)

    def test_roadmap_missing_at_subject_is_unavailable(self):
        git(self.repo, "rm", "-q", "docs/ROADMAP.md")
        git(self.repo, "commit", "-qm", "remove roadmap")
        subject = git(self.repo, "rev-parse", "HEAD")
        for path, value in list(self.responses.items()):
            self.responses[path.replace(self.commit, subject)] = copy.deepcopy(value)
        self.responses[f"{collector.API}/commits/{subject}"]["body"]["sha"] = subject
        self.responses[f"{collector.API}/commits/{subject}/pulls"]["body"][0]["merge_commit_sha"] = subject
        code, _, err = self.run_collector(subject=subject)
        self.assertEqual(code, 1)
        self.assertIn("cannot read docs/ROADMAP.md", err)

    def test_built_record_rejected_by_the_validator_leaves_the_manifest_untouched(self):
        (self.repo / "docs" / "ROADMAP.md").write_text(ROADMAP.replace("v0.4 in progress", "v0.4 met"))
        before = self.manifest.read_text()
        code, _, err = self.run_collector()
        self.assertEqual(code, 1)
        self.assertIn("built record rejected", err)
        self.assertIn("milestone_line is stale", err)
        self.assertEqual(self.manifest.read_text(), before)


class ArgumentTests(CollectorHarness):
    def test_bad_collected_at_is_rejected(self):
        with contextlib.redirect_stderr(io.StringIO()):
            with self.assertRaises(SystemExit):
                collector.parse_args(["--collected-at", "yesterday"])

    def test_missing_manifest_and_missing_status_hero_fail(self):
        code, _, err = self.run_collector("--manifest", "site/absent.json")
        self.assertEqual(code, 1)
        self.assertIn("cannot read the status hero", err)
        self.manifest.write_text(json.dumps({"heroes": [{"page_id": "landing"}]}))
        code, _, err = self.run_collector()
        self.assertEqual(code, 1)
        self.assertIn("cannot read the status hero", err)
        absolute = self.root / "elsewhere.json"
        absolute.write_text(json.dumps(self.document))
        code, _, err = self.run_collector("--manifest", str(absolute))
        self.assertEqual(code, 0, err)
        self.assertEqual(json.loads(absolute.read_text())["heroes"][1]["state"], "all-Tier-1")

    def test_unresolvable_origin_main_fails(self):
        self.stub_file.write_text(json.dumps(self.responses))
        err = io.StringIO()
        with contextlib.redirect_stderr(err):
            code = collector.main(["--repo-root", str(self.repo)])
        self.assertEqual(code, 1)
        self.assertIn("cannot resolve origin/main", err.getvalue())


if __name__ == "__main__":
    unittest.main()
