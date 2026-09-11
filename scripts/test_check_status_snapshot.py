"""Shallow-safe tests for the status hero's required-check snapshot (D-241).

Every case uses in-memory records and throwaway ``git init`` repositories.
Nothing here runs ``check-site.sh`` or ``check_site_evidence.py`` against the
real manifest: those resolve preserved trees that a depth-1 governance checkout
does not have.  Real-record cases live in the Pages-only
``site_status_evidence_test.py``.
"""

import contextlib
import copy
import hashlib
import io
import json
import re
import subprocess
import tempfile
import unittest
from pathlib import Path

import check_status_snapshot
import site_status_evidence as status


MILESTONE = "Current milestone: v0.3 — acceptance criteria met; v0.4 in progress."
ROADMAP = (
    "# Roadmap\n\n**" + MILESTONE + "** v0.2 was met earlier.\n\n"
    "- [x] something (roadmap-evidence: example)\n"
)
COLLECTOR_SOURCE = "print('synthetic collector')\n"
TEST_SOURCE = "class T:\n    def test_synthetic_case(self):\n        pass\n"
GATE_RUN = 34552872912
AUDIT_RUN = 34552229293


def sha256(text):
    return hashlib.sha256(text.encode()).hexdigest()


def git(root, *args):
    return subprocess.run(["git", "-C", str(root), *args], capture_output=True, text=True, check=True).stdout.strip()


def make_repo(root, merges=1):
    """A synthetic default branch: root commit, then ``merges`` squash-shaped commits."""
    subprocess.run(["git", "init", "-q", "-b", "main", str(root)], check=True)
    git(root, "config", "user.email", "test@example.test")
    git(root, "config", "user.name", "Status Snapshot Test")
    git(root, "config", "commit.gpgsign", "false")
    (root / "docs").mkdir()
    (root / "docs" / "ROADMAP.md").write_text(ROADMAP)
    (root / "site").mkdir()
    (root / "site" / "evidence-heroes.json").write_text(json.dumps({"heroes": [{"page_id": "status", "state": "unavailable"}]}) + "\n")
    git(root, "add", ".")
    git(root, "commit", "-qm", "root")
    shas = []
    for index in range(merges):
        (root / f"file-{index}.txt").write_text(f"{index}\n")
        git(root, "add", ".")
        git(root, "commit", "-qm", f"merge {index}")
        shas.append(git(root, "rev-parse", "HEAD"))
    return shas


def make_evidence_root(root):
    scripts = root / "scripts"
    scripts.mkdir(exist_ok=True)
    (scripts / "collect_status_snapshot.py").write_text(COLLECTOR_SOURCE)
    (scripts / "test_check_status_snapshot.py").write_text(TEST_SOURCE)


def record(commit, tree, head_sha="a" * 40):
    def check(identity, label, name, sha, run, job, completed):
        path, event = status.EXPECTED_WORKFLOWS[name]
        return {
            "id": identity, "label": label, "sha": sha, "check": name, "app_id": status.APP_ID,
            "conclusion": "success", "completed_at": completed, "run_id": run,
            "run_url": f"{status.REPO}/actions/runs/{run}", "job_url": f"{status.REPO}/actions/runs/{run}/job/{job}",
            "workflow_path": path, "event": event,
        }
    platforms = [
        {"runner": runner, "architecture": architecture, "check_run_name": name, "conclusion": "success",
         "job_url": f"{status.REPO}/actions/runs/{GATE_RUN}/job/{103119276900 + index}"}
        for index, (name, runner, architecture) in enumerate(status.TIER1)
    ]
    hero = {
        "page_id": "status",
        "evidence_id": "status-snapshot-v1",
        "kind": status.KIND,
        "route": "/status/",
        "page_path": "site/status/index.html",
        "fixture": {"path": status.COLLECTOR, "sha256": sha256(COLLECTOR_SOURCE)},
        "test": {"path": status.TEST, "sha256": sha256(TEST_SOURCE), "names": ["test_synthetic_case"]},
        "command": {"cwd": "repository-root", "argv": ["python3", status.COLLECTOR, "--subject", commit],
                    "requires": status.REQUIRES},
        "snapshot": {"subjects": [
            {"id": "published-revision", "label": "Published revision (default branch)", "sha": commit,
             "check": None, "app_id": None, "conclusion": None, "completed_at": None, "run_id": None,
             "run_url": None, "job_url": None, "workflow_path": None, "event": None, "tree": tree, "parent_count": 1,
             "merged_pull_request": {"number": 1005, "head_sha": head_sha, "head_tree": tree,
                                     "url": f"{status.REPO}/pull/1005", "merged_at": "2026-09-11T01:59:50Z"}},
            check("post-merge-ci-gate", "Post-merge CI gate", "ci-gate", commit, GATE_RUN, 103121414776, "2026-09-11T02:10:46Z"),
            check("pre-merge-audit", "Pre-merge policy audit", "audit", head_sha, AUDIT_RUN, 103117345163, "2026-09-11T01:50:30Z"),
        ]},
        "repository": {"commit": commit, "tree": tree, "url": f"{status.REPO}/commit/{commit}"},
        "attestation": {"collected_at": "2026-09-11T03:00:00Z", "collection_method": status.COLLECTION_METHOD,
                        "sanitized": True, "milestone_line": MILESTONE, "required_contexts": list(status.REQUIRED_CONTEXTS)},
        "environment": {"platforms": platforms},
        "state": "all-Tier-1",
        "limitations": status.LIMITATIONS,
        "stable_links": None,
        "projections": {"html": "site/status/index.html", "markdown": "site/index.html.md", "llm": "site/llms.txt",
                        "structured_data": "site/status/index.html", "social": "site/status/index.html"},
    }
    hero["stable_links"] = status.expected_links(hero)
    return hero


class SyntheticRepository(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory(prefix="status-snapshot-")
        self.root = Path(self.directory.name)
        self.repo = self.root / "repo"
        self.repo.mkdir()
        self.evidence = self.root / "evidence"
        self.evidence.mkdir()
        make_evidence_root(self.evidence)
        self.commits = make_repo(self.repo, merges=1)
        self.commit = self.commits[0]
        self.tree = git(self.repo, "rev-parse", f"{self.commit}^{{tree}}")
        self.hero = record(self.commit, self.tree)

    def tearDown(self):
        self.directory.cleanup()

    def mutated(self, mutate):
        hero = copy.deepcopy(self.hero)
        mutate(hero)
        return hero

    def assert_rejected(self, mutate, fragment):
        hero = self.mutated(mutate)
        with self.assertRaises(SystemExit) as caught:
            status.validate(hero, self.evidence, self.repo)
        self.assertIn(fragment, str(caught.exception))

    def run_cli(self, argv):
        with contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()):
            return check_status_snapshot.main(argv)

    def write_manifest(self, hero, path=None):
        path = path or (self.root / "manifest.json")
        path.write_text(json.dumps({"schema_version": "2.1.0", "heroes": [hero]}) + "\n")
        return path


