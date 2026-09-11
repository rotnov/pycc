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

import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[1]
ARCHITECTURE = 4

# Fields whose absence is caught by a check that names the rule rather than the
# field: the hero-order check sees a `None` page id, and the stable-links check
# compares the whole closed set at once.  Every one of `stable_links`' keys
# belongs here, including the three the rejection's own prose happens to name --
# an assertion on "commit" would otherwise pass against a message that says
# "commit" only because it lists all five keys, proving nothing about which one
# was deleted.
STABLE_LINKS = "stable_links must be exactly the immutable commit"
FIELD_REJECTIONS = {
    ("page_id",): "hero page inventory/order must be exactly",
    ("stable_links", "owner"): STABLE_LINKS,
    ("stable_links", "part"): STABLE_LINKS,
    ("stable_links", "commit"): STABLE_LINKS,
    ("stable_links", "tree"): STABLE_LINKS,
    ("stable_links", "fixture"): STABLE_LINKS,
}


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
            """Every deletable field, as a trail whose last element names it."""
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
                # The deleted key's own name, not the universal
                # "evidence-heroes.json:" prefix every rejection carries: the
                # subTest has to prove that this field's absence is what was
                # caught. Only dict keys are yielded, so the name is always the
                # trail's last element.
                self.run_case(mutate, FIELD_REJECTIONS.get(trail, trail[-1]))

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
            rewritten = (json.dumps(trace, indent=2) + "\n").encode()
            path.write_bytes(rewritten)
            # Re-pin the digest, or the generic trace sha256/bytes check fires
            # first and the compiler-revision branch is never reached.
            pinned = doc["heroes"][ARCHITECTURE]["snapshot"]["trace"]
            pinned["sha256"] = hashlib.sha256(rewritten).hexdigest()
            pinned["bytes"] = len(rewritten)
        self.run_case(mutate, "names a different compiler revision")

    def test_verification_command_cannot_drift(self):
        def mutate(doc, site, root):
            doc["heroes"][ARCHITECTURE]["command"]["argv"] = ["cargo", "test"]
        self.run_case(mutate, "architecture command drifted")

    # -- state and projection negative controls -----------------------------

    def test_state_cannot_be_overstated_in_the_record(self):
        def mutate(doc, site, root):
            doc["heroes"][ARCHITECTURE]["state"] = "all-Tier-1"
        self.run_case(mutate, "does not match the derived state")

    def test_state_cannot_be_understated_in_the_record(self):
        def mutate(doc, site, root):
            doc["heroes"][ARCHITECTURE]["state"] = "unavailable"
        self.run_case(mutate, "must keep fixture=null; decorative partial evidence is forbidden")

    def test_page_state_drift_is_rejected(self):
        for drifted in ("all-Tier-1", "unavailable"):
            with self.subTest(state=drifted):
                def mutate(doc, site, root, drifted=drifted):
                    page = site / "architecture/index.html"
                    source = page.read_text()
                    self.assertIn('data-evidence-state="partial"', source)
                    page.write_text(source.replace('data-evidence-state="partial"',
                                                   f'data-evidence-state="{drifted}"'))
                self.run_case(mutate, "must carry exactly one matching data-evidence-role")

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
                self.run_case(mutate, "must contain exactly one marker for hero 'architecture'")

    def test_a_stage_cannot_claim_evidence_it_does_not_have(self):
        """The LLVM IR stage has no artifact; presenting it as one must be rejected."""
        def mutate(doc, site, root):
            stages = doc["heroes"][ARCHITECTURE]["snapshot"]["stages"]
            llvm_ir = next(item for item in stages if item["id"] == "llvm-ir")
            self.assertEqual(llvm_ir["evidence"], "none")
            llvm_ir["evidence"] = "artifact"
        self.run_case(mutate, "may not claim evidence")

    def test_softened_limitations_are_rejected(self):
        def mutate(doc, site, root):
            doc["heroes"][ARCHITECTURE]["limitations"] = "Everything is covered."
        self.run_case(mutate, "limitations drifted from the reviewed text")

    def test_an_edited_stage_excerpt_is_rejected_by_its_row_text(self):
        """Adding a token to an excerpt changes the collapsed row text first."""
        def mutate(doc, site, root):
            page = site / "architecture/index.html"
            source = page.read_text()
            marker = "ModModule {"
            self.assertIn(marker, source)
            page.write_text(source.replace(marker, "ModModule { /* trimmed */", 1))
        self.run_case(mutate, "must read exactly as that stage's own path, identity, byte count")

    def test_a_reindented_stage_excerpt_is_rejected_by_its_raw_bytes(self):
        """The row comparison collapses whitespace; the excerpt comparison does
        not. Reindenting one line keeps every token, so it survives the row
        check and must be caught by the byte-exact excerpt check."""
        def mutate(doc, site, root):
            page = site / "architecture/index.html"
            source = page.read_text()
            line = "\n    node_index: NodeIndex(None),\n"
            self.assertIn(line, source)
            page.write_text(source.replace(line, "\n        node_index: NodeIndex(None),\n", 1))
        self.run_case(mutate, "exact leading bytes of their artifacts, in stage order")

    def test_page_locale_drift_is_rejected(self):
        """Both halves of the locale claim: the `lang` attribute and the meta tag.

        This record's page is the first architecture page the locale check ever
        sees -- the `unavailable` projection never reached it -- so without these
        cases the check would be satisfied by a page nobody ever mutated.
        """
        for old, new in (
            ('<html lang="en-US">', '<html lang="en">'),
            ('<meta property="og:locale" content="en_US">',
             '<meta property="og:locale" content="en_GB">'),
        ):
            with self.subTest(mutation=new):
                def mutate(doc, site, root, old=old, new=new):
                    page = site / "architecture/index.html"
                    source = page.read_text()
                    self.assertIn(old, source)
                    page.write_text(source.replace(old, new, 1))
                self.run_case(mutate, "architecture locale must be en-US / en_US")

    def test_json_ld_language_drift_is_rejected(self):
        """The JSON-LD language is a separate claim from the page's own locale."""
        def mutate(doc, site, root):
            page = site / "architecture/index.html"
            source = page.read_text()
            self.assertIn('"inLanguage": "en-US",', source)
            page.write_text(source.replace('"inLanguage": "en-US",', '"inLanguage": "en",', 1))
        self.run_case(mutate, "hero 'architecture' JSON-LD inLanguage must be en-US")

    def test_a_symlinked_artifact_is_rejected(self):
        """An artifact must be a real file: a symlink is refused even when its
        target holds the pinned bytes, so the digest cannot be satisfied by
        something the evidence root only points at."""
        def mutate(doc, site, root):
            victim = root / "tests/fixtures/architecture-trace/trace.json"
            twin = victim.with_suffix(".json.twin")
            twin.write_bytes(victim.read_bytes())
            victim.unlink()
            victim.symlink_to(twin.name)
        self.run_case(mutate, "architecture trace record file is missing or unsafe")

    def test_an_artifact_reached_through_an_escaping_directory_is_rejected(self):
        """The path is a real file and no link itself, but an ancestor directory
        is a symlink out of the evidence root -- the case the resolved-path
        containment clause exists for, which a leaf-only check would accept."""
        outside = Path(tempfile.mkdtemp(prefix="site-pipeline-outside-"))
        self.addCleanup(shutil.rmtree, outside, True)

        def mutate(doc, site, root):
            directory = root / "tests/fixtures/architecture-trace"
            escaped = outside / "architecture-trace"
            shutil.move(str(directory), str(escaped))
            directory.symlink_to(escaped)
        self.run_case(mutate, "architecture trace record file is missing or unsafe")

    def test_moving_branch_link_is_rejected(self):
        def mutate(doc, site, root):
            commit = doc["heroes"][ARCHITECTURE]["repository"]["commit"]
            doc["heroes"][ARCHITECTURE]["stable_links"]["fixture"] = (
                "https://github.com/rotnov/pycc/blob/main/tests/fixtures/quick_start.py")
            self.assertEqual(len(commit), 40)
        self.run_case(mutate, "stable_links must be exactly the immutable commit")


    # -- attestation and provenance negative controls ------------------------

    def test_attestation_collected_at_cannot_be_in_the_future(self):
        """A well-formed RFC 3339 instant that has not happened yet. The field
        stays present and syntactically valid, so `require_exact_fields` and
        `is_utc_instant` both accept it and the freshness branch is what
        rejects it."""
        def mutate(doc, site, root):
            doc["heroes"][ARCHITECTURE]["attestation"]["collected_at"] = "2099-01-02T03:04:05Z"
        self.run_case(
            mutate,
            "architecture attestation collected_at must not be later than the validation time")

    def test_environment_cannot_drift_from_the_reviewed_toolchain(self):
        """A wrong-but-well-formed toolchain version: the record still carries
        every environment field, so only the reviewed-tuple comparison can
        refuse it."""
        def mutate(doc, site, root):
            doc["heroes"][ARCHITECTURE]["environment"]["rust"] = "1.98.0"
        self.run_case(
            mutate,
            "architecture environment drifted from the reviewed toolchain and capture host")

    def test_recorded_tree_must_be_the_commit_s_real_tree(self):
        """A syntactically valid SHA that is not this commit's tree. `git
        rev-parse <commit>^{tree}` is the only thing that can tell: the tree is
        not part of `stable_links`, so no link check sees it. The trace record
        must be rewritten and re-pinned alongside, or the trace record's own
        `compiler_tree` comparison fires first and `verify_git` is never
        reached."""
        def mutate(doc, site, root):
            bogus = "b" * 40
            doc["heroes"][ARCHITECTURE]["repository"]["tree"] = bogus
            path = root / "tests/fixtures/architecture-trace/trace.json"
            trace = json.loads(path.read_text())
            trace["compiler_tree"] = bogus
            rewritten = (json.dumps(trace, indent=2) + "\n").encode()
            path.write_bytes(rewritten)
            pinned = doc["heroes"][ARCHITECTURE]["snapshot"]["trace"]
            pinned["sha256"] = hashlib.sha256(rewritten).hexdigest()
            pinned["bytes"] = len(rewritten)
        self.run_case(mutate, "tree differs from the recorded tree")

    # -- visibility negative controls ----------------------------------------

    def test_the_stylesheet_cannot_hide_the_architecture_hero(self):
        """A stage row must be removed, not hidden. The rule names the hero's
        own class, so the stylesheet surface of the visibility model is what
        rejects it -- the selector is echoed back, which is what distinguishes
        this case from the embedded-`<style>` one below."""
        def mutate(doc, site, root):
            path = site / "styles.css"
            path.write_text(path.read_text() + "\n.page-hero { display: none; }\n")
        self.run_case(
            mutate,
            "must not hide the evidence hero or its stage rows or add text to them: .page-hero")

    def test_an_embedded_style_block_cannot_hide_the_stage_rows(self):
        """The same claim on the page's own embedded surface, aimed at the
        stage-row definitions rather than the hero root, so the echoed selector
        proves this case reached the embedded-CSS collection and not the
        stylesheet one."""
        def mutate(doc, site, root):
            page = site / "architecture/index.html"
            source = page.read_text()
            self.assertIn("</head>", source)
            page.write_text(source.replace(
                "</head>",
                "<style>.hero-evidence-details dd { visibility: hidden; }</style>\n</head>",
                1))
        self.run_case(
            mutate,
            "must not hide the evidence hero or its stage rows or add text to them: "
            ".hero-evidence-details dd")

    def test_any_inline_style_inside_the_hero_is_rejected_structurally(self):
        """The third surface is an allowlist, not a declaration model: a
        `style` attribute anywhere in the hero is refused for being there at
        all. The benign declaration is the control -- it is rejected with the
        identical message, which is the proof that this case is about the
        attribute allowlist and says nothing about `display: none` in
        particular."""
        for declaration in ("display: none", "color: red"):
            with self.subTest(declaration=declaration):
                def mutate(doc, site, root, declaration=declaration):
                    page = site / "architecture/index.html"
                    source = page.read_text()
                    self.assertIn("<dt>05 MIR</dt>", source)
                    page.write_text(source.replace(
                        "<dt>05 MIR</dt>",
                        f'<dt style="{declaration}">05 MIR</dt>',
                        1))
                self.run_case(
                    mutate,
                    "architecture stage rows must pair one visible label with one row each")


if __name__ == "__main__":
    unittest.main()
