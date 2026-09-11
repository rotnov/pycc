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
        return {
            "id": identity, "label": label, "sha": sha, "check": name, "app_id": status.APP_ID,
            "conclusion": "success", "completed_at": completed, "run_id": run,
            "run_url": f"{status.REPO}/actions/runs/{run}", "job_url": f"{status.REPO}/actions/runs/{run}/job/{job}",
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
             "run_url": None, "job_url": None, "tree": tree, "parent_count": 1,
             "merged_pull_request": {"number": 1005, "head_sha": head_sha, "head_tree": tree,
                                     "url": f"{status.REPO}/pull/1005"}},
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
        for index, key in [(0, "merged_pull_request"), (0, "tree"), (0, "parent_count"), (1, "run_url"), (2, "job_url")]:
            self.assert_rejected(lambda hero, i=index, k=key: hero["snapshot"]["subjects"][i].pop(k), "fields drifted")
        self.assert_rejected(lambda hero: hero["snapshot"]["subjects"][0]["merged_pull_request"].pop("head_tree"), "fields drifted")
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
        for value in ("2026-09-11T03:00:00+00:00", "2026-09-11 03:00:00Z", "2026-09-11", None, 3):
            self.assert_rejected(lambda hero, v=value: hero["attestation"].__setitem__("collected_at", v), "RFC 3339 UTC")
            self.assert_rejected(lambda hero, v=value: hero["snapshot"]["subjects"][1].__setitem__("completed_at", v), "RFC 3339 UTC")

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
        self.assertIn("not an ancestor of HEAD", str(caught.exception))

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
        self.assertIn("not an ancestor of the base", str(caught.exception))

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