class RecordInvariantTests(SyntheticRepository):
    def test_consistent_record_is_accepted_and_verified_against_git(self):
        status.validate(self.hero, self.evidence, self.repo)
        status.verify_git(self.hero, self.repo)
        self.assertEqual(status.derive_state(self.hero), "all-Tier-1")

    def test_every_nested_field_is_required(self):
        shape = status.expected_shape()
        for field in ("fixture", "test", "command", "snapshot", "repository", "attestation", "environment"):
            for key in sorted(shape[field]):
                self.assert_rejected(lambda hero, f=field, k=key: hero[f].pop(k), "fields drifted")
            self.assert_rejected(lambda hero, f=field: hero[f].__setitem__("extra", 1), "fields drifted")
        for index, key in [(0, "merged_pull_request"), (0, "tree"), (0, "parent_count"), (1, "run_url"), (2, "job_url"),
                           (1, "workflow_path"), (2, "event")]:
            self.assert_rejected(lambda hero, i=index, k=key: hero["snapshot"]["subjects"][i].pop(k), "fields drifted")
        for key in ("head_tree", "merged_at"):
            self.assert_rejected(lambda hero, k=key: hero["snapshot"]["subjects"][0]["merged_pull_request"].pop(k), "fields drifted")
        self.assert_rejected(lambda hero: hero["environment"]["platforms"][0].pop("job_url"), "fields drifted")

    def test_head_tree_mismatch_is_rejected(self):
        self.assert_rejected(lambda hero: hero["snapshot"]["subjects"][0]["merged_pull_request"].__setitem__("head_tree", "b" * 40),
                             "head_tree must equal the published tree")

    def test_another_pull_request_head_is_rejected_when_audit_stays_on_the_old_head(self):
        def mutate(hero):
            hero["snapshot"]["subjects"][0]["merged_pull_request"]["head_sha"] = "c" * 40
        self.assert_rejected(mutate, "pre-merge-audit must be observed on the merged pull request head")

    def test_wrong_app_id_is_rejected(self):
        self.assert_rejected(lambda hero: hero["snapshot"]["subjects"][1].__setitem__("app_id", 1), "app_id must be 15368")

    def test_checks_are_bound_to_their_workflow_file_and_event(self):
        cases = [
            (2, "workflow_path", ".github/workflows/ci.yml",
             "pre-merge-audit must be observed in run of .github/workflows/workflow-policy.yml under pull_request_target, "
             "not '.github/workflows/ci.yml' under 'pull_request_target'"),
            (2, "event", "pull_request", "not '.github/workflows/workflow-policy.yml' under 'pull_request'"),
            (1, "workflow_path", ".github/workflows/workflow-policy.yml",
             "post-merge-ci-gate must be observed in run of .github/workflows/ci.yml under push"),
            (1, "event", "pull_request", "not '.github/workflows/ci.yml' under 'pull_request'"),
            (1, "event", None, "not '.github/workflows/ci.yml' under None"),
            (0, "workflow_path", ".github/workflows/ci.yml", "published-revision workflow_path must be null"),
            (0, "event", "push", "published-revision event must be null"),
        ]
        for index, field, value, fragment in cases:
            with self.subTest(index=index, field=field, value=value):
                self.assert_rejected(lambda hero, i=index, f=field, v=value: hero["snapshot"]["subjects"][i].__setitem__(f, v), fragment)

    def test_audit_must_predate_the_merge_and_the_gate_must_follow_it(self):
        merged = lambda hero: hero["snapshot"]["subjects"][0]["merged_pull_request"]
        for value in ("2026-09-11T01:50:29Z", "2026-09-11T00:00:00Z"):
            self.assert_rejected(lambda hero, v=value: merged(hero).__setitem__("merged_at", v),
                                 "pre-merge-audit must complete no later than the pull request merged")
        self.assert_rejected(lambda hero: hero["snapshot"]["subjects"][2].__setitem__("completed_at", "2026-09-11T01:59:51Z"),
                             "pre-merge-audit must complete no later than the pull request merged")
        for value in ("2026-09-11T02:10:47Z", "2026-09-11T03:00:00Z"):
            self.assert_rejected(lambda hero, v=value: merged(hero).__setitem__("merged_at", v),
                                 "post-merge-ci-gate must complete no earlier than the pull request merged")
        for value in ("2026-09-11T01:59:50", "2026-09-11 01:59:50Z", None, 1):
            self.assert_rejected(lambda hero, v=value: merged(hero).__setitem__("merged_at", v),
                                 "merged_at must be an RFC 3339 UTC timestamp")
        for value in ("2026-09-11T01:50:30Z", "2026-09-11T02:10:46Z"):
            hero = self.mutated(lambda hero, v=value: merged(hero).__setitem__("merged_at", v))
            self.assertIsNone(status.validate(hero, self.evidence, self.repo))

    def test_missing_merged_pull_request_is_rejected(self):
        self.assert_rejected(lambda hero: hero["snapshot"]["subjects"][0].__setitem__("merged_pull_request", None), "must be an object")

    def test_unequal_trees_are_rejected(self):
        self.assert_rejected(lambda hero: hero["repository"].__setitem__("tree", "d" * 40), "published-revision must be the repository commit and tree")
        self.assert_rejected(lambda hero: hero["snapshot"]["subjects"][0].__setitem__("tree", "d" * 40), "published-revision must be the repository commit and tree")

    def test_two_parent_subject_is_rejected(self):
        self.assert_rejected(lambda hero: hero["snapshot"]["subjects"][0].__setitem__("parent_count", 2), "exactly one parent")

    def test_parent_count_that_only_compares_equal_to_one_is_rejected(self):
        for value in (True, 1.0, "1"):
            with self.subTest(value=value):
                self.assert_rejected(lambda hero, value=value: hero["snapshot"]["subjects"][0].__setitem__("parent_count", value), "exactly one parent")

    def test_incomplete_collection_is_rejected(self):
        self.assert_rejected(lambda hero: hero["environment"]["platforms"].pop(), "five Tier-1 jobs")
        self.assert_rejected(lambda hero: hero["snapshot"]["subjects"].pop(1), "three ordered subjects")
        self.assert_rejected(lambda hero: hero["environment"]["platforms"][0].__setitem__("check_run_name", "governance"),
                             "closed Tier-1 list")
        duplicated = self.mutated(lambda hero: hero["environment"]["platforms"].__setitem__(1, dict(hero["environment"]["platforms"][0])))
        with self.assertRaises(SystemExit):
            status.validate(duplicated, self.evidence, self.repo)

    def test_relabelled_checks_are_rejected(self):
        self.assert_rejected(lambda hero: hero["snapshot"]["subjects"][2].__setitem__("sha", hero["repository"]["commit"]),
                             "pre-merge-audit must be observed on the merged pull request head")
        self.assert_rejected(lambda hero: hero["snapshot"]["subjects"][1].__setitem__("sha", "a" * 40),
                             "post-merge-ci-gate must be observed on the repository commit")
        self.assert_rejected(lambda hero: hero["snapshot"]["subjects"][1].__setitem__("check", "audit"), "check must be 'ci-gate'")
        self.assert_rejected(lambda hero: hero["snapshot"]["subjects"][2].__setitem__("check", "ci-gate"), "check must be 'audit'")

    def test_head_equal_to_merge_commit_is_rejected(self):
        def mutate(hero):
            hero["snapshot"]["subjects"][0]["merged_pull_request"]["head_sha"] = hero["repository"]["commit"]
            hero["snapshot"]["subjects"][2]["sha"] = hero["repository"]["commit"]
        self.assert_rejected(mutate, "head_sha must differ from the merge commit")

    def test_non_success_conclusions_with_all_tier_1_state_are_rejected(self):
        for value in (None, "skipped", "failure", "cancelled", "in_progress"):
            self.assert_rejected(lambda hero, v=value: hero["snapshot"]["subjects"][1].__setitem__("conclusion", v), "conclusion must be success")
            self.assert_rejected(lambda hero, v=value: hero["snapshot"]["subjects"][2].__setitem__("conclusion", v), "conclusion must be success")
            self.assert_rejected(lambda hero, v=value: hero["environment"]["platforms"][3].__setitem__("conclusion", v), "conclusion must be success")
            self.assertEqual(status.derive_state(self.mutated(lambda hero, v=value: hero["environment"]["platforms"][3].__setitem__("conclusion", v))), "unavailable")

    def test_state_must_match_the_derived_state(self):
        for value in ("partial", "experimental", "superseded", "unavailable"):
            self.assert_rejected(lambda hero, v=value: hero.__setitem__("state", v), "does not match the derived state")
        self.assertEqual(status.derive_state({"snapshot": None, "environment": None}), "unavailable")
        self.assertEqual(status.derive_state({"snapshot": {"subjects": "x"}, "environment": {"platforms": [1, 2, 3, 4, 5]}}), "unavailable")

    def test_non_immutable_links_are_rejected(self):
        self.assert_rejected(lambda hero: hero["snapshot"]["subjects"][1].__setitem__("run_url", f"{status.REPO}/actions/runs/main"), "immutable run URL")
        self.assert_rejected(lambda hero: hero["snapshot"]["subjects"][1].__setitem__("run_id", GATE_RUN + 1), "immutable run URL")
        self.assert_rejected(lambda hero: hero["snapshot"]["subjects"][1].__setitem__("run_id", "34552872912"), "positive integer")
        self.assert_rejected(lambda hero: hero["snapshot"]["subjects"][2].__setitem__("job_url", f"{status.REPO}/actions/runs/{GATE_RUN}/job/1"), "immutable job URL")
        self.assert_rejected(lambda hero: hero["environment"]["platforms"][2].__setitem__("job_url", f"{status.REPO}/actions/runs/{AUDIT_RUN}/job/1"), "under the ci-gate run")
        self.assert_rejected(lambda hero: hero["repository"].__setitem__("url", f"{status.REPO}/tree/main"), "immutable commit URL")
        self.assert_rejected(lambda hero: hero["snapshot"]["subjects"][0]["merged_pull_request"].__setitem__("url", f"{status.REPO}/pull/1"), "immutable pull URL")
        self.assert_rejected(lambda hero: hero["stable_links"].__setitem__("commit", f"{status.REPO}/commit/main"), "stable_links must be exactly")
        self.assert_rejected(lambda hero: hero["stable_links"].pop("job_macos-14"), "stable_links must be exactly")

    def test_timestamps_must_be_rfc3339_utc(self):
        for value in ("2026-09-11T03:00:00+00:00", "2026-09-11 03:00:00Z", "2026-09-11", None, 3,
                      "2026-99-99T99:99:99Z", "2026-02-30T00:00:00Z", "2026-09-11T24:00:00Z"):
            self.assert_rejected(lambda hero, v=value: hero["attestation"].__setitem__("collected_at", v), "RFC 3339 UTC")
            self.assert_rejected(lambda hero, v=value: hero["snapshot"]["subjects"][1].__setitem__("completed_at", v), "RFC 3339 UTC")

    def test_capture_time_must_not_precede_check_completion(self):
        for value in ("2000-01-01T00:00:00Z", "2026-09-11T01:50:29Z"):
            self.assert_rejected(lambda hero, v=value: hero["attestation"].__setitem__("collected_at", v), "no earlier than every recorded completed_at")

    def test_capture_time_must_not_be_in_the_future(self):
        for value in ("2099-01-01T00:00:00Z", "9999-12-31T23:59:59Z"):
            self.assert_rejected(lambda hero, v=value: hero["attestation"].__setitem__("collected_at", v), "must not be later than the validation time")
        self.assertRegex(status.utc_now(), r"^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z$")

    def test_platform_rows_must_link_distinct_jobs(self):
        def duplicate_row(hero):
            hero["environment"]["platforms"][1]["job_url"] = hero["environment"]["platforms"][0]["job_url"]

        def reuse_gate_job(hero):
            hero["environment"]["platforms"][3]["job_url"] = hero["snapshot"]["subjects"][1]["job_url"]

        self.assert_rejected(duplicate_row, "distinct jobs")
        self.assert_rejected(reuse_gate_job, "distinct jobs")

    def test_audit_must_come_from_a_run_distinct_from_the_gate(self):
        def reuse_gate_run(hero):
            gate, audit = hero["snapshot"]["subjects"][1], hero["snapshot"]["subjects"][2]
            for key in ("run_id", "run_url", "job_url"):
                audit[key] = gate[key]

        def reuse_audit_job(hero):
            hero["environment"]["platforms"][4]["job_url"] = hero["snapshot"]["subjects"][2]["job_url"]

        self.assert_rejected(reuse_gate_run, "distinct from the ci-gate run")
        self.assert_rejected(reuse_audit_job, "under the ci-gate run")

    def test_attestation_and_command_drift_are_rejected(self):
        self.assert_rejected(lambda hero: hero["attestation"].__setitem__("sanitized", "yes"), "collection_method/sanitized")
        self.assert_rejected(lambda hero: hero["attestation"].__setitem__("collection_method", "curl"), "collection_method/sanitized")
        self.assert_rejected(lambda hero: hero["attestation"].__setitem__("required_contexts", ["ci-gate"]), "required_contexts")
        self.assert_rejected(lambda hero: hero["command"].__setitem__("argv", ["python3", status.COLLECTOR]), "argv must invoke")
        self.assert_rejected(lambda hero: hero["command"].__setitem__("cwd", "/tmp"), "cwd/requires")
        self.assert_rejected(lambda hero: hero.__setitem__("limitations", "all-Tier-1 proves release readiness"), "limitations drifted")
        self.assert_rejected(lambda hero: hero.__setitem__("kind", "native-build-output"), "applies only to the status hero")
        self.assert_rejected(lambda hero: hero["snapshot"]["subjects"][0].__setitem__("label", "Revision"), "label must be")
        self.assert_rejected(lambda hero: hero["snapshot"]["subjects"][0].__setitem__("conclusion", "success"), "a revision is not a check")
        self.assert_rejected(lambda hero: hero["repository"].__setitem__("commit", "abc"), "full lowercase SHA")
        self.assert_rejected(lambda hero: hero["snapshot"]["subjects"][1].__setitem__("sha", "ABC"), "full lowercase SHA")
        self.assert_rejected(lambda hero: hero["snapshot"]["subjects"][0]["merged_pull_request"].__setitem__("head_sha", 5), "full lowercase SHA")
        self.assert_rejected(lambda hero: hero["snapshot"]["subjects"][0]["merged_pull_request"].__setitem__("number", "1005"), "positive integer")

    def test_milestone_line_must_be_a_current_milestone_lead(self):
        for value in (None, 7, "", "v0.3 — acceptance criteria met"):
            with self.subTest(value=value):
                self.assert_rejected(lambda hero, value=value: hero["attestation"].__setitem__("milestone_line", value), "current-milestone lead")
        self.assertIsNone(status.milestone_lead("**Current milestone: unterminated"))

    def test_pinned_scripts_and_test_names_are_verified(self):
        self.assert_rejected(lambda hero: hero["fixture"].__setitem__("sha256", "0" * 64), "fixture sha256 differs")
        self.assert_rejected(lambda hero: hero["test"].__setitem__("sha256", "0" * 64), "test sha256 differs")
        self.assert_rejected(lambda hero: hero["fixture"].__setitem__("path", "scripts/other.py"), "fixture path must be")
        self.assert_rejected(lambda hero: hero["test"].__setitem__("names", ["test_missing"]), "test is not registered")
        self.assert_rejected(lambda hero: hero["test"].__setitem__("names", []), "non-empty list")
        self.assert_rejected(lambda hero: hero["test"].__setitem__("names", ["test_synthetic_case", "test_synthetic_case"]),
                             "every registered test in source order")
        (self.evidence / "scripts" / "test_check_status_snapshot.py").write_text(
            TEST_SOURCE + "    def test_added_later(self):\n        pass\n")
        self.assert_rejected(lambda hero: hero["test"].__setitem__("sha256", sha256(
            TEST_SOURCE + "    def test_added_later(self):\n        pass\n")), "every registered test in source order")
        (self.evidence / "scripts" / "test_check_status_snapshot.py").write_text(TEST_SOURCE)
        (self.evidence / "scripts" / "collect_status_snapshot.py").unlink()
        self.assert_rejected(lambda hero: None, "missing or unsafe")


