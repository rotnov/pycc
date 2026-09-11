"""Record-internal and wiring controls for the architecture pipeline trace (D-243).

`unittest discover -s scripts -p 'test_*.py'` picks these up, so every case here
must stay fast and offline: no website checker run, no compiler run.  The slow
public-CLI controls live in `scripts/site_pipeline_evidence_test.py`, which
`scripts/test-check-site.sh` invokes explicitly.

Two kinds of case live here.  The first exercises
`scripts/site_pipeline_evidence.py`'s pure functions directly, each with an
independent negative case, so a defect in the derivation is caught without the
whole site harness.  The second proves the module is actually *wired in* --
registered in the Pages workflow's path filters, invoked by the shell harness,
and dispatched by `check_site_evidence.py` -- because a contract module nothing
calls is indistinguishable from one that passes.
"""

import json
from pathlib import Path
import sys
import unittest


ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "scripts"))

import site_pipeline_evidence as pipeline  # noqa: E402


def hero():
    document = json.loads((ROOT / "site/evidence-heroes.json").read_text())
    return next(item for item in document["heroes"] if item["page_id"] == "architecture")


class DerivedStateTests(unittest.TestCase):
    def test_shipped_record_derives_partial(self):
        self.assertEqual(pipeline.derive_state(hero()), "partial")
        self.assertEqual(hero()["state"], "partial")

    def test_every_stage_evidenced_would_derive_all_tier_1(self):
        record = hero()
        for stage in record["snapshot"]["stages"]:
            stage["evidence"] = "artifact"
        self.assertEqual(pipeline.derive_state(record), "all-Tier-1")

    def test_a_broken_stage_list_derives_unavailable(self):
        for broken in (None, [], "stages", [{"evidence": "artifact"}]):
            with self.subTest(stages=broken):
                record = hero()
                record["snapshot"]["stages"] = broken
                self.assertEqual(pipeline.derive_state(record), "unavailable")

    def test_an_unknown_evidence_kind_derives_unavailable(self):
        record = hero()
        record["snapshot"]["stages"][0]["evidence"] = "vibes"
        self.assertEqual(pipeline.derive_state(record), "unavailable")

    def test_missing_snapshot_derives_unavailable(self):
        record = hero()
        record["snapshot"] = None
        self.assertEqual(pipeline.derive_state(record), "unavailable")

    def test_llvm_ir_is_the_only_unevidenced_stage(self):
        unevidenced = [stage["id"] for stage in hero()["snapshot"]["stages"]
                       if stage["evidence"] == "none"]
        self.assertEqual(unevidenced, ["llvm-ir"])


class ShapeTests(unittest.TestCase):
    def test_shipped_record_matches_every_closed_field_set(self):
        record = hero()
        shape = pipeline.expected_shape()
        for field in ("fixture", "test", "command", "snapshot", "repository",
                      "attestation", "environment"):
            pipeline.require_exact_fields(record[field], shape[field], field)
        for stage in record["snapshot"]["stages"]:
            pipeline.require_exact_fields(stage, shape["stage"], "stage")

    def test_an_extra_field_is_rejected(self):
        with self.assertRaises(SystemExit):
            pipeline.require_exact_fields({"a": 1, "b": 2}, {"a"}, "context")

    def test_a_missing_field_is_rejected(self):
        with self.assertRaises(SystemExit):
            pipeline.require_exact_fields({"a": 1}, {"a", "b"}, "context")

    def test_a_non_object_is_rejected(self):
        with self.assertRaises(SystemExit):
            pipeline.require_exact_fields(["a"], {"a"}, "context")


class IdentityTests(unittest.TestCase):
    def test_every_pinned_artifact_identity_matches_the_working_tree(self):
        record = hero()
        pinned = [(record["fixture"]["path"], record["fixture"]["sha256"]),
                  (record["test"]["path"], record["test"]["sha256"]),
                  (record["snapshot"]["trace"]["path"], record["snapshot"]["trace"]["sha256"])]
        pinned.extend((stage["path"], stage["sha256"]) for stage in record["snapshot"]["stages"]
                      if stage["path"] and stage["sha256"])
        # fixture + test + trace, plus the five stages that carry an artifact
        # (source, parser, hir, type-check and mir; type-check re-pins the HIR
        # bytes, which is exactly the claim that type checking rewrites nothing).
        self.assertEqual(len(pinned), 3 + 5)
        for path, digest in pinned:
            with self.subTest(path=path):
                data = pipeline.read_artifact(ROOT, path, "test")
                self.assertEqual(pipeline.canonical_sha256(data), digest)

    def test_reading_a_missing_artifact_fails(self):
        with self.assertRaises(SystemExit):
            pipeline.read_artifact(ROOT, "tests/fixtures/architecture-trace/absent.txt", "test")

    def test_reading_outside_the_evidence_root_fails(self):
        with self.assertRaises(SystemExit):
            pipeline.read_artifact(ROOT, "../etc/hosts", "test")

    def test_crlf_is_canonicalized_before_hashing(self):
        self.assertEqual(pipeline.canonical_sha256(b"a\r\nb"), pipeline.canonical_sha256(b"a\nb"))
        self.assertEqual(pipeline.canonical_bytes(b"a\r\nb"), 3)

    def test_excerpts_are_exact_artifact_prefixes(self):
        for stage in hero()["snapshot"]["stages"]:
            if stage["id"] not in pipeline.EXCERPTED:
                continue
            with self.subTest(stage=stage["id"]):
                excerpt = pipeline.excerpt_text(ROOT, stage["path"])
                body = pipeline.read_artifact(ROOT, stage["path"], "test").decode()
                self.assertTrue(body.startswith(excerpt.rstrip("\n")))
                self.assertEqual(excerpt.count("\n"), pipeline.EXCERPT_LINES)

    def test_an_artifact_shorter_than_the_excerpt_fails(self):
        with self.assertRaises(SystemExit):
            pipeline.excerpt_text(ROOT, pipeline.FIXTURE)


