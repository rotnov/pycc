"""Public-CLI controls for the real status snapshot record (D-241).

Runs only through the full-history Pages shell harness (`test-check-site.sh`):
the subject-side Git checks need the default-branch history, which the depth-1
governance checkout does not have.  Synthetic and record-internal cases live in
`test_check_status_snapshot.py`.
"""

import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[1]
STATUS = 5


class StatusEvidenceTests(unittest.TestCase):
    def run_case(self, mutate=None, expected=None):
        with tempfile.TemporaryDirectory(prefix="site-status-test-") as directory:
            root = Path(directory)
            site = root / "site"
            shutil.copytree(ROOT / "site", site)
            document = json.loads((site / "evidence-heroes.json").read_text())
            paths = {"tests/fixtures/conformance-breadth-manifest.json"}
            for hero in document["heroes"]:
                if hero["fixture"] is None:
                    continue
                paths.update([hero["fixture"]["path"], hero["test"]["path"]])
                artifacts = hero["snapshot"].get("artifacts")
                if artifacts is None:
                    # The landing snapshot is one file; the status snapshot holds subjects.
                    artifacts = [hero["snapshot"]] if "path" in hero["snapshot"] else []
                paths.update(item["path"] for item in artifacts)
            for relative in paths:
                destination = root / relative
                destination.parent.mkdir(parents=True, exist_ok=True)
                shutil.copyfile(ROOT / relative, destination)
            if mutate:
                mutate(document, site, root)
            (site / "evidence-heroes.json").write_text(json.dumps(document, ensure_ascii=False) + "\n")
            env = dict(os.environ, SITE_DIR=str(site), EVIDENCE_ROOT_PATH=str(root))
            result = subprocess.run(["sh", str(ROOT / "scripts/check-site.sh")], cwd=ROOT,
                                    env=env, capture_output=True, text=True)
            if expected is None:
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            else:
                self.assertNotEqual(result.returncode, 0, "public CLI accepted mutation")
                self.assertIn(expected, result.stdout + result.stderr)

    @staticmethod
    def replace_everywhere(site, old, new):
        """Co-mutate every projection so only the Git-backed check can reject."""
        for relative in ("evidence-heroes.json", "status/index.html", "index.html.md", "llms.txt"):
            path = site / relative
            source = path.read_text()
            if old not in source:
                raise AssertionError(f"mutation target absent in {relative}: {old}")
            path.write_text(source.replace(old, new))

    def test_published_status_record(self):
        manifest = json.loads((ROOT / "site/evidence-heroes.json").read_text())
        self.assertEqual(manifest["schema_version"], "2.1.0")
        hero = manifest["heroes"][STATUS]
        self.assertEqual(hero["page_id"], "status")
        self.assertEqual(hero["state"], "all-Tier-1")
        self.assertEqual(hero["fixture"]["path"], "scripts/collect_status_snapshot.py")
        self.assertEqual(hero["test"]["path"], "scripts/test_check_status_snapshot.py")
        self.assertEqual([item["id"] for item in hero["snapshot"]["subjects"]],
                         ["published-revision", "post-merge-ci-gate", "pre-merge-audit"])
        self.assertEqual(len(hero["environment"]["platforms"]), 5)

    def test_healthy_public_cli(self):
        self.run_case()

    def test_every_nested_record_field_is_required(self):
        document = json.loads((ROOT / "site/evidence-heroes.json").read_text())

        def fields(value, trail=()):
            if isinstance(value, dict):
                for key, child in value.items():
                    yield trail + (key,)
                    yield from fields(child, trail + (key,))
            elif isinstance(value, list):
                for index, child in enumerate(value):
                    yield from fields(child, trail + (index,))
        for trail in fields(document["heroes"][STATUS]):
            with self.subTest(field=trail):
                def mutate(doc, site, root, trail=trail):
                    value = doc["heroes"][STATUS]
                    for key in trail[:-1]:
                        value = value[key]
                    del value[trail[-1]]
                self.run_case(mutate, "evidence")

    def test_stale_milestone_line_is_rejected(self):
        def mutate(doc, site, root):
            hero = doc["heroes"][STATUS]
            old = hero["attestation"]["milestone_line"]
            new = "Current milestone: v0.9 — everything is finished."
            hero["attestation"]["milestone_line"] = new
            page = site / "status/index.html"
            source = page.read_text()
            self.assertIn(old, source)
            page.write_text(source.replace(old, new))
        self.run_case(mutate, "milestone_line differs from docs/ROADMAP.md at")

    def test_subject_must_be_an_ancestor_of_head(self):
        def mutate(doc, site, root):
            commit = doc["heroes"][STATUS]["repository"]["commit"]
            self.replace_everywhere(site, commit, "a" * 40)
            doc.clear()
            doc.update(json.loads((site / "evidence-heroes.json").read_text()))
        self.run_case(mutate, "is not an ancestor of HEAD")

    def test_recorded_tree_must_match_the_subject_tree(self):
        def mutate(doc, site, root):
            tree = doc["heroes"][STATUS]["repository"]["tree"]
            for relative in ("evidence-heroes.json", "status/index.html"):
                path = site / relative
                source = path.read_text()
                self.assertIn(tree, source)
                path.write_text(source.replace(tree, "b" * 40))
            doc.clear()
            doc.update(json.loads((site / "evidence-heroes.json").read_text()))
        self.run_case(mutate, "tree differs from the recorded tree")

    def test_two_parent_subject_is_rejected(self):
        self.run_case(lambda doc, site, root: doc["heroes"][STATUS]["snapshot"]["subjects"][0].__setitem__("parent_count", 2),
                      "exactly one parent")

    def test_non_success_conclusion_cannot_stay_all_tier_1(self):
        for trail in ((("environment", "platforms", 4, "conclusion"), "failure"),
                      (("snapshot", "subjects", 1, "conclusion"), "skipped"),
                      (("snapshot", "subjects", 2, "conclusion"), None)):
            with self.subTest(field=trail[0]):
                def mutate(doc, site, root, trail=trail):
                    value = doc["heroes"][STATUS]
                    for key in trail[0][:-1]:
                        value = value[key]
                    value[trail[0][-1]] = trail[1]
                self.run_case(mutate, "must be success for a non-unavailable record")

    def test_hidden_or_missing_proof_rows_are_rejected(self):
        for old, new, expected in (
            ('<details class="hero-evidence-details">', '<details hidden class="hero-evidence-details">',
             "visible proof row/limitation missing"),
            ("not release readiness.", "release readiness.", "visible proof row/limitation missing"),
            ("https://github.com/rotnov/pycc/actions/runs/34552229293/job/103117345163",
             "https://github.com/rotnov/pycc/actions/workflows/audit.yml", "immutable commit/tree/pull/run/job links missing"),
            ('<html lang="en-US">', '<html lang="en">', "locale must be en-US"),
        ):
            with self.subTest(mutation=new):
                def mutate(doc, site, root, old=old, new=new):
                    page = site / "status/index.html"
                    source = page.read_text()
                    self.assertIn(old, source)
                    page.write_text(source.replace(old, new, 1))
                self.run_case(mutate, expected)

    def test_central_summaries_cannot_drift(self):
        for relative in ("index.html.md", "llms.txt"):
            for old, new in (("five Tier-1 jobs success", "five Tier-1 jobs green"),
                             ("later merges are not covered until the snapshot is refreshed", "later merges are covered")):
                with self.subTest(surface=relative, mutation=new):
                    def mutate(doc, site, root, relative=relative, old=old, new=new):
                        path = site / relative
                        source = path.read_text()
                        self.assertIn(old, source)
                        path.write_text(source.replace(old, new, 1))
                    self.run_case(mutate, "snapshot summary/limitations drifted")


if __name__ == "__main__":
    unittest.main()