class GitObjectTests(SyntheticRepository):
    def test_subject_not_an_ancestor_of_head_is_rejected(self):
        git(self.repo, "checkout", "-qb", "side", f"{self.commit}^")
        (self.repo / "side.txt").write_text("side\n")
        git(self.repo, "add", ".")
        git(self.repo, "commit", "-qm", "side")
        with self.assertRaises(SystemExit) as caught:
            status.verify_git(self.hero, self.repo)
        self.assertIn("not on the first-parent history of HEAD", str(caught.exception))

    def merge_subject_through_second_parent(self):
        """Rebuild main so the subject is reachable from HEAD only as a merge commit's second parent."""
        git(self.repo, "branch", "-q", "side", self.commit)
        git(self.repo, "reset", "-q", "--hard", f"{self.commit}^")
        (self.repo / "mainline.txt").write_text("mainline\n")
        git(self.repo, "add", ".")
        git(self.repo, "commit", "-qm", "mainline")
        git(self.repo, "merge", "-q", "--no-ff", "--no-edit", "side")
        head = git(self.repo, "rev-parse", "HEAD")
        self.assertEqual(git(self.repo, "rev-parse", "HEAD^2"), self.commit)
        self.assertEqual(subprocess.run(["git", "-C", str(self.repo), "merge-base", "--is-ancestor", self.commit, head]).returncode, 0)
        return head

    def test_subject_reachable_only_through_a_second_parent_is_rejected(self):
        self.merge_subject_through_second_parent()
        with self.assertRaises(SystemExit) as caught:
            status.verify_git(self.hero, self.repo)
        self.assertIn("not on the first-parent history of HEAD", str(caught.exception))
        self.assertTrue(status.on_first_parent_history(self.repo, self.commit, "side"))

    def test_two_parent_subject_commit_is_rejected_from_git(self):
        git(self.repo, "checkout", "-qb", "topic", f"{self.commit}^")
        (self.repo / "topic.txt").write_text("topic\n")
        git(self.repo, "add", ".")
        git(self.repo, "commit", "-qm", "topic")
        git(self.repo, "checkout", "-q", "main")
        git(self.repo, "merge", "-q", "--no-ff", "-m", "merge topic", "topic")
        merge = git(self.repo, "rev-parse", "HEAD")
        hero = record(merge, git(self.repo, "rev-parse", f"{merge}^{{tree}}"))
        with self.assertRaises(SystemExit) as caught:
            status.verify_git(hero, self.repo)
        self.assertIn("exactly one parent", str(caught.exception))

    def test_milestone_line_is_proved_against_the_subject_not_the_working_tree(self):
        hero = self.mutated(lambda hero: hero["attestation"].__setitem__("milestone_line", "Current milestone: v0.2 — met."))
        with self.assertRaises(SystemExit) as caught:
            status.verify_git(hero, self.repo)
        self.assertIn(f"differs from docs/ROADMAP.md at {self.commit}", str(caught.exception))
        # A milestone-transition pull request rewrites the working-tree roadmap while the
        # pinned subject keeps its own line: both checks still accept the record.
        (self.repo / "docs" / "ROADMAP.md").write_text(ROADMAP.replace("v0.4 in progress", "v0.4 — acceptance criteria met"))
        status.validate(self.hero, self.evidence, self.repo)
        status.verify_git(self.hero, self.repo)
        (self.repo / "docs" / "ROADMAP.md").write_text("# Roadmap\n\nno milestone line\n")
        status.verify_git(self.hero, self.repo)

    def test_recorded_tree_must_match_git(self):
        hero = self.mutated(lambda hero: None)
        hero["repository"]["tree"] = hero["snapshot"]["subjects"][0]["tree"] = "e" * 40
        hero["snapshot"]["subjects"][0]["merged_pull_request"]["head_tree"] = "e" * 40
        with self.assertRaises(SystemExit) as caught:
            status.verify_git(hero, self.repo)
        self.assertIn("tree differs from the recorded tree", str(caught.exception))