class LinkAndSummaryTests(unittest.TestCase):
    def test_stable_links_are_the_derived_immutable_links(self):
        record = hero()
        self.assertEqual(record["stable_links"], pipeline.expected_links(record))

    def test_no_stable_link_points_at_a_moving_ref(self):
        for name, url in hero()["stable_links"].items():
            with self.subTest(link=name):
                self.assertNotIn("/blob/main/", url)
                self.assertNotIn("/tree/main", url)

    def test_the_summary_is_published_verbatim_on_both_text_surfaces(self):
        line = pipeline.summary(hero())
        for surface in ("site/index.html.md", "site/llms.txt"):
            with self.subTest(surface=surface):
                self.assertEqual((ROOT / surface).read_text().count(line), 1)

    def test_the_summary_states_the_llvm_gap(self):
        self.assertIn("LLVM IR unavailable", pipeline.summary(hero()))

    def test_collected_at_is_a_real_utc_instant(self):
        self.assertTrue(pipeline.is_utc_instant(hero()["attestation"]["collected_at"]))
        for bad in ("2026-09-11", "2026-09-11 17:34:06Z", "2026-13-11T00:00:00Z", 17, None):
            with self.subTest(value=bad):
                self.assertFalse(pipeline.is_utc_instant(bad))

    def test_utc_now_is_in_the_records_own_shape(self):
        self.assertTrue(pipeline.is_utc_instant(pipeline.utc_now()))


class UnavailableProjectionTests(unittest.TestCase):
    def test_an_unavailable_record_may_not_leave_stage_rows_on_the_page(self):
        with self.assertRaises(SystemExit):
            pipeline.validate_unavailable_projection(hero(), ROOT, ROOT / "site")

    def test_a_page_without_stage_rows_passes_the_unavailable_projection(self):
        record = hero()
        record["page_path"] = "site/python-aot-compilers/index.html"
        pipeline.validate_unavailable_projection(record, ROOT, ROOT / "site")


class WiringTests(unittest.TestCase):
    def test_the_pages_workflow_filters_on_every_new_path(self):
        workflow = (ROOT / ".github/workflows/pages.yml").read_text()
        for path in ("scripts/site_pipeline_evidence.py",
                     "scripts/site_pipeline_evidence_test.py",
                     "scripts/test_site_pipeline_wiring.py",
                     "tests/architecture_trace.rs",
                     "tests/architecture_manifest.rs",
                     "tests/fixtures/architecture-trace/**"):
            with self.subTest(path=path):
                # Both the push and the pull_request filter must carry it.
                self.assertEqual(workflow.count(f'- "{path}"'), 2)

    def test_the_shell_harness_invokes_the_public_cli_suite(self):
        harness = (ROOT / "scripts/test-check-site.sh").read_text()
        self.assertIn('python3 -B "$repo_root/scripts/site_pipeline_evidence_test.py"', harness)

    def test_the_architecture_hero_left_the_generic_unavailable_loop(self):
        harness = (ROOT / "scripts/test-check-site.sh").read_text()
        self.assertIn("for index in (3, 6, 7):", harness)
        self.assertNotIn("for index in (3, 4, 6, 7):", harness)

    def test_the_site_checker_dispatches_the_pipeline_module(self):
        checker = (ROOT / "scripts/check_site_evidence.py").read_text()
        self.assertIn("import site_pipeline_evidence", checker)
        self.assertIn("site_pipeline_evidence.validate(", checker)
        self.assertIn("site_pipeline_evidence.validate_projection(", checker)

    def test_the_testing_inventory_lists_both_suites(self):
        testing = (ROOT / "docs/TESTING.md").read_text()
        for name in ("scripts/site_pipeline_evidence.py",
                     "scripts/site_pipeline_evidence_test.py",
                     "scripts/test_site_pipeline_wiring.py",
                     "tests/architecture_trace.rs",
                     "tests/architecture_manifest.rs"):
            with self.subTest(name=name):
                self.assertIn(name, testing)


if __name__ == "__main__":
    unittest.main()
