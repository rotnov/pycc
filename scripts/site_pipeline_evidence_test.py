"""Public-CLI controls for the real architecture pipeline-trace record (D-243).

Every case here drives the shipped public entrypoint, `scripts/check-site.sh`,
against a full copy of `site/` plus an evidence root holding the pinned
artifacts, so a case proves what a reviewer actually gets rather than what the
module would say if called directly.  Record-internal and pure-function cases
live in `test_site_pipeline_wiring.py`, which `unittest discover` picks up.

This file is invoked explicitly from `scripts/test-check-site.sh` beside its
Part 1 siblings; it is deliberately not named `test_*.py`, because a case here
runs the whole website checker and is far too slow for the discovery suite.

Independent negative proof cases are the point (the `.harden` incident
`independent-negative-proof-cases-missing` records this exact gap from #565):
for every claim the record makes, there is a mutation that must be rejected,
and `test_healthy_public_cli` is the positive control that keeps those
rejections honest.
"""

import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[1]
ARCHITECTURE = 4


def artifact_paths(document):
    """Every checked-in path the manifest's records read from the evidence root."""
    paths = {"tests/fixtures/conformance-breadth-manifest.json"}
    for hero in document["heroes"]:
        if hero["fixture"] is None:
            continue
        paths.update([hero["fixture"]["path"], hero["test"]["path"]])
        snapshot = hero["snapshot"]
        artifacts = snapshot.get("artifacts")
        if artifacts is None:
            artifacts = [snapshot] if "path" in snapshot else []
        paths.update(item["path"] for item in artifacts)
        trace = snapshot.get("trace")
        if trace is not None:
            paths.add(trace["path"])
        for stage in snapshot.get("stages", []):
            if stage.get("path"):
                paths.add(stage["path"])
    return paths