def render_page(hero):
    """A minimal status page whose hero carries one visible row per subject and per Tier-1 platform."""
    subjects = subject_by_id = {item["id"]: item for item in hero["snapshot"]["subjects"]}
    revision = subject_by_id["published-revision"]
    merged = revision["merged_pull_request"]
    links = hero["stable_links"]
    rows = [f'<dt>{revision["label"]}</dt><dd><a href="{links["commit"]}">{revision["sha"]}</a> · one parent · '
            f'<a href="{links["tree"]}">tree</a> <code>{hero["repository"]["tree"]}</code> · '
            f'<a href="{merged["url"]}">#{merged["number"]}</a> head {merged["head_sha"]}, same tree</dd>']
    for item in (subjects["post-merge-ci-gate"], subjects["pre-merge-audit"]):
        rows.append(f'<dt>{item["label"]}</dt><dd>{item["check"]} on {item["sha"]} · App {status.APP_ID} · '
                    f'{item["conclusion"]} · completed {item["completed_at"]} · '
                    f'<a href="{item["run_url"]}">run {item["run_id"]}</a> · <a href="{item["job_url"]}">job</a></dd>')
    items = [f'<li><a href="{row["job_url"]}">{status.platform_row_text(row)}</a></li>'
             for row in hero["environment"]["platforms"]]
    return ('<html lang="en-US"><head><meta property="og:locale" content="en_US">'
            '<script type="application/ld+json">{"dateModified": "2026-01-01"}</script></head><body>'
            '<main id="main-content" class="content-page">'
            f'<header data-evidence-role="hero" data-evidence-id="{hero["evidence_id"]}">'
            '<p class="eyebrow">Evidence page · Updated 2026-01-01</p><h1>What pycc can do <span>today.</span></h1>'
            f'<p class="page-lede">{status.HERO_MASTHEAD[1]}</p>'
            '<div class="page-meta">'
            f'<span data-evidence-id="{hero["evidence_id"]}"><strong>Evidence hero</strong> {hero["state"]} · snapshot of main '
            f'{hero["repository"]["commit"][:8]} · ci-gate {subjects["post-merge-ci-gate"]["conclusion"]} · '
            f'audit {subjects["pre-merge-audit"]["conclusion"]} (PR #{merged["number"]}) · '
            f'captured {hero["attestation"]["collected_at"]}</span>'
            '<span><strong>Milestone</strong> v0.3 acceptance criteria met, released as v0.3.0; v0.4 in progress</span>'
            '<span><strong>Acceptance</strong> v0.1, v0.2, and v0.3 all fully met</span>'
            '<span><strong>Readiness</strong> pre-alpha</span></div>'
            f'<details><summary>{status.HERO_DETAILS_TOGGLE}</summary>'
            f'<dl>{"".join(rows)}<dt>{status.TIER1_HEADING}</dt><dd>{status.TIER1_HEADING_ROW}</dd></dl><ul>{"".join(items)}</ul>'
            f'<p>{status.expected_closing_paragraph(hero)}</p></details></header></main></body></html>\n')


class ProjectionCase(SyntheticRepository):
    def write_site(self, hero, page=None):
        site = self.root / "site"
        (site / "status").mkdir(parents=True, exist_ok=True)
        (site / "status" / "index.html").write_text(page or render_page(hero))
        for name in ("index.html.md", "llms.txt"):
            (site / name).write_text(f"# central\n\n{status.summary(hero)}\n")
        if not (site / "styles.css").exists():
            (site / "styles.css").write_text(".unrelated { display: none; }\nfooter p { display: none; }\n")
        return site

    def swap(self, page, first, second):
        self.assertIn(first, page)
        self.assertIn(second, page)
        return page.replace(first, "\0", 1).replace(second, first, 1).replace("\0", second, 1)

    def assert_page_rejected(self, page, fragment):
        site = self.write_site(self.hero, page)
        with self.assertRaises(SystemExit) as caught:
            status.validate_projection(self.hero, self.repo, site)
        self.assertIn(fragment, str(caught.exception))


