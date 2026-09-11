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

    def test_subject_must_be_on_the_first_parent_history_of_head(self):
        def mutate(doc, site, root):
            commit = doc["heroes"][STATUS]["repository"]["commit"]
            self.replace_everywhere(site, commit, "a" * 40)
            page = site / "status/index.html"
            page.write_text(page.read_text().replace(f"snapshot of main <code>{commit[:8]}</code>", "snapshot of main <code>aaaaaaaa</code>"))
            doc.clear()
            doc.update(json.loads((site / "evidence-heroes.json").read_text()))
        self.run_case(mutate, "is not on the first-parent history of HEAD")

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

    def test_check_from_another_workflow_or_event_is_rejected(self):
        for index, field, value in ((2, "workflow_path", ".github/workflows/ci.yml"), (2, "event", "pull_request"),
                                    (1, "workflow_path", ".github/workflows/workflow-policy.yml"), (1, "event", "pull_request")):
            with self.subTest(index=index, field=field):
                self.run_case(lambda doc, site, root, i=index, f=field, v=value:
                              doc["heroes"][STATUS]["snapshot"]["subjects"][i].__setitem__(f, v),
                              "must be observed in run of .github/workflows/")

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
             "hero must render every reviewed masthead block, the details toggle and the record's closing paragraph exactly once"),
            ("not release readiness.", "release readiness.",
             "hero prose must be exactly the reviewed masthead, the details toggle and the record's closing paragraph; unexpected: Roadmap"),
            ("https://github.com/rotnov/pycc/actions/runs/34552229293/job/103117345163",
             "https://github.com/rotnov/pycc/actions/workflows/audit.yml",
             "proof row for Pre-merge policy audit must read exactly as that subject's own sha, check, conclusion, time and links"),
            ("App 15368 · success · completed 2026-09-11T02:10:46Z", "App 15368 · failure (recorded success) · completed 2026-09-11T02:10:46Z",
             "proof row for Post-merge CI gate must read exactly as that subject's own sha, check, conclusion, time and links"),
            ("aarch64-apple-darwin · success</a>", "aarch64-apple-darwin · failure (recorded success)</a>",
             "Tier-1 row for build-test-coverage must appear exactly once, reading exactly"),
            ("ci-gate success · audit success (PR #1005)", "ci-gate failure · audit failure (PR #1005)",
             "collapsed hero summary must read exactly as the record's state, subject, conclusions, pull request and capture time"),
            ("snapshot of main <code>4111208c</code>", "snapshot of main <code>deadbeef</code>",
             "collapsed hero summary must read exactly as the record's state, subject, conclusions, pull request and capture time"),
            ('<span\n            data-evidence-id="status-snapshot-v1"', '<span hidden\n            data-evidence-id="status-snapshot-v1"',
             "exactly one visible collapsed hero summary"),
            ('<span\n            data-evidence-id="status-snapshot-v1"', '<span style="DISPLAY: NONE"\n            data-evidence-id="status-snapshot-v1"',
             "exactly one visible collapsed hero summary"),
            ('<details class="hero-evidence-details">', '<details style="Visibility: Hidden" class="hero-evidence-details">',
             "hero must render every reviewed masthead block, the details toggle and the record's closing paragraph exactly once"),
            ("<dt>Pre-merge policy audit</dt>", '<dt style="DISPLAY:NONE">Pre-merge policy audit</dt>',
             "proof rows must pair one visible label with one row each"),
            ("<dt>Tier-1 jobs</dt>", "<dt>Current gate result</dt><dd>ci-gate failure</dd><dt>Tier-1 jobs</dt>",
             "proof rows must be exactly the three subject rows and the Tier-1 heading; unexpected: Current gate result"),
            ("<dd>in the ci-gate run, all success:</dd>", "<dd>in the ci-gate run, all failure:</dd>",
             "proof row for Tier-1 jobs must read exactly"),
            ('<details class="hero-evidence-details">', '<dialog><details class="hero-evidence-details">',
             "hero must render every reviewed masthead block, the details toggle and the record's closing paragraph exactly once"),
            ('<a href="https://github.com/rotnov/pycc/actions/runs/34552229293/job/103117345163">job</a>',
             '<a href="https://example.invalid/wrong" href="https://github.com/rotnov/pycc/actions/runs/34552229293/job/103117345163">job</a>',
             "status page <a> repeats an attribute, which browsers and this checker would read differently: href"),
            ('<details class="hero-evidence-details">', '<details inert class="hero-evidence-details">',
             "hero must render every reviewed masthead block, the details toggle and the record's closing paragraph exactly once"),
            ("<summary>Snapshot subjects, conclusions and immutable links</summary>", "<summary inert>Snapshot subjects, conclusions and immutable links</summary>",
             "hero must render every reviewed masthead block, the details toggle and the record's closing paragraph exactly once"),
            ('<details class="hero-evidence-details">', '<details style="opacity: -0e0" class="hero-evidence-details">',
             "hero must render every reviewed masthead block, the details toggle and the record's closing paragraph exactly once"),
            ('<details class="hero-evidence-details">', '<details style="--h: 0; opacity: var(--h)" class="hero-evidence-details">',
             "hero must render every reviewed masthead block, the details toggle and the record's closing paragraph exactly once"),
            ('<details class="hero-evidence-details">', '<details style="opacity: 0" class="hero-evidence-details">',
             "hero must render every reviewed masthead block, the details toggle and the record's closing paragraph exactly once"),
            ('<details class="hero-evidence-details">', '<details style="font-size: .0px" class="hero-evidence-details">',
             "hero must render every reviewed masthead block, the details toggle and the record's closing paragraph exactly once"),
            ("<summary>Snapshot subjects, conclusions and immutable links</summary>",
             "<summary>Snapshot subjects, conclusions and immutable links</summary><p>Current gate result: ci-gate failure; audit failure.</p>",
             "hero prose must be exactly the reviewed masthead, the details toggle and the record's closing paragraph; unexpected: Current gate result"),
            ("<span><strong>Readiness</strong> pre-alpha</span>", "<span><strong>Readiness</strong> pre-alpha</span><span>ci-gate failure</span>",
             "hero prose must be exactly the reviewed masthead, the details toggle and the record's closing paragraph; unexpected: ci-gate failure"),
            ("(read-only <code>gh api</code>).", "(read-only <code>gh api</code>; ci-gate failure).",
             "hero prose must be exactly the reviewed masthead, the details toggle and the record's closing paragraph; unexpected: Roadmap"),
            ("</dl>", "<dt>Current gate result: ci-gate failure</dt></dl>", "proof rows must pair one visible label with one row each"),
            ("<summary>Snapshot subjects, conclusions and immutable links</summary>", "<summary hidden>Snapshot subjects, conclusions and immutable links</summary>",
             "hero must render every reviewed masthead block, the details toggle and the record's closing paragraph exactly once"),
            ('<link rel="stylesheet" href="../styles.css">', '<link rel="stylesheet" href="../styles.css"><style>.page-hero { display: none }</style>',
             "stylesheet must not hide the evidence hero"),
            ('<link rel="stylesheet" href="../styles.css">', '<link rel="stylesheet" href="../styles.css"><link rel="stylesheet" href="../hide.css">',
             "may link no stylesheet but site/styles.css"),
            ("<p class=\"eyebrow\">Evidence page · Updated 2026-09-11</p>", "<p class=\"eyebrow\">Evidence page · Updated 9999-99-99</p>",
             "hero prose must be exactly the reviewed masthead"),
            ('"dateModified": "2026-09-11"', '"dateModified": "2026-09-10"', "hero prose must be exactly the reviewed masthead"),
            ("<h1>What pycc can do <span>today.</span></h1>", "", "must render every reviewed masthead block"),
            ("<span><strong>Readiness</strong> pre-alpha</span>", "", "must render every reviewed masthead block"),
            ('<html lang="en-US">', '<html lang="en">', "locale must be en-US"),
        ):
            with self.subTest(mutation=new):
                def mutate(doc, site, root, old=old, new=new):
                    page = site / "status/index.html"
                    source = page.read_text()
                    self.assertIn(old, source)
                    page.write_text(source.replace(old, new, 1))
                self.run_case(mutate, expected)

    def test_swapped_proof_rows_are_rejected(self):
        gate_run = "https://github.com/rotnov/pycc/actions/runs/34552872912"
        audit_run = "https://github.com/rotnov/pycc/actions/runs/34552229293"
        subject = "proof row for Post-merge CI gate must read exactly as that subject's own sha, check, conclusion, time and links"
        for name, first, second, expected in (
            ("run links", f'href="{gate_run}"', f'href="{audit_run}"', subject),
            ("job links", f'href="{gate_run}/job/103121414776"', f'href="{audit_run}/job/103117345163"', subject),
            ("subject shas", "on <code>4111208c0910c0a7588408a6ac7d2b2bad20592c</code>",
             "on <code>984eb6f27ca9f4809fa1277e12f8c83283bef1f6</code>", subject),
            ("completion times", "completed 2026-09-11T02:10:46Z", "completed 2026-09-11T01:50:30Z", subject),
            ("subject labels", "<dt>Post-merge CI gate</dt>", "<dt>Pre-merge policy audit</dt>", subject),
            ("platform job links", f'href="{gate_run}/job/103119276938"', f'href="{gate_run}/job/103119277030"',
             "Tier-1 row for build-test-coverage must appear exactly once, reading exactly as its own runner, target, conclusion and job link"),
        ):
            with self.subTest(swapped=name):
                def mutate(doc, site, root, first=first, second=second):
                    page = site / "status/index.html"
                    source = page.read_text()
                    self.assertIn(first, source)
                    self.assertIn(second, source)
                    page.write_text(source.replace(first, "\0", 1).replace(second, first, 1).replace("\0", second, 1))
                self.run_case(mutate, expected)

    def test_stylesheet_hiding_the_proof_is_rejected(self):
        for rule in (".hero-evidence-details { display: none; }", "@media (max-width: 980px) { .page-hero dd { display: none; } }",
                     "body details li { visibility: hidden; }", ".page-hero DD { DISPLAY: NONE; }", ".page-meta span { Visibility: Hidden }",
                     ".hero-evidence-details { opacity: 0; }", ".page-hero dl { font-size: 0 }", ".page-hero { transform: scale(0) }",
                     ".page-hero dd { visibility: collapse }", ".page-hero { content-visibility: hidden }",
                     ".content-page { display: none; }", "#main-content { opacity: .0 }", "body { font-size: .0px }",
                     "main > .page-hero { transform: scale(.0) }", ".page-hero { opacity: -0 }", "#main-content { opacity: 0e0 }",
                     ".page-hero { --hidden: 0; opacity: var(--hidden) }", "#main-content { font-size: clamp(0px, 1vw, 1rem) }",
                     ".page-hero { transform: scale(var(--s)) }", "#main-content { opacity: abs(0) }", ".page-hero { font-size: clamp(-5px, 10vw, -1px) }",
                     ".page-hero { -webkit-transform: scale(1, 0) }",
                     "#main-content { scale: 0 }",
                     '.page-hero::after { content: "{ ci-gate failure"; }', ".page-hero::after { content: '}'; }",
                     '.page-hero { /* " */ opacity: 0 }', '.page-hero::after { content: "/*"; opacity: 0 }',
                     ".page-hero { transform: scale(1, 0) }", '[DATA-EVIDENCE-ROLE="hero"] { display: none; }',
                     '.page-meta span:first-child::after { content: " · ci-gate failure"; }', ".page-hero::before { content: attr(data-evidence-state) }"):
            with self.subTest(rule=rule):
                def mutate(doc, site, root, rule=rule):
                    path = site / "styles.css"
                    path.write_text(path.read_text() + "\n" + rule + "\n")
                self.run_case(mutate, "stylesheet must not hide the evidence hero")

    def test_stylesheet_left_open_is_rejected(self):
        for rule in ('.other { content: "unterminated }', ".other { color: red } /* unterminated"):
            with self.subTest(rule=rule):
                def mutate(doc, site, root, rule=rule):
                    path = site / "styles.css"
                    path.write_text(path.read_text() + "\n" + rule + "\n")
                self.run_case(mutate, "styles.css must not leave a CSS comment or quoted string open")

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