class PipelineEvidenceTests(unittest.TestCase):
    def run_case(self, mutate=None, expected=None):
        with tempfile.TemporaryDirectory(prefix="site-pipeline-test-") as directory:
            root = Path(directory)
            site = root / "site"
            shutil.copytree(ROOT / "site", site)
            document = json.loads((site / "evidence-heroes.json").read_text())
            for relative in artifact_paths(document):
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

    # -- positive control ---------------------------------------------------

    def test_healthy_public_cli(self):
        self.run_case()

    def test_published_architecture_record(self):
        manifest = json.loads((ROOT / "site/evidence-heroes.json").read_text())
        self.assertEqual(manifest["schema_version"], "2.2.0")
        hero = manifest["heroes"][ARCHITECTURE]
        self.assertEqual(hero["page_id"], "architecture")
        self.assertEqual(hero["kind"], "compiler-pipeline-trace")
        self.assertEqual(hero["state"], "partial")
        self.assertEqual(hero["fixture"]["path"], "tests/fixtures/quick_start.py")
        self.assertEqual(hero["test"]["path"], "tests/architecture_trace.rs")
        self.assertEqual([item["id"] for item in hero["snapshot"]["stages"]],
                         ["source", "parser", "hir", "type-check", "mir", "llvm-ir", "native", "stdout"])
        self.assertEqual([item["id"] for item in hero["snapshot"]["stages"]
                          if item["evidence"] == "none"], ["llvm-ir"])

    # -- record-shape negative controls -------------------------------------

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
        for trail in fields(document["heroes"][ARCHITECTURE]):
            with self.subTest(field=trail):
                def mutate(doc, site, root, trail=trail):
                    value = doc["heroes"][ARCHITECTURE]
                    for key in trail[:-1]:
                        value = value[key]
                    del value[trail[-1]]
                self.run_case(mutate, "evidence")

    # -- artifact-identity negative controls --------------------------------

    def test_corrupted_stage_artifact_is_rejected(self):
        def mutate(doc, site, root):
            path = root / "tests/fixtures/architecture-trace/parser-ast.txt"
            path.write_bytes(path.read_bytes() + b"injected\n")
        self.run_case(mutate, "architecture stage parser")

    def test_missing_stage_artifact_is_rejected(self):
        def mutate(doc, site, root):
            (root / "tests/fixtures/architecture-trace/hir-module.txt").unlink()
        self.run_case(mutate, "missing or unsafe")

    def test_missing_trace_record_is_rejected(self):
        def mutate(doc, site, root):
            (root / "tests/fixtures/architecture-trace/trace.json").unlink()
        self.run_case(mutate, "missing or unsafe")

    def test_rewritten_verification_test_is_rejected(self):
        def mutate(doc, site, root):
            path = root / "tests/architecture_trace.rs"
            path.write_text(path.read_text() + "\n// drift\n")
        self.run_case(mutate, "architecture test sha256 differs")

    def test_test_names_must_match_the_registered_tests(self):
        def mutate(doc, site, root):
            doc["heroes"][ARCHITECTURE]["test"]["names"].append("never_registered")
        self.run_case(mutate, "architecture test names")

    def test_trace_record_must_pin_the_same_compiler_revision(self):
        def mutate(doc, site, root):
            path = root / "tests/fixtures/architecture-trace/trace.json"
            trace = json.loads(path.read_text())
            trace["compiler_commit"] = "0" * 40
            path.write_text(json.dumps(trace, indent=2) + "\n")
        self.run_case(mutate, "architecture snapshot trace sha256/bytes differ")

    def test_verification_command_cannot_drift(self):
        def mutate(doc, site, root):
            doc["heroes"][ARCHITECTURE]["command"]["argv"] = ["cargo", "test"]
        self.run_case(mutate, "architecture command drifted")

    # -- state and projection negative controls -----------------------------

    def test_state_cannot_be_overstated_in_the_record(self):
        def mutate(doc, site, root):
            doc["heroes"][ARCHITECTURE]["state"] = "all-Tier-1"
        self.run_case(mutate, "architecture")

    def test_state_cannot_be_understated_in_the_record(self):
        def mutate(doc, site, root):
            doc["heroes"][ARCHITECTURE]["state"] = "unavailable"
        self.run_case(mutate, "architecture")

    def test_page_state_drift_is_rejected(self):
        for drifted in ("all-Tier-1", "unavailable"):
            with self.subTest(state=drifted):
                def mutate(doc, site, root, drifted=drifted):
                    page = site / "architecture/index.html"
                    source = page.read_text()
                    self.assertIn('data-evidence-state="partial"', source)
                    page.write_text(source.replace('data-evidence-state="partial"',
                                                   f'data-evidence-state="{drifted}"'))
                self.run_case(mutate, "architecture")

    def test_markdown_and_llm_markers_must_carry_the_record_state(self):
        marker = ("<!-- evidence-hero: architecture | architecture-trace-v1 | "
                  "compiler-pipeline-trace | partial | /architecture/ -->")
        for surface in ("index.html.md", "llms.txt"):
            with self.subTest(surface=surface):
                def mutate(doc, site, root, surface=surface):
                    path = site / surface
                    source = path.read_text()
                    self.assertIn(marker, source)
                    path.write_text(source.replace(marker, marker.replace("| partial |", "| all-Tier-1 |")))
                self.run_case(mutate, "architecture")

    def test_softened_limitations_are_rejected(self):
        def mutate(doc, site, root):
            doc["heroes"][ARCHITECTURE]["limitations"] = "Everything is covered."
        self.run_case(mutate, "architecture")

    def test_page_cannot_paraphrase_a_stage_excerpt(self):
        def mutate(doc, site, root):
            page = site / "architecture/index.html"
            source = page.read_text()
            marker = "ModModule {"
            self.assertIn(marker, source)
            page.write_text(source.replace(marker, "ModModule { /* trimmed */", 1))
        self.run_case(mutate, "architecture")

    def test_moving_branch_link_is_rejected(self):
        def mutate(doc, site, root):
            commit = doc["heroes"][ARCHITECTURE]["repository"]["commit"]
            doc["heroes"][ARCHITECTURE]["stable_links"]["fixture"] = (
                "https://github.com/rotnov/pycc/blob/main/tests/fixtures/quick_start.py")
            self.assertEqual(len(commit), 40)
        self.run_case(mutate, "architecture")


if __name__ == "__main__":
    unittest.main()