class ProjectionTests(ProjectionCase):
    def test_rendered_rows_bound_to_their_subjects_are_accepted(self):
        site = self.write_site(self.hero)
        self.assertIsNone(status.validate_projection(self.hero, self.repo, site))

    def test_swapped_subject_evidence_is_rejected(self):
        page = render_page(self.hero)
        gate, audit = [subject for subject in self.hero["snapshot"]["subjects"][1:]]
        rejected = "must read exactly as that subject's own sha, check, conclusion, time and links"
        for name, first, second in (
            ("run links", f'href="{gate["run_url"]}"', f'href="{audit["run_url"]}"'),
            ("job links", f'href="{gate["job_url"]}"', f'href="{audit["job_url"]}"'),
            ("shas", f'on {gate["sha"]}', f'on {audit["sha"]}'),
            ("completion times", f'completed {gate["completed_at"]}', f'completed {audit["completed_at"]}'),
            ("labels", f'<dt>{gate["label"]}</dt>', f'<dt>{audit["label"]}</dt>'),
            ("checks", f'<dd>{gate["check"]} on', f'<dd>{audit["check"]} on'),
        ):
            with self.subTest(swapped=name):
                self.assert_page_rejected(self.swap(page, first, second), rejected)
        with self.subTest(swapped="published revision links"):
            first, second = self.hero["stable_links"]["commit"], self.hero["stable_links"]["tree"]
            self.assert_page_rejected(self.swap(page, f'href="{first}"', f'href="{second}"'), rejected)
        with self.subTest(contradiction="conclusion"):
            contradicted = page.replace(f'{gate["conclusion"]} · completed', f'failure (recorded {gate["conclusion"]}) · completed', 1)
            self.assert_page_rejected(contradicted, rejected)
        with self.subTest(contradiction="extra sha"):
            self.assert_page_rejected(page.replace(f'on {gate["sha"]}', f'on {gate["sha"]} (was {audit["sha"]})', 1), rejected)
        with self.subTest(missing="audit row"):
            self.assert_page_rejected(page.replace(f'<dt>{audit["label"]}</dt>', "<dt>Audit</dt>", 1),
                                      f"proof row missing for {audit['label']}")

    def test_platform_rows_are_bound_to_their_jobs(self):
        page = render_page(self.hero)
        first, second = self.hero["environment"]["platforms"][:2]
        with self.subTest(swapped="job links"):
            self.assert_page_rejected(self.swap(page, f'href="{first["job_url"]}"', f'href="{second["job_url"]}"'),
                                      "reading exactly as its own runner, target, conclusion and job link")
        with self.subTest(swapped="runner and target"):
            self.assert_page_rejected(page.replace(f'· {first["runner"]} · {first["architecture"]} ·', f'· {first["runner"]} · {second["architecture"]} ·', 1),
                                      "reading exactly as its own runner, target, conclusion and job link")
        with self.subTest(contradiction="conclusion"):
            self.assert_page_rejected(page.replace(f'{first["architecture"]} · {first["conclusion"]}',
                                                   f'{first["architecture"]} · failure (recorded {first["conclusion"]})', 1),
                                      "reading exactly as its own runner, target, conclusion and job link")
        with self.subTest(mutation="dropped row"):
            start = page.index("<li>")
            self.assert_page_rejected(page[:start] + page[page.index("</li>", start) + 5:],
                                      "exactly one visible row per Tier-1 platform")
        with self.subTest(mutation="duplicated row"):
            start = page.index("<li>")
            end = page.index("</li>", start) + 5
            self.assert_page_rejected(page[:end] + page[start:end] + page[end:], "exactly one visible row per Tier-1 platform")
        with self.subTest(mutation="duplicated row in place of another"):
            start = page.index("<li>")
            end = page.index("</li>", start) + 5
            second_start = page.index("<li>", end)
            second_end = page.index("</li>", second_start) + 5
            self.assert_page_rejected(page[:second_start] + page[start:end] + page[second_end:],
                                      f"Tier-1 row for {first['check_run_name']} must appear exactly once")

    def test_platform_row_text_repeats_runner_and_target_only_when_the_job_name_lacks_them(self):
        rows = self.hero["environment"]["platforms"]
        self.assertEqual(status.platform_row_text(rows[0]), "build-test-coverage · macos-14 · aarch64-apple-darwin · success")
        self.assertEqual(status.platform_row_text(rows[1]), "native-build-test (macos-15-intel, x86_64-apple-darwin) · success")

    def test_stylesheet_rules_hiding_the_hero_are_rejected(self):
        page = render_page(self.hero)
        for rule in (".hero-row { display: none; }", "header dd { visibility: hidden; }", "body header li { display:none }",
                     "@media (max-width: 980px) { dl dt, .other { display: none; } }", "* { display: none; }",
                     "[data-evidence-role] { display: none; }", "header > dl > dd:nth-child(2) { display: none; }",
                     "HEADER DD { DISPLAY: NONE; }", "dl dt { Visibility : Hidden }", ".page-meta span { display:NONE }",
                     "header { opacity: 0; }", "dl { opacity: 0.0 }", "header dd { visibility: collapse }", "header { content-visibility: hidden }",
                     "header li { font-size: 0 }", "header dl { transform: translateY(1px) scale(0) }",
                     ".content-page { display: none; }", "#main-content { opacity: 0 }", "body { display:none }",
                     "html body main { font-size: 0 }", "main > header { opacity: .0 }", "header dd { font-size: .0px }",
                     "header { transform: scale(.00) }", "dl { opacity: 0. }",
                     "header { opacity: -0 }", "dl { opacity: 0e0 }", "header dd { font-size: +0.0E-1px }", "dl { transform: scale(-.0e2) }",
                     "header { transform: scale(1, 0) }", "dl { transform: scale3d(1, 1, 0) }", "header { transform: rotate(1deg) scaleZ(0) }",
                     '[DATA-EVIDENCE-ROLE="hero"] { display: none; }', "[Data-Evidence-Id] dd { opacity: 0 }",
                     "header { --hidden: 0; opacity: var(--hidden) }", "dl { display: var(--d, none) }", "header dd { visibility: VAR(--v) }",
                     "header { opacity: calc(1 - 1) }", "dl { font-size: min(0px, 1rem) }", "header { font-size: clamp(0, 1vw, 2rem) }",
                     "dl { font-size: clamp(var(--min), 1vw, 2rem) }", "header { transform: scale(var(--s)) }", "header { transform: var(--t) }",
                     "dl { content-visibility: env(--cv) }", "header dd { font-size: attr(data-size px) }",
                     "header { opacity: abs(0) }", "dl { font-size: round(0.4px, 1px) }", "header { font-size: clamp(-5px, 10vw, -1px) }",
                     "dl { font-size: clamp(0.0e1px, 1vw, 2rem) }", "header { font-size: clamp(1, 1vw, 2rem) }", "dl { font-size: clamp(1rem, calc(0px), 2rem) }",
                     "header { transform: translate(calc(0px)) }", "dl { transform: var(--t) }", "header { transform: scale(1) rotate(var(--r)) }",
                     "dl { transform: matrix(1, 0, 0, 0, 0, 0) }", "dl { -webkit-transform: scale(1, 0) }", "header { -moz-opacity: 0 }",
                     "header { -ms-transform: var(--t) }",
                     "header { -o-opacity: 0 }",
                     "header { scale: 0 }", "dl { scale: 1 0 }", "header { scale: 0% }", "dl { scale: var(--s) }", "header { SCALE: .0 }",
                     '.page-meta span:first-child::after { content: " · ci-gate failure"; }', "header::before { content: attr(data-evidence-state) }",
                     'header::after { content: "{ ci-gate failure"; }', "dl::after { content: '}'; }", "dl::after { content: \"\\\"{\"; }",
                     'header { /* " */ opacity: 0 }', 'header::after { content: "/*"; opacity: 0 }', "dl { --t: '{'; opacity: 0 }",
                     'footer { content: "unterminated }', "footer { color: red } /* unterminated",
                     "main > header dd::after { CONTENT : 'failure' }", "body::after { content: counter(x) }", "dl::before { content: url(x.svg) }"):
            with self.subTest(rule=rule):
                site = self.write_site(self.hero, page.replace("<dl>", '<dl class="hero-row">', 1))
                (site / "styles.css").write_text(f"footer p {{ display: none; }}\n{rule}\n")
                with self.assertRaises(SystemExit) as caught:
                    status.validate_projection(self.hero, self.repo, site)
                self.assertIn("stylesheet must not hide the evidence hero", str(caught.exception))
        for rule in ("footer p { display: none; }", ".pipeline-step::after { display: none; }", "#elsewhere { display: none; }",
                     'body::before { content: ""; }', "header::after { content: none }", "dl::before { content: normal; }",
                     ".page-meta { justify-content: center }", '.pipeline-step:not(:last-child)::after { content: "→" }',
                     "footer::after { content: 'x' }", "header dd { content: '' !important }",
                     "/* header dd { display: none; } */", "header dd { color: red; }", "[hidden] { display: none; }",
                     "body .other dd { display: none; }", ".site-nav a:not([aria-current]) { display: none; }",
                     "header { opacity: 0.9 }", "header dd { font-size: 0.76rem }", "header { transform: scale(0.5) }", "table { border-collapse: collapse }",
                     ".content-page .other { display: none; }", "main footer { display: none }", "header { opacity: .5 }",
                     "header dd { font-size: .76rem }", "header { transform: scale(.5) }",
                     "header { transform: scale(1.0, 1) }", "dl { transform: scale3d(10, 1.0, 1) }", "header { transform: scale(1e-0) }",
                     "header { font-size: clamp(2.5rem, 4.8vw, 4.8rem) }", "dl { color: var(--ink) }", "header { width: calc(100% - 1rem) }",
                     "header dd { font-size: clamp(.5rem, 1vw, 1rem) }", "header { font-size: clamp(2.5rem, 4.8vw, 4.8rem) !important }",
                     "dl { transform: translateY(-2px) }", "header { transform: scale(1) rotate(45deg) }", "dl { transform: none }", "header { scale: 1 }", 'footer::after { content: "{"; }', 'header::after { content: ""; }', "header { /* opacity: 0 */ color: red }", "dl { scale: 0.5 1 }", "header { scale: none }", "dl { --scale: 0 }",
                     "header { font-size: clamp(+1rem, 1vw, 2rem) }", "dl { font-size: clamp(00.5rem, 1vw, 2rem) }",
                     "dl { --webkit-transform: scale(0); transform: scale(1) }", "header { --hero-opacity: 0 }"):
            with self.subTest(rule=rule):
                site = self.write_site(self.hero, page)
                (site / "styles.css").write_text(rule + "\n")
                self.assertIsNone(status.validate_projection(self.hero, self.repo, site))

    def test_collapsed_summary_is_checked_exactly(self):
        page = render_page(self.hero)
        exact = "collapsed hero summary must read exactly"
        self.assertIn(status.expected_summary_line(self.hero), " ".join(page.replace("<strong>Evidence hero</strong>", "Evidence hero").split()))
        for name, old, new, expected in (
            ("contradicted conclusions", "ci-gate success · audit success", "ci-gate failure · audit failure", exact),
            ("one contradicted conclusion", "· audit success (PR", "· audit failure (PR", exact),
            ("another subject", f"snapshot of main {self.hero['repository']['commit'][:8]}", "snapshot of main deadbeef", exact),
            ("another pull request", "(PR #1005)", "(PR #1006)", exact),
            ("another capture time", "captured 2026-09-11T03:00:00Z</span>", "captured 2026-09-11T04:00:00Z</span>", exact),
            ("a link inside the summary", "<strong>Evidence hero</strong>", '<a href="https://example.invalid">Evidence hero</a>', exact),
            ("hidden summary", '<span data-evidence-id=', '<span hidden data-evidence-id=', "exactly one visible collapsed hero summary"),
            ("summary without the evidence id", '<span data-evidence-id=', "<span data-other=", "exactly one visible collapsed hero summary"),
            ("duplicated summary", "</span></div>", "</span><span data-evidence-id=\"x\">again</span></div>", "exactly one visible collapsed hero summary"),
        ):
            with self.subTest(mutation=name):
                self.assertIn(old, page)
                self.assert_page_rejected(page.replace(old, new, 1), expected)
        with self.subTest(mutation="summary moved outside the hero"):
            start, end = page.index('<div class="page-meta">'), page.index("</div>") + 6
            self.assert_page_rejected(page[:start] + page[end:] + page[start:end], "exactly one visible collapsed hero summary")

    def test_rows_outside_or_hidden_inside_the_hero_do_not_count(self):
        page = render_page(self.hero)
        gate = self.hero["snapshot"]["subjects"][1]
        with self.subTest(mutation="row moved outside the hero"):
            row = f'<dt>{gate["label"]}</dt>'
            start = page.index(row)
            end = page.index("</dd>", start) + 5
            moved = page[:start] + page[end:] + f"<dl>{page[start:end]}</dl>"
            self.assert_page_rejected(moved, f"proof row missing for {gate['label']}")
        with self.subTest(mutation="hidden row"):
            hidden = page.replace(f'<dt>{gate["label"]}</dt><dd>', f'<dt>{gate["label"]}</dt><dd hidden>', 1)
            # A visible label whose row is hidden is a dangling label: the pairing check fires first.
            self.assert_page_rejected(hidden, "must pair one visible label with one row each")
            inert = page.replace(f'<dt>{gate["label"]}</dt><dd>', f'<dt>{gate["label"]}</dt><dd inert>', 1)
            self.assert_page_rejected(inert, "must pair one visible label with one row each")
        for style in ("DISPLAY: NONE", "Visibility:Hidden", "color: red; DISPLAY:none", "opacity: 0", "opacity:0.0 !important",
                      "visibility: collapse", "content-visibility: hidden", "font-size: 0", "transform: scale(0)",
                      "opacity: .0", "font-size: .0px", "transform: scale(.00)",
                      "opacity: -0", "opacity: 0e0", "font-size: +0.0E-1px", "transform: scale(-.0e2)", "transform: scale(1, 0)"):
            with self.subTest(mutation=f"row hidden inline by {style}"):
                hidden = page.replace(f'<dt>{gate["label"]}</dt><dd>', f'<dt>{gate["label"]}</dt><dd style="{style}">', 1)
                self.assert_page_rejected(hidden, "must pair one visible label with one row each")
        with self.subTest(mutation="summary hidden inline"):
            self.assert_page_rejected(page.replace("<span data-evidence-id=", '<span style="DISPLAY: NONE" data-evidence-id=', 1),
                                      "exactly one visible collapsed hero summary")
        for name, extra in (("a contradicting labelled row", "<dt>Current gate result</dt><dd>ci-gate failure</dd>"),
                            ("a harmless labelled row", "<dt>Note</dt><dd>see above</dd>")):
            with self.subTest(mutation=name):
                self.assert_page_rejected(page.replace("</dl>", extra + "</dl>", 1),
                                          "proof rows must be exactly the three subject rows and the Tier-1 heading; unexpected: ")
        with self.subTest(mutation="Tier-1 heading row drifted"):
            self.assert_page_rejected(page.replace(status.TIER1_HEADING_ROW, "in the ci-gate run, all failure:", 1),
                                      f"proof row for {status.TIER1_HEADING} must read exactly")
        with self.subTest(mutation="closing paragraph contradicted beside the required literals"):
            self.assert_page_rejected(page.replace("(read-only gh api).", "(read-only gh api; ci-gate failure).", 1),
                                      "hero prose must be exactly the reviewed masthead")
        for name, extra in (("a dangling contradicting label", "<dt>Current gate result: ci-gate failure</dt>"),
                            ("a doubled label", f'<dt>Note</dt><dt>{status.TIER1_HEADING}</dt><dd>{status.TIER1_HEADING_ROW}</dd>')):
            with self.subTest(mutation=name):
                target = "</dl>" if name.startswith("a dangling") else f"<dt>{status.TIER1_HEADING}</dt><dd>{status.TIER1_HEADING_ROW}</dd>"
                self.assert_page_rejected(page.replace(target, extra + ("</dl>" if target == "</dl>" else ""), 1),
                                          "must pair one visible label with one row each")
        with self.subTest(mutation="row without a label"):
            self.assert_page_rejected(page.replace(f'<dt>{gate["label"]}</dt>', "", 1),
                                      "must pair one visible label with one row each")
        with self.subTest(mutation="label reused"):
            audit = self.hero["snapshot"]["subjects"][2]
            self.assert_page_rejected(page.replace(f'<dt>{audit["label"]}</dt>', f'<dt>{gate["label"]}</dt>', 1),
                                      "must pair one visible label with one row each")


