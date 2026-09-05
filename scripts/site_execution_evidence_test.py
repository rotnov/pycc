"""Public-CLI controls; run through the full-history Pages shell harness."""

import copy
import hashlib
import html
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[1]


class ExecutionEvidenceTests(unittest.TestCase):
    def run_case(self, mutate=None, expected=None):
        with tempfile.TemporaryDirectory(prefix="site-execution-test-") as directory:
            root = Path(directory)
            site = root / "site"
            shutil.copytree(ROOT / "site", site)
            document = json.loads((site / "evidence-heroes.json").read_text())
            paths = {"tests/fixtures/conformance-breadth-manifest.json"}
            for hero in document["heroes"]:
                if hero["fixture"] is None:
                    continue
                paths.update([hero["fixture"]["path"], hero["test"]["path"]])
                paths.update(item["path"] for item in hero["snapshot"].get("artifacts", [hero["snapshot"]]))
            for relative in paths:
                destination = root / relative
                destination.parent.mkdir(parents=True, exist_ok=True)
                shutil.copyfile(ROOT / relative, destination)
            if mutate:
                mutate(document, site, root)
            (site / "evidence-heroes.json").write_text(json.dumps(document) + "\n")
            env = dict(os.environ, SITE_DIR=str(site), EVIDENCE_ROOT_PATH=str(root))
            result = subprocess.run(["sh", str(ROOT / "scripts/check-site.sh")], cwd=ROOT,
                                    env=env, capture_output=True, text=True)
            if expected is None:
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            else:
                self.assertNotEqual(result.returncode, 0, "public CLI accepted mutation")
                self.assertIn(expected, result.stdout + result.stderr)

    @staticmethod
    def edit(site, relative, old, new):
        path = site / relative
        source = path.read_text()
        if old not in source:
            raise AssertionError(f"mutation target absent: {old}")
        path.write_text(source.replace(old, new, 1))

    def test_published_execution_inventory(self):
        manifest = json.loads((ROOT / "site/evidence-heroes.json").read_text())
        self.assertEqual(manifest["schema_version"], "2.0.0")
        for hero in manifest["heroes"][1:3]:
            self.assertEqual(hero["state"], "all-Tier-1")
            self.assertTrue((ROOT / hero["page_path"]).is_file())
            self.assertEqual(len(hero["command"]["executions"]), 2)

    def test_healthy_public_cli(self):
        self.run_case()

    def test_primary_navigation_cannot_be_satisfied_by_footer_links(self):
        for relative in ("index.html", "status/index.html", "architecture/index.html",
                         "python-aot-compilers/index.html", "ai-native/index.html",
                         "language-support/index.html", "diagnostics/index.html"):
            for route in ("language-support", "diagnostics"):
                with self.subTest(page=relative, route=route):
                    def mutate(doc, site, root):
                        path = site / relative
                        source = path.read_text()
                        start = source.index('<nav class="site-nav"')
                        end = source.index("</nav>", start)
                        navigation = source[start:end]
                        prefix = "" if relative == "index.html" else "../"
                        href = f'href="{prefix}{route}/"'
                        self.assertIn(href, navigation)
                        path.write_text(source[:start] + navigation.replace(href, 'href="#omitted"', 1) + source[end:])
                    self.run_case(mutate, "primary navigation")

    def test_mobile_navigation_and_provenance_must_remain_usable(self):
        for old, new in [("/* Evidence identifiers wrap without hiding source text. */\n  overflow-wrap: anywhere;", "overflow-wrap: normal;"),
                         ("/* Keep every evidence route available without JavaScript. */\n    flex-wrap: wrap;", "flex-wrap: nowrap;")]:
            with self.subTest(rule=old):
                self.run_case(lambda doc, site, root: self.edit(site, "styles.css", old, new), "mobile evidence")

    def test_crlf_artifact_checkout_is_accepted(self):
        def mutate(doc, site, root):
            for hero in doc["heroes"][1:3]:
                for item in [hero["fixture"], hero["test"], *hero["snapshot"]["artifacts"]]:
                    path = root / item["path"]
                    path.write_bytes(path.read_bytes().replace(b"\r\n", b"\n").replace(b"\n", b"\r\n"))
        self.run_case(mutate)

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
        for index in (1, 2):
            for trail in fields(document["heroes"][index]):
                with self.subTest(hero=index, field=trail):
                    def mutate(doc, site, root, trail=trail):
                        value = doc["heroes"][index]
                        for key in trail[:-1]:
                            value = value[key]
                        del value[trail[-1]]
                    self.run_case(mutate, "evidence")

    def test_source_test_and_snapshot_must_match_preserved_git_blobs(self):
        for index in (1, 2):
            artifacts = [("fixture", None), ("test", None), ("snapshot", 0)]
            if index == 2:
                artifacts.append(("snapshot", 1))
            for key, slot in artifacts:
                for action in ("remove", "change", "co-mutate"):
                    with self.subTest(hero=index, artifact=key, action=action):
                        def mutate(doc, site, root):
                            hero = doc["heroes"][index]
                            artifact = hero[key]["artifacts"][slot] if key == "snapshot" else hero[key]
                            path = root / artifact["path"]
                            if action == "remove":
                                path.unlink()
                            else:
                                path.write_bytes(path.read_bytes() + b"\n")
                                if action == "co-mutate":
                                    artifact["sha256"] = hashlib.sha256(path.read_bytes()).hexdigest()
                                    if "text" in artifact:
                                        artifact["text"] = path.read_text()
                        self.run_case(mutate, "execution evidence")

    def test_well_shaped_but_wrong_provenance_or_execution_is_rejected(self):
        changes = [
            (("repository", "commit"), "a" * 40),
            (("repository", "tree"), "a" * 40),
            (("attestation", "tested_tree"), "b" * 40),
            (("attestation", "tested_commit"), "b" * 40),
            (("attestation", "run_id"), 33969157528),
            (("environment", "python"), "3.14.6"),
            (("environment", "platforms", 0, "job_url"), "https://github.com/rotnov/pycc/actions/runs/33969157527/job/101314505712"),
            (("command", "cwd"), "site"),
            (("command", "executions", 0, "argv"), ["pycc", "build", "invented.py"]),
            (("command", "executions", 0, "profile"), "release"),
            (("command", "executions", 0, "flags"), ["--release"]),
            (("command", "executions", 0, "exit_status"), False),
            (("command", "executions", 0, "stderr"), {"artifact": "stdout"}),
            (("command", "executions", 0, "stdout"), {"empty": True}),
            (("test", "names"), ["invented_test"]),
        ]
        for index in (1, 2):
            for trail, wrong in changes:
                with self.subTest(hero=index, field=trail):
                    def mutate(doc, site, root):
                        value = doc["heroes"][index]
                        for key in trail[:-1]:
                            value = value[key]
                        value[trail[-1]] = wrong
                    self.run_case(mutate, "execution evidence")

    def test_visible_source_results_and_exact_newlines_are_bound(self):
        for slug in ("language-support", "diagnostics"):
            for original, wrong in [('data-execution="source"', 'hidden data-execution="source"'),
                                    ('<code data-execution="source">', '<code data-execution="source">\n'),
                                    ('data-execution="source"', 'data-execution="another-source"'),
                                    ('data-evidence-role="hero"', 'hidden data-evidence-role="hero"')]:
                with self.subTest(slug=slug, original=original):
                    self.run_case(lambda doc, site, root: self.edit(site, f"{slug}/index.html", original, wrong), "visible H1" if original == 'data-evidence-role="hero"' else "visible ordered")

    def test_every_visible_execution_unit_rejects_byte_and_visibility_drift(self):
        for slug, executions in (("language-support", ("pycc", "cpython")),
                                 ("diagnostics", ("human", "json"))):
            for execution in executions:
                for field in ("command", "stdout", "stderr", "exit"):
                    marker = f'<code data-execution="{execution}-{field}">'
                    for replacement in (marker + "\n", marker + "wrong", marker.replace("<code ", "<code hidden ")):
                        with self.subTest(slug=slug, execution=execution, field=field, mutation=replacement):
                            self.run_case(lambda doc, site, root: self.edit(site, f"{slug}/index.html", marker, replacement), "visible ordered")

    def test_diagnostic_json_semantics_cannot_be_co_mutated_with_local_hashes(self):
        for field, wrong in (("code", "T0001"), ("message", "invented message"),
                             ("help", ["invented repair"]), ("line", 2), ("col", 9), ("len", 5)):
            with self.subTest(field=field):
                def mutate(doc, site, root):
                    artifact = doc["heroes"][2]["snapshot"]["artifacts"][1]
                    original = artifact["text"]
                    value = json.loads(original)
                    target = value["spans"][0] if field in ("line", "col", "len") else value
                    target[field] = wrong
                    changed = json.dumps(value, separators=(",", ":")) + "\n"
                    (root / artifact["path"]).write_text(changed)
                    artifact.update(text=changed, sha256=hashlib.sha256(changed.encode()).hexdigest())
                    self.edit(site, "diagnostics/index.html", html.escape(original.rstrip("\n")), html.escape(changed.rstrip("\n")))
                self.run_case(mutate, "diagnostics snapshot differs from the reviewed execution record")

    def test_snapshot_mixing_is_rejected(self):
        for index in (1, 2):
            def mutate(doc, site, root):
                doc["heroes"][index]["snapshot"]["artifacts"][0] = copy.deepcopy(doc["heroes"][3-index]["snapshot"]["artifacts"][0])
            self.run_case(mutate, "snapshot differs from the reviewed execution record")
        self.run_case(lambda doc, site, root: doc["heroes"][2]["command"]["executions"][0]["stdout"].update(artifact="json"), "command differs from the reviewed execution record")

    def test_fake_human_help_and_unsupported_repair_are_rejected(self):
        self.run_case(lambda doc, site, root: self.edit(site, "diagnostics/index.html", '<code data-execution="human-stdout">',
                      '<code data-execution="human-stdout">help: convert the argument\n'), "visible ordered")
        self.run_case(lambda doc, site, root: self.edit(site, "diagnostics/index.html", "</article>",
                      '<p>Repair with <code>int("three")</code>.</p></article>'), "unsupported claim")

    def test_h1_is_unique_and_visible(self):
        for slug in ("language-support", "diagnostics"):
            for old, new in [("<h1>", "<h1 hidden>"), ("<h1>", "<h2>"), ("</h1>", "</h1><h1>Extra heading</h1>")]:
                with self.subTest(slug=slug, mutation=new):
                    self.run_case(lambda doc, site, root: self.edit(site, f"{slug}/index.html", old, new), "visible H1")

    def test_missing_or_duplicate_hero_is_rejected(self):
        for slug in ("language-support", "diagnostics"):
            for old, new in [('data-evidence-role="hero"', 'data-evidence-role="example"'),
                             ("</article>", '<section data-evidence-role="hero"></section></article>')]:
                with self.subTest(slug=slug, mutation=new):
                    self.run_case(lambda doc, site, root: self.edit(site, f"{slug}/index.html", old, new), "evidence")

    def test_added_unsupported_compatibility_claim_is_rejected(self):
        for slug in ("language-support", "diagnostics"):
            for claim in ("pycc fully supports Python 3.14.", "pycc supports all of PEP 526."):
                with self.subTest(slug=slug, claim=claim):
                    def mutate(doc, site, root):
                        self.edit(site, f"{slug}/index.html", "</article>", f"<p>{claim}</p></article>")
                        # Co-update ordinary, social and JSON-LD descriptions as
                        # well as both central maps; agreement is not authority.
                        page = site / slug / "index.html"
                        text = page.read_text()
                        import re
                        description = re.search(r'<meta name="description" content="([^"]+)"', text)[1]
                        page.write_text(text.replace(description, description + " " + claim))
                        for relative in ("index.html.md", "llms.txt"):
                            path = site / relative
                            path.write_text(path.read_text() + "\n" + claim + "\n")
                    self.run_case(mutate, "unsupported claim")

    def test_each_projection_preserves_state_and_limitations(self):
        for index in (1, 2):
            for surface in ("html", "markdown", "llm", "structured_data", "social", "social_twitter"):
                with self.subTest(hero=index, surface=surface):
                    def mutate(doc, site, root):
                        hero = doc["heroes"][index]
                        path = hero["projections"]["social" if surface == "social_twitter" else surface].removeprefix("site/")
                        if surface in ("markdown", "llm"):
                            self.edit(site, path, hero["limitations"], "All behavior is guaranteed.")
                        elif surface == "html":
                            self.edit(site, path, '<p class="evidence-limitations">', '<p hidden class="evidence-limitations">')
                        elif surface == "structured_data":
                            self.edit(site, path, '"value": "all-Tier-1"', '"value": "experimental"')
                        else:
                            import re
                            file = site / path
                            text = file.read_text()
                            identity = 'property="og:description"' if surface == "social" else 'name="twitter:description"'
                            text, count = re.subn(r'(<meta ' + identity + r'[^>]*data-evidence-state=")all-Tier-1', r'\1experimental', text)
                            self.assertEqual(count, 1)
                            file.write_text(text)
                    self.run_case(mutate, "evidence")

    def test_each_new_route_requires_metadata_and_discovery(self):
        for slug in ("language-support", "diagnostics"):
            for key in ("description", "robots", "og:type", "og:site_name", "og:locale",
                        "og:url", "og:title", "og:description", "og:image", "og:image:alt",
                        "twitter:card", "twitter:title", "twitter:description", "twitter:image", "twitter:image:alt"):
                with self.subTest(slug=slug, key=key):
                    def mutate(doc, site, root):
                        import re
                        path = site / slug / "index.html"
                        text = path.read_text()
                        text, count = re.subn(r'<meta (?:name|property)="' + re.escape(key) + r'"[^>]*>', '', text)
                        self.assertEqual(count, 1)
                        path.write_text(text)
                    self.run_case(mutate, "evidence" if key in ("og:description", "twitter:description", "og:locale") else slug)
            self.run_case(lambda doc, site, root: (site / slug / "index.html").unlink(), "" )
            self.run_case(lambda doc, site, root: self.edit(site, "sitemap.xml",
                          f"<loc>https://rotnov.github.io/pycc/{slug}/</loc>", "<loc>https://rotnov.github.io/pycc/missing/</loc>"), "Sitemap")

    def test_current_breadth_scope_cannot_drift(self):
        def mutate(doc, site, root):
            path = root / "tests/fixtures/conformance-breadth-manifest.json"
            data = json.loads(path.read_text())
            row = next(row for row in data["rows"] if row["fixtures"] == ["pep_0526_var_annotations.py"])
            row["not_proven"][0]["kind"] = "out-of-scope"
            path.write_text(json.dumps(data))
        self.run_case(mutate, "current breadth scope drifted")

    def test_republished_surfaces_describe_v0_4_in_progress(self):
        for relative in ("index.html", "status/index.html", "index.html.md", "llms.txt"):
            with self.subTest(surface=relative):
                text = " ".join((ROOT / "site" / relative).read_text().split())
                self.assertIn("v0.4 is in progress.", text)
                self.assertIn("Cross-file project from imports have landed.", text)
                self.assertIn("incremental compilation remain incomplete.", text)

    def test_current_milestone_scope_mutations(self):
        for relative in ("index.html", "status/index.html", "index.html.md", "llms.txt"):
            for old, new in [("v0.4 is in progress.", "v0.4 has not started."),
                             ("Cross-file project from imports have landed.", "All Python imports are supported."),
                             ("incremental compilation remain incomplete.", "incremental compilation are complete.")]:
                with self.subTest(surface=relative, claim=old):
                    self.run_case(lambda doc, site, root: self.edit(site, relative, old, new), "v0.4")


if __name__ == "__main__":
    unittest.main()
