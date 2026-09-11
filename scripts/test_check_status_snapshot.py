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
        for index, key in [(0, "merged_pull_request"), (0, "tree"), (0, "parent_count"), (1, "run_url"), (2, "job_url")]:
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
    return ('<html lang="en-US"><head><meta property="og:locale" content="en_US"></head><body>'
            f'<header data-evidence-role="hero" data-evidence-id="{hero["evidence_id"]}"><div class="page-meta">'
            f'<span data-evidence-id="{hero["evidence_id"]}"><strong>Evidence hero</strong> {hero["state"]} · snapshot of main '
            f'{hero["repository"]["commit"][:8]} · ci-gate {subjects["post-merge-ci-gate"]["conclusion"]} · '
            f'audit {subjects["pre-merge-audit"]["conclusion"]} (PR #{merged["number"]}) · '
            f'captured {hero["attestation"]["collected_at"]}</span></div>'
            f'<dl>{"".join(rows)}<dt>Tier-1 jobs</dt><dd>in the ci-gate run:</dd></dl><ul>{"".join(items)}</ul>'
            f'<p>{hero["attestation"]["milestone_line"]} {hero["limitations"]}</p></header></body></html>\n')


class ProjectionTests(SyntheticRepository):
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
                     "HEADER DD { DISPLAY: NONE; }", "dl dt { Visibility : Hidden }", ".page-meta span { display:NONE }"):
            with self.subTest(rule=rule):
                site = self.write_site(self.hero, page.replace("<dl>", '<dl class="hero-row">', 1))
                (site / "styles.css").write_text(f"footer p {{ display: none; }}\n{rule}\n")
                with self.assertRaises(SystemExit) as caught:
                    status.validate_projection(self.hero, self.repo, site)
                self.assertIn("stylesheet must not hide the evidence hero", str(caught.exception))
        for rule in ("footer p { display: none; }", ".pipeline-step::after { display: none; }", "#elsewhere { display: none; }",
                     "/* header dd { display: none; } */", "header dd { color: red; }", "[hidden] { display: none; }",
                     "body .other dd { display: none; }", ".site-nav a:not([aria-current]) { display: none; }"):
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
            self.assert_page_rejected(hidden, f"proof row missing for {gate['label']}")
        with self.subTest(mutation="row without a label"):
            self.assert_page_rejected(page.replace(f'<dt>{gate["label"]}</dt>', "", 1),
                                      "must pair one visible label with one row each")
        with self.subTest(mutation="label reused"):
            audit = self.hero["snapshot"]["subjects"][2]
            self.assert_page_rejected(page.replace(f'<dt>{audit["label"]}</dt>', f'<dt>{gate["label"]}</dt>', 1),
                                      "must pair one visible label with one row each")


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