class HeroProseTests(ProjectionCase):
    """Free text inside the hero is closed: only the reviewed blocks, each once."""

    def setUp(self):
        super().setUp()
        self.page = render_page(self.hero)
        self.projection = self.write_site
        self.reject = self.assert_page_rejected

    def test_unlabelled_prose_inside_the_hero_is_rejected(self):
        contradiction = "<p>Current gate result: ci-gate failure; audit failure.</p>"
        for name, old, new in (
            ("paragraph after the summary toggle", "</summary>", "</summary>" + contradiction),
            ("paragraph before the rows", '<div class="page-meta">', contradiction + '<div class="page-meta">'),
            ("span in the page meta", "<span><strong>Readiness", "<span>ci-gate failure</span><span><strong>Readiness"),
            ("heading", "<h1>", "<h2>ci-gate failure</h2><h1>"),
            ("loose text in a container", "<details>", "<details>ci-gate failure"),
            ("loose text in the hero itself", "<p class=\"eyebrow\">", "ci-gate failure<p class=\"eyebrow\">"),
            ("eyebrow extended", "Updated 2026-01-01</p>", "Updated 2026-01-01 · ci-gate failure</p>"),
            ("eyebrow date malformed", "Updated 2026-01-01</p>", "Updated 2026-1-1</p>"),
            ("eyebrow date off the JSON-LD dateModified", "Updated 2026-01-01</p>", "Updated 9999-99-99</p>"),
            ("masthead drifted", "Readiness</strong> pre-alpha", "Readiness</strong> alpha"),
            ("toggle drifted", status.HERO_DETAILS_TOGGLE, "Snapshot subjects (ci-gate failure)"),
            ("closing paragraph reordered", "Roadmap at that revision: ", "Roadmap: "),
        ):
            with self.subTest(mutation=name):
                self.assertIn(old, self.page)
                self.reject(self.page.replace(old, new, 1),
                            "hero prose must be exactly the reviewed masthead, the details toggle and the record's closing paragraph; unexpected: ")

    def test_details_toggle_and_closing_paragraph_are_mandatory(self):
        for name, old, new in (
            ("toggle hidden", "<summary>", "<summary hidden>"),
            ("toggle hidden inline", "<summary>", '<summary style="opacity: .0">'),
            ("toggle inert", "<summary>", "<summary inert>"),
            ("toggle and paragraph inert with the disclosure", "<details>", "<details inert>"),
            ("toggle and paragraph inside a closed dialog", "<details>", "<dialog><details>"),
            ("toggle hidden by a signed exponent zero", "<summary>", '<summary style="opacity: -0e0">'),
            ("toggle removed", f"<summary>{status.HERO_DETAILS_TOGGLE}</summary>", ""),
            ("closing paragraph moved outside the hero", "</details></header>", "</details></header><p>" + status.expected_closing_paragraph(self.hero) + "</p>"),
        ):
            with self.subTest(mutation=name):
                self.assertIn(old, self.page)
                mutated = self.page.replace(old, new, 1)
                if name.startswith("closing paragraph"):
                    mutated = mutated.replace("<p>" + status.expected_closing_paragraph(self.hero) + "</p></details>", "</details>", 1)
                site = self.projection(self.hero, mutated)
                with self.assertRaises(SystemExit) as caught:
                    status.validate_projection(self.hero, self.repo, site)
                self.assertIn("must render every reviewed masthead block, the details toggle and the record's closing paragraph "
                              "exactly once; missing: " + (status.HERO_DETAILS_TOGGLE if name.startswith("toggle") else "Roadmap"),
                              str(caught.exception))

    def test_repeated_attributes_are_rejected_and_open_dialogs_accepted(self):
        job = re.search(r'<a href="([^"]+)">job</a>', self.page).group(0)
        for name, old, new in (
            ("a repeated href on a job link", job, job.replace("<a ", '<a href="https://example.invalid/wrong" ', 1)),
            ("a repeated class on the hero", '<header data-evidence-role="hero"', '<header class="a" data-evidence-role="hero" class="b"'),
            ("a repeated id outside the hero", '<main id="main-content"', '<main id="main-content" class="x" id="other"'),
        ):
            with self.subTest(mutation=name):
                self.assertIn(old, self.page)
                self.reject(self.page.replace(old, new, 1), "repeats an attribute, which browsers and this checker would read differently: ")
        outside = self.page.replace("</header></main>", "</header><dialog open><p>note</p></dialog></main>", 1)
        status.validate_projection(self.hero, self.repo, self.projection(self.hero, outside))

    def test_unreviewed_containers_inside_the_hero_hide_what_they_wrap(self):
        """The hero's structure is an allowlist, so an unnamed container hides its subtree.

        Each wrapper below is a container a reader never sees through — a popover,
        a disabled fieldset, a second disclosure level, an element the reviewed set
        does not name at all. None of them is on a denylist; all of them are
        rejected because the hero's tags and attributes are enumerated instead.
        """
        for name, opening, closing in (
            ("a popover div", "<div popover>", "</div>"),
            ("a popover on a reviewed tag", '<div popover="auto">', "</div>"),
            ("a disabled fieldset", "<fieldset disabled>", "</fieldset>"),
            ("an unreviewed tag", "<figure>", "</figure>"),
            ("a custom element", "<pycc-panel>", "</pycc-panel>"),
            ("a second disclosure level", "<details><summary>more</summary>", "</details>"),
            ("an open second disclosure level", "<details open><summary>more</summary>", "</details>"),
        ):
            with self.subTest(wrapper=name):
                wrapped = self.page.replace("<dl>", opening + "<dl>", 1).replace("</dl>", "</dl>" + closing, 1)
                self.reject(wrapped, f"status visible proof row/limitation missing: {status.APP_ID}")
        with self.subTest(wrapper="an unreviewed attribute on the hero root"):
            self.reject(self.page.replace('<header data-evidence-role="hero"', '<header popover data-evidence-role="hero"', 1),
                        "status must render exactly one visible evidence hero")
        with self.subTest(wrapper="the one reviewed disclosure level"):
            # The rule is nesting, not <details> itself: the hero's own toggle stays visible.
            status.validate_projection(self.hero, self.repo, self.projection(self.hero, self.page))

    def test_an_unavailable_record_must_leave_no_proof_rows_on_the_page(self):
        """``build_record`` never emits an unavailable record, so this guards a hand-edited manifest."""
        hero = copy.deepcopy(self.hero)
        hero["state"] = "unavailable"
        site = self.projection(self.hero, self.page)
        with self.assertRaises(SystemExit) as caught:
            status.validate_unavailable_projection(hero, self.repo, site)
        self.assertIn("status record is unavailable but the page still renders proof rows: "
                      f"App {status.APP_ID}, {status.TIER1_HEADING}, /actions/runs/", str(caught.exception))
        stripped = self.page[:self.page.index("<details>")] + "</header></main></body></html>\n"
        site = self.projection(self.hero, stripped)
        self.assertIsNone(status.validate_unavailable_projection(hero, self.repo, site))

    def test_embedded_stylesheets_and_foreign_links_are_checked(self):
        for name, old, new, expected in (
            ("style in head", "</head>", "<style>.content-page { display: none }</style></head>", "stylesheet must not hide"),
            ("style in the hero", "<details>", "<details><style>dl { opacity: .0 }</style>", "stylesheet must not hide"),
            ("import", "</head>", "<style>@import url(other.css);</style></head>", "stylesheet must not hide the evidence hero or its proof rows or add text to them: @import"),
            ("foreign stylesheet link", "</head>", '<link rel="stylesheet" href="../other.css"></head>', "may link no stylesheet but site/styles.css: ../other.css"),
        ):
            with self.subTest(mutation=name):
                self.assertIn(old, self.page)
                self.reject(self.page.replace(old, new, 1), expected)
        for name, old, new in (
            ("harmless embedded style", "</head>", "<style>footer p { display: none }</style></head>"),
            ("the site stylesheet linked", "</head>", '<link rel="stylesheet" href="../styles.css?v=2"></head>'),
        ):
            with self.subTest(mutation=name):
                site = self.projection(self.hero, self.page.replace(old, new, 1))
                self.assertIsNone(status.validate_projection(self.hero, self.repo, site))

    def test_reviewed_prose_block_cannot_repeat(self):
        for old in ("<h1>What pycc can do <span>today.</span></h1>", '<p class="eyebrow">Evidence page · Updated 2026-01-01</p>'):
            with self.subTest(block=old):
                self.assertIn(old, self.page)
                self.reject(self.page.replace(old, old + old, 1), "hero prose block rendered twice")

    def test_every_reviewed_masthead_block_is_mandatory(self):
        for old in ('<p class="eyebrow">Evidence page · Updated 2026-01-01</p>', "<h1>What pycc can do <span>today.</span></h1>",
                    f'<p class="page-lede">{status.HERO_MASTHEAD[1]}</p>',
                    "<span><strong>Milestone</strong> v0.3 acceptance criteria met, released as v0.3.0; v0.4 in progress</span>",
                    "<span><strong>Acceptance</strong> v0.1, v0.2, and v0.3 all fully met</span>",
                    "<span><strong>Readiness</strong> pre-alpha</span>"):
            for name, new in (("removed", ""), ("hidden", old.replace(">", ' style="opacity: .0">', 1))):
                with self.subTest(block=old[:40], mutation=name):
                    self.assertIn(old, self.page)
                    self.reject(self.page.replace(old, new, 1), "must render every reviewed masthead block")
        script = '<script type="application/ld+json">{"dateModified": "2026-01-01"}</script>'
        for name, new in (("no dateModified", ""), ("two dateModified", script + script.replace("2026-01-01", "2026-02-02"))):
            with self.subTest(mutation=name):
                self.reject(self.page.replace(script, new, 1), "exactly one JSON-LD dateModified")

    def test_hidden_or_outside_prose_and_inline_markup_are_accepted(self):
        for name, old, new in (
            ("hidden paragraph", "</summary>", '</summary><p hidden>Current gate result: ci-gate failure.</p>'),
            ("inline-hidden span", "<span><strong>Readiness", '<span style="DISPLAY: NONE">ci-gate failure</span><span><strong>Readiness'),
            ("prose after the hero", "</header>", "</header><p>Current gate result: ci-gate failure; audit failure.</p>"),
            ("inline emphasis inside a block", "Readiness</strong> pre-alpha", "Readiness</strong> <em>pre</em>-alpha"),
            ("whitespace and comments", "Roadmap at that revision: ", "Roadmap at that\n   revision: <!-- ci-gate failure -->"),
            ("dateModified spacing", '{"dateModified": "2026-01-01"}', '{ "dateModified" :"2026-01-01" }'),
        ):
            with self.subTest(mutation=name):
                self.assertIn(old, self.page)
                site = self.projection(self.hero, self.page.replace(old, new, 1))
                self.assertIsNone(status.validate_projection(self.hero, self.repo, site))


class CurrencyTests(SyntheticRepository):
    def advance(self, merges):
        for _ in range(merges):
            index = len(list(self.repo.glob("later-*.txt")))
            (self.repo / f"later-{index}.txt").write_text(f"{index}\n")
            git(self.repo, "add", ".")
            git(self.repo, "commit", "-qm", f"later {index}")
        return git(self.repo, "rev-parse", "HEAD")

    def page_edit(self, base):
        git(self.repo, "checkout", "-qb", "feature", base)
        (self.repo / "site" / "status").mkdir(exist_ok=True)
        (self.repo / "site" / "status" / "index.html").write_text("<html></html>\n")
        git(self.repo, "add", ".")
        git(self.repo, "commit", "-qm", "edit status page")
        return git(self.repo, "rev-parse", "HEAD")

    def test_distance_twenty_is_accepted_and_twenty_one_is_rejected(self):
        base = self.advance(20)
        head = self.page_edit(base)
        self.assertTrue(status.currency_required(self.repo, base, head))
        self.assertEqual(status.check_currency(self.hero, self.repo, base, 20), 20)
        git(self.repo, "checkout", "-q", "main")
        base = self.advance(1)
        with self.assertRaises(SystemExit) as caught:
            status.check_currency(self.hero, self.repo, base, 20)
        self.assertIn("21 first-parent merges behind", str(caught.exception))

    def test_subject_outside_the_base_history_is_rejected(self):
        git(self.repo, "checkout", "-qb", "other", f"{self.commit}^")
        (self.repo / "other.txt").write_text("other\n")
        git(self.repo, "add", ".")
        git(self.repo, "commit", "-qm", "other")
        base = git(self.repo, "rev-parse", "HEAD")
        with self.assertRaises(SystemExit) as caught:
            status.check_currency(self.hero, self.repo, base, 20)
        self.assertIn("not on the first-parent history of the base", str(caught.exception))

    def test_subject_merged_into_the_base_through_a_second_parent_is_rejected(self):
        base = GitObjectTests.merge_subject_through_second_parent(self)
        with self.assertRaises(SystemExit) as caught:
            status.check_currency(self.hero, self.repo, base, 20)
        self.assertIn("not on the first-parent history of the base", str(caught.exception))

    def test_currency_is_required_only_for_status_page_or_record_edits(self):
        base = self.advance(1)
        git(self.repo, "checkout", "-qb", "docs-only", base)
        (self.repo / "docs" / "other.md").write_text("x\n")
        git(self.repo, "add", ".")
        git(self.repo, "commit", "-qm", "docs only")
        self.assertFalse(status.currency_required(self.repo, base, git(self.repo, "rev-parse", "HEAD")))
        manifest = self.repo / "site" / "evidence-heroes.json"
        manifest.write_text(json.dumps({"heroes": [self.hero]}) + "\n")
        git(self.repo, "add", ".")
        git(self.repo, "commit", "-qm", "status record")
        self.assertTrue(status.currency_required(self.repo, base, git(self.repo, "rev-parse", "HEAD")))
        manifest.write_text("not json\n")
        git(self.repo, "add", ".")
        git(self.repo, "commit", "-qm", "broken record")
        self.assertTrue(status.currency_required(self.repo, base, git(self.repo, "rev-parse", "HEAD")))
        with self.assertRaises(SystemExit):
            status.currency_required(self.repo, "0" * 40, base)

    def test_cli_currency_skips_untriggered_ranges_and_enforces_triggered_ones(self):
        base = self.advance(3)
        manifest = self.write_manifest(self.hero)
        git(self.repo, "checkout", "-qb", "quiet", base)
        (self.repo / "quiet.txt").write_text("q\n")
        git(self.repo, "add", ".")
        git(self.repo, "commit", "-qm", "quiet")
        quiet = git(self.repo, "rev-parse", "HEAD")
        argv = ["--currency", "--base", base, "--head", quiet, str(manifest), str(self.evidence), str(self.repo)]
        self.assertEqual(self.run_cli(argv), 0)
        git(self.repo, "checkout", "-q", "main")
        head = self.page_edit(base)
        argv = ["--currency", "--base", base, "--head", head, "--max-first-parent-distance", "3", str(manifest), str(self.evidence), str(self.repo)]
        self.assertEqual(self.run_cli(argv), 0)
        argv[argv.index("3")] = "2"
        with self.assertRaises(SystemExit) as caught:
            self.run_cli(argv)
        self.assertIn("3 first-parent merges behind", str(caught.exception))


class CommandLineTests(SyntheticRepository):
    def test_structural_and_verify_git_modes(self):
        manifest = self.write_manifest(self.hero)
        self.assertEqual(self.run_cli([str(manifest), str(self.evidence), str(self.repo)]), 0)
        self.assertEqual(self.run_cli(["--verify-git", str(manifest), str(self.evidence), str(self.repo)]), 0)
        hero = self.mutated(lambda hero: hero["repository"].__setitem__("tree", "f" * 40))
        hero["snapshot"]["subjects"][0]["tree"] = hero["snapshot"]["subjects"][0]["merged_pull_request"]["head_tree"] = "f" * 40
        manifest = self.write_manifest(hero)
        self.assertEqual(self.run_cli([str(manifest), str(self.evidence), str(self.repo)]), 0)
        with self.assertRaises(SystemExit) as caught:
            self.run_cli(["--verify-git", str(manifest), str(self.evidence), str(self.repo)])
        self.assertIn("tree differs", str(caught.exception))

    def test_unavailable_record_passes_every_mode(self):
        manifest = self.write_manifest({"page_id": "status", "state": "unavailable"})
        for flags in ([], ["--verify-git"], ["--currency", "--base", self.commit, "--head", self.commit]):
            self.assertEqual(self.run_cli([*flags, str(manifest), str(self.evidence), str(self.repo)]), 0)

    def test_missing_or_invalid_inventory_is_rejected(self):
        with self.assertRaises(SystemExit) as caught:
            self.run_cli([str(self.root / "absent.json"), str(self.evidence), str(self.repo)])
        self.assertIn("cannot read the hero inventory", str(caught.exception))
        manifest = self.write_manifest({"page_id": "landing", "state": "all-Tier-1"})
        with self.assertRaises(SystemExit) as caught:
            self.run_cli([str(manifest), str(self.evidence), str(self.repo)])
        self.assertIn("status hero is missing", str(caught.exception))

    def test_argument_validation(self):
        manifest = self.write_manifest(self.hero)
        with contextlib.redirect_stderr(io.StringIO()):
            with self.assertRaises(SystemExit):
                check_status_snapshot.parse_args(["--currency", str(manifest), str(self.evidence), str(self.repo)])
            with self.assertRaises(SystemExit):
                check_status_snapshot.parse_args(["--max-first-parent-distance", "-1", str(manifest), str(self.evidence), str(self.repo)])
        args = check_status_snapshot.parse_args([str(manifest), str(self.evidence), str(self.repo)])
        self.assertEqual(args.max_first_parent_distance, 20)


class SummaryTests(SyntheticRepository):
    def test_summary_is_one_line_and_names_the_subjects(self):
        text = status.summary(self.hero)
        self.assertNotIn("\n", text)
        for literal in ("status-snapshot-v1", "all-Tier-1", self.commit, "#1005", "a" * 40, "2026-09-11T03:00:00Z",
                        status.LIMITATIONS, "https://rotnov.github.io/pycc/status/"):
            self.assertIn(literal, text)


if __name__ == "__main__":
    unittest.main()
