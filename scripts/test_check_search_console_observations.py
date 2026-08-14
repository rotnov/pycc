#!/usr/bin/env python3
"""Mutation tests for the Google Search Console observation validator.

These tests exercise the production-path validator through both direct
``validate()`` calls and the real CLI entrypoint.  The key mutation test
``test_corrupted_ledger_status_with_unchanged_roadmap_fails`` corrupts the
ledger status in ``SEARCH_VISIBILITY.md`` while leaving ``ROADMAP.md``
unchanged — exactly the contradictory-indexing-status scenario from issue #163
— and verifies the audit rejects it.
"""

from __future__ import annotations

import importlib.util
import json
import subprocess
import sys
import tempfile
import unittest
from datetime import datetime, timezone
from pathlib import Path

VALIDATOR_PATH = Path(__file__).with_name("check_search_console_observations.py")
VALIDATOR_SPEC = importlib.util.spec_from_file_location(
    "check_search_console_observations", VALIDATOR_PATH
)
if VALIDATOR_SPEC is None or VALIDATOR_SPEC.loader is None:
    raise RuntimeError("could not load the search console observation validator")
VALIDATOR_MODULE = importlib.util.module_from_spec(VALIDATOR_SPEC)
VALIDATOR_SPEC.loader.exec_module(VALIDATOR_MODULE)

ObservationError = VALIDATOR_MODULE.ObservationError
validate = VALIDATOR_MODULE.validate
validate_artifact = VALIDATOR_MODULE.validate_artifact
validate_bindings = VALIDATOR_MODULE.validate_bindings
visibility_binding_phrases = VALIDATOR_MODULE.visibility_binding_phrases
roadmap_binding_phrases = VALIDATOR_MODULE.roadmap_binding_phrases
search_console_history_timestamps = VALIDATOR_MODULE.search_console_history_timestamps
current_interpretation_section = VALIDATOR_MODULE.current_interpretation_section
normalize_whitespace = VALIDATOR_MODULE.normalize_whitespace


def _load_repository_files(repository_root: Path) -> dict[str, str]:
    """Load the real repository docs into a dict for test setup."""
    docs = repository_root / "docs"
    return {
        "SEARCH_CONSOLE_OBSERVATIONS.json": (docs / "SEARCH_CONSOLE_OBSERVATIONS.json").read_text(),
        "SEARCH_VISIBILITY.md": (docs / "SEARCH_VISIBILITY.md").read_text(),
        "ROADMAP.md": (docs / "ROADMAP.md").read_text(),
    }


class SearchConsoleObservationTests(unittest.TestCase):
    """Unit tests that call validate() directly against temp trees."""

    def setUp(self) -> None:
        self.repository_root = Path(__file__).resolve().parent.parent
        self.files = _load_repository_files(self.repository_root)
        self.temporary = tempfile.TemporaryDirectory(prefix="pycc-search-console-")
        self.root = Path(self.temporary.name)
        docs = self.root / "docs"
        docs.mkdir(parents=True)
        for name, content in self.files.items():
            (docs / name).write_text(content)

    def tearDown(self) -> None:
        self.temporary.cleanup()

    # --- Positive tests ---

    def test_clean_tree_passes(self) -> None:
        validate(self.root)

    def test_artifact_validates_against_clean_tree(self) -> None:
        artifact = json.loads(self.files["SEARCH_CONSOLE_OBSERVATIONS.json"])
        validate_artifact(artifact)

    # --- Artifact structure mutation tests ---

    def test_artifact_version_must_be_1(self) -> None:
        artifact = json.loads(self.files["SEARCH_CONSOLE_OBSERVATIONS.json"])
        artifact["artifact_version"] = 2
        with self.assertRaisesRegex(ObservationError, "artifact_version must be 1"):
            validate_artifact(artifact)

    def test_artifact_rejects_missing_top_level_field(self) -> None:
        artifact = json.loads(self.files["SEARCH_CONSOLE_OBSERVATIONS.json"])
        del artifact["provenance"]
        with self.assertRaisesRegex(ObservationError, "unexpected top-level fields"):
            validate_artifact(artifact)

    def test_artifact_rejects_extra_top_level_field(self) -> None:
        artifact = json.loads(self.files["SEARCH_CONSOLE_OBSERVATIONS.json"])
        artifact["extra"] = True
        with self.assertRaisesRegex(ObservationError, "unexpected top-level fields"):
            validate_artifact(artifact)

    def test_provenance_must_declare_sanitized(self) -> None:
        artifact = json.loads(self.files["SEARCH_CONSOLE_OBSERVATIONS.json"])
        artifact["provenance"]["sanitized"] = False
        with self.assertRaisesRegex(ObservationError, "sanitized as true"):
            validate_artifact(artifact)

    def test_provenance_collected_at_must_be_valid_timestamp(self) -> None:
        artifact = json.loads(self.files["SEARCH_CONSOLE_OBSERVATIONS.json"])
        artifact["provenance"]["collected_at"] = "not-a-timestamp"
        with self.assertRaisesRegex(ObservationError, "collected_at"):
            validate_artifact(artifact)

    def test_observations_must_be_nonempty(self) -> None:
        artifact = json.loads(self.files["SEARCH_CONSOLE_OBSERVATIONS.json"])
        artifact["observations"] = []
        with self.assertRaisesRegex(ObservationError, "nonempty list"):
            validate_artifact(artifact)

    def test_observation_timestamps_must_be_nondecreasing(self) -> None:
        artifact = json.loads(self.files["SEARCH_CONSOLE_OBSERVATIONS.json"])
        artifact["observations"][0]["observed_at"] = "2026-07-30T00:00:00Z"
        with self.assertRaisesRegex(ObservationError, "nondecreasing"):
            validate_artifact(artifact)

    def test_latest_projection_must_match_latest_observation(self) -> None:
        artifact = json.loads(self.files["SEARCH_CONSOLE_OBSERVATIONS.json"])
        artifact["latest_projection"]["canonical_urls_total"] = 4
        with self.assertRaisesRegex(ObservationError, "canonical_urls_total disagrees"):
            validate_artifact(artifact)

    def test_latest_projection_rejects_extra_field(self) -> None:
        artifact = json.loads(self.files["SEARCH_CONSOLE_OBSERVATIONS.json"])
        artifact["latest_projection"]["extra"] = True
        with self.assertRaisesRegex(ObservationError, "latest_projection has unexpected"):
            validate_artifact(artifact)

    def test_url_inspection_status_must_be_valid(self) -> None:
        artifact = json.loads(self.files["SEARCH_CONSOLE_OBSERVATIONS.json"])
        artifact["observations"][-1]["url_inspection"]["status"] = "unknown"
        with self.assertRaisesRegex(ObservationError, "url_inspection status is invalid"):
            validate_artifact(artifact)

    def test_sitemap_search_console_status_must_be_valid(self) -> None:
        artifact = json.loads(self.files["SEARCH_CONSOLE_OBSERVATIONS.json"])
        artifact["observations"][-1]["sitemap"]["search_console_status"] = "unknown"
        with self.assertRaisesRegex(ObservationError, "sitemap search_console_status is invalid"):
            validate_artifact(artifact)

    def test_performance_status_must_be_valid(self) -> None:
        artifact = json.loads(self.files["SEARCH_CONSOLE_OBSERVATIONS.json"])
        artifact["observations"][-1]["performance"]["status"] = "unknown"
        with self.assertRaisesRegex(ObservationError, "performance status is invalid"):
            validate_artifact(artifact)

    # --- History table binding tests ---

    def test_history_timestamps_must_match_artifact(self) -> None:
        path = self.root / "docs" / "SEARCH_VISIBILITY.md"
        text = path.read_text()
        # Remove the last history row by replacing its timestamp
        text = text.replace(
            "| 2026-07-29T10:25:27Z |",
            "| 2026-07-29T10:25:28Z |",
            1,
        )
        path.write_text(text)
        with self.assertRaisesRegex(ObservationError, "timestamps do not match"):
            validate(self.root)

    # --- Prose binding mutation tests ---

    def test_corrupted_ledger_status_with_unchanged_roadmap_fails(self) -> None:
        """Corrupt the URL inspection status in the ledger while leaving
        the roadmap unchanged — the audit must reject this mutation.

        This is the core production-path negative test from issue #163:
        a contradictory indexing status must not survive the check.
        """
        path = self.root / "docs" / "SEARCH_VISIBILITY.md"
        text = path.read_text()
        # Corrupt the Current interpretation: change "five canonical URLs"
        # to "four canonical URLs" so the bound phrase no longer matches.
        # The roadmap is left unchanged.
        corrupted = text.replace(
            "All five canonical URLs now have positive URL Inspection evidence",
            "All four canonical URLs now have positive URL Inspection evidence",
            1,
        )
        self.assertNotEqual(text, corrupted, "corruption should change the text")
        path.write_text(corrupted)
        with self.assertRaisesRegex(
            ObservationError,
            "missing bound phrase.*URL inspection canonical count",
        ):
            validate(self.root)

    def test_corrupted_ledger_sitemap_status_with_unchanged_roadmap_fails(self) -> None:
        """Corrupt the sitemap status in the ledger while leaving the
        roadmap unchanged."""
        path = self.root / "docs" / "SEARCH_VISIBILITY.md"
        text = path.read_text()
        corrupted = text.replace(
            "Sitemap processing remains unsuccessful",
            "Sitemap processing is now successful",
            1,
        )
        self.assertNotEqual(text, corrupted)
        path.write_text(corrupted)
        with self.assertRaisesRegex(
            ObservationError,
            "missing bound phrase.*sitemap status",
        ):
            validate(self.root)

    def test_corrupted_ledger_performance_with_unchanged_roadmap_fails(self) -> None:
        """Corrupt the performance numbers in the ledger while leaving the
        roadmap unchanged."""
        path = self.root / "docs" / "SEARCH_VISIBILITY.md"
        text = path.read_text()
        corrupted = text.replace(
            "15 impressions and 2 clicks",
            "10 impressions and 1 click",
            1,
        )
        self.assertNotEqual(text, corrupted)
        path.write_text(corrupted)
        with self.assertRaisesRegex(
            ObservationError,
            "missing bound phrase.*impressions",
        ):
            validate(self.root)

    def test_corrupted_roadmap_with_unchanged_ledger_fails(self) -> None:
        """Corrupt the roadmap projection while leaving the ledger unchanged."""
        path = self.root / "docs" / "ROADMAP.md"
        text = path.read_text()
        corrupted = text.replace(
            "All five canonical website URLs have positive Google URL Inspection evidence",
            "All four canonical website URLs have positive Google URL Inspection evidence",
            1,
        )
        self.assertNotEqual(text, corrupted)
        path.write_text(corrupted)
        with self.assertRaisesRegex(
            ObservationError,
            "missing bound phrase.*URL inspection canonical count",
        ):
            validate(self.root)

    def test_corrupted_roadmap_performance_with_unchanged_ledger_fails(self) -> None:
        """Corrupt the performance numbers in the roadmap while leaving the
        ledger unchanged."""
        path = self.root / "docs" / "ROADMAP.md"
        text = path.read_text()
        corrupted = text.replace(
            "15 impressions, 2 clicks",
            "10 impressions, 1 click",
            1,
        )
        self.assertNotEqual(text, corrupted)
        path.write_text(corrupted)
        with self.assertRaisesRegex(
            ObservationError,
            "missing bound phrase.*impressions",
        ):
            validate(self.root)

    def test_corrupted_artifact_latest_projection_fails(self) -> None:
        """Corrupt the latest_projection in the artifact — both documents
        still have the correct prose, but the artifact is wrong."""
        path = self.root / "docs" / "SEARCH_CONSOLE_OBSERVATIONS.json"
        artifact = json.loads(path.read_text())
        artifact["latest_projection"]["performance_impressions"] = 99
        path.write_text(json.dumps(artifact, indent=2) + "\n")
        with self.assertRaisesRegex(
            ObservationError,
            "performance_impressions disagrees",
        ):
            validate(self.root)

    def test_missing_current_interpretation_section_fails(self) -> None:
        path = self.root / "docs" / "SEARCH_VISIBILITY.md"
        text = path.read_text()
        # Remove the Current interpretation heading
        corrupted = text.replace("## Current interpretation", "## Archived interpretation", 1)
        path.write_text(corrupted)
        with self.assertRaisesRegex(ObservationError, "missing 'Current interpretation'"):
            validate(self.root)

    # --- Binding phrase generation tests ---

    def test_visibility_binding_phrases_include_url_inspection(self) -> None:
        artifact = json.loads(self.files["SEARCH_CONSOLE_OBSERVATIONS.json"])
        projection = validate_artifact(artifact)
        phrases = visibility_binding_phrases(projection)
        phrase_texts = [p[0] for p in phrases]
        self.assertIn("five canonical URLs", phrase_texts)
        self.assertIn("positive URL Inspection evidence", phrase_texts)

    def test_roadmap_binding_phrases_include_url_inspection(self) -> None:
        artifact = json.loads(self.files["SEARCH_CONSOLE_OBSERVATIONS.json"])
        projection = validate_artifact(artifact)
        phrases = roadmap_binding_phrases(projection)
        phrase_texts = [p[0] for p in phrases]
        self.assertIn("five canonical website URLs", phrase_texts)
        self.assertIn("positive Google URL Inspection evidence", phrase_texts)

    def test_visibility_binding_phrases_include_performance(self) -> None:
        artifact = json.loads(self.files["SEARCH_CONSOLE_OBSERVATIONS.json"])
        projection = validate_artifact(artifact)
        phrases = visibility_binding_phrases(projection)
        phrase_texts = [p[0] for p in phrases]
        self.assertIn("15 impressions", phrase_texts)
        self.assertIn("2 clicks", phrase_texts)

    def test_roadmap_binding_phrases_include_ctr_and_position(self) -> None:
        artifact = json.loads(self.files["SEARCH_CONSOLE_OBSERVATIONS.json"])
        projection = validate_artifact(artifact)
        phrases = roadmap_binding_phrases(projection)
        phrase_texts = [p[0] for p in phrases]
        self.assertIn("13.3% CTR", phrase_texts)
        self.assertIn("average position 5.7", phrase_texts)

    def test_whitespace_normalization_detects_wrapped_phrases(self) -> None:
        text = "average\nposition 6.3"
        self.assertIn("average position 6.3", normalize_whitespace(text))

    # --- Page indexing aggregate mutation tests (issue #204) ---

    def test_page_indexing_observations_must_be_present(self) -> None:
        """Removing the page_indexing_observations key must fail."""
        path = self.root / "docs" / "SEARCH_CONSOLE_OBSERVATIONS.json"
        artifact = json.loads(path.read_text())
        del artifact["page_indexing_observations"]
        path.write_text(json.dumps(artifact, indent=2) + "\n")
        with self.assertRaisesRegex(ObservationError, "unexpected top-level fields"):
            validate(self.root)

    def test_page_indexing_observations_must_be_nonempty(self) -> None:
        """An empty page_indexing_observations list must fail."""
        path = self.root / "docs" / "SEARCH_CONSOLE_OBSERVATIONS.json"
        artifact = json.loads(path.read_text())
        artifact["page_indexing_observations"] = []
        path.write_text(json.dumps(artifact, indent=2) + "\n")
        with self.assertRaisesRegex(ObservationError, "page_indexing_observations must be a nonempty list"):
            validate(self.root)

    def test_page_indexing_timestamps_must_be_nondecreasing(self) -> None:
        """Page indexing snapshots must be append-only."""
        path = self.root / "docs" / "SEARCH_CONSOLE_OBSERVATIONS.json"
        artifact = json.loads(path.read_text())
        later = json.loads(json.dumps(artifact["page_indexing_observations"][0]))
        later["collected_at"] = "2026-07-30T00:00:00Z"
        earlier = artifact["page_indexing_observations"][0]
        earlier["collected_at"] = "2026-07-28T00:00:00Z"
        artifact["page_indexing_observations"] = [later, earlier]
        path.write_text(json.dumps(artifact, indent=2) + "\n")
        with self.assertRaisesRegex(ObservationError, "nondecreasing"):
            validate(self.root)

    def test_page_indexing_report_last_updated_must_be_date(self) -> None:
        """report_last_updated must be a YYYY-MM-DD date, not a timestamp."""
        path = self.root / "docs" / "SEARCH_CONSOLE_OBSERVATIONS.json"
        artifact = json.loads(path.read_text())
        artifact["page_indexing_observations"][0]["report_last_updated"] = "2026-07-24T00:00:00Z"
        path.write_text(json.dumps(artifact, indent=2) + "\n")
        with self.assertRaisesRegex(ObservationError, "report_last_updated.*YYYY-MM-DD"):
            validate(self.root)

    def test_page_indexing_reason_row_must_preserve_reason(self) -> None:
        """Deleting the reason from a reason row must fail."""
        path = self.root / "docs" / "SEARCH_CONSOLE_OBSERVATIONS.json"
        artifact = json.loads(path.read_text())
        del artifact["page_indexing_observations"][0]["reason_rows"][0]["reason"]
        path.write_text(json.dumps(artifact, indent=2) + "\n")
        with self.assertRaisesRegex(ObservationError, "reason row has unexpected fields"):
            validate(self.root)

    def test_page_indexing_example_last_crawl_must_be_date(self) -> None:
        """example last_crawl must be a YYYY-MM-DD date."""
        path = self.root / "docs" / "SEARCH_CONSOLE_OBSERVATIONS.json"
        artifact = json.loads(path.read_text())
        artifact["page_indexing_observations"][0]["reason_rows"][0]["examples"][0]["last_crawl"] = "2026-07-25T12:00:00Z"
        path.write_text(json.dumps(artifact, indent=2) + "\n")
        with self.assertRaisesRegex(ObservationError, "example last_crawl.*YYYY-MM-DD"):
            validate(self.root)

    def test_page_indexing_data_freshness_must_be_valid(self) -> None:
        """An invalid data_freshness state must fail."""
        path = self.root / "docs" / "SEARCH_CONSOLE_OBSERVATIONS.json"
        artifact = json.loads(path.read_text())
        artifact["page_indexing_observations"][0]["data_freshness"] = "stale"
        path.write_text(json.dumps(artifact, indent=2) + "\n")
        with self.assertRaisesRegex(ObservationError, "data_freshness is invalid"):
            validate(self.root)

    def test_page_indexing_validation_state_must_be_valid(self) -> None:
        """An invalid validation_state must fail."""
        path = self.root / "docs" / "SEARCH_CONSOLE_OBSERVATIONS.json"
        artifact = json.loads(path.read_text())
        artifact["page_indexing_observations"][0]["reason_rows"][0]["validation_state"] = "pending"
        path.write_text(json.dumps(artifact, indent=2) + "\n")
        with self.assertRaisesRegex(ObservationError, "validation_state is invalid"):
            validate(self.root)

    def test_page_indexing_examples_must_be_nonempty(self) -> None:
        """An empty examples list must fail."""
        path = self.root / "docs" / "SEARCH_CONSOLE_OBSERVATIONS.json"
        artifact = json.loads(path.read_text())
        artifact["page_indexing_observations"][0]["reason_rows"][0]["examples"] = []
        path.write_text(json.dumps(artifact, indent=2) + "\n")
        with self.assertRaisesRegex(ObservationError, "examples must be a nonempty list"):
            validate(self.root)

    def test_aggregate_conflation_with_fresh_freshness_fails(self) -> None:
        """Declaring the lagging aggregate 'fresh' while it disagrees with
        the per-URL inspection count must fail."""
        path = self.root / "docs" / "SEARCH_CONSOLE_OBSERVATIONS.json"
        artifact = json.loads(path.read_text())
        artifact["page_indexing_observations"][0]["data_freshness"] = "fresh"
        artifact["latest_projection"]["page_indexing_data_freshness"] = "fresh"
        path.write_text(json.dumps(artifact, indent=2) + "\n")
        with self.assertRaisesRegex(ObservationError, "report_lag_or_unreconciled"):
            validate(self.root)

    def test_aggregate_overwrite_of_per_url_total_fails(self) -> None:
        """Replacing the five positive per-URL inspection total with the
        aggregate count of four must fail the prose binding."""
        path = self.root / "docs" / "SEARCH_CONSOLE_OBSERVATIONS.json"
        artifact = json.loads(path.read_text())
        artifact["observations"][-1]["url_inspection"]["canonical_urls_total"] = 4
        artifact["observations"][-1]["url_inspection"]["canonical_urls_on_google"] = 4
        artifact["latest_projection"]["canonical_urls_total"] = 4
        artifact["latest_projection"]["canonical_urls_on_google"] = 4
        path.write_text(json.dumps(artifact, indent=2) + "\n")
        with self.assertRaisesRegex(ObservationError, "missing bound phrase.*URL inspection canonical count"):
            validate(self.root)

    def test_page_indexing_projection_must_match_latest_observation(self) -> None:
        """The latest_projection page_indexing_indexed must match the latest
        page indexing observation."""
        path = self.root / "docs" / "SEARCH_CONSOLE_OBSERVATIONS.json"
        artifact = json.loads(path.read_text())
        artifact["latest_projection"]["page_indexing_indexed"] = 99
        path.write_text(json.dumps(artifact, indent=2) + "\n")
        with self.assertRaisesRegex(ObservationError, "page_indexing_indexed disagrees"):
            validate(self.root)

    def test_page_indexing_reason_projection_must_match(self) -> None:
        """The latest_projection page_indexing_reason must match the latest
        page indexing observation's first reason row."""
        path = self.root / "docs" / "SEARCH_CONSOLE_OBSERVATIONS.json"
        artifact = json.loads(path.read_text())
        artifact["latest_projection"]["page_indexing_reason"] = "Different reason"
        path.write_text(json.dumps(artifact, indent=2) + "\n")
        with self.assertRaisesRegex(ObservationError, "page_indexing_reason disagrees"):
            validate(self.root)

    def test_page_indexing_example_url_projection_must_match(self) -> None:
        """The latest_projection page_indexing_example_url must match the
        latest page indexing observation's first example URL."""
        path = self.root / "docs" / "SEARCH_CONSOLE_OBSERVATIONS.json"
        artifact = json.loads(path.read_text())
        artifact["latest_projection"]["page_indexing_example_url"] = "https://example.com/"
        path.write_text(json.dumps(artifact, indent=2) + "\n")
        with self.assertRaisesRegex(ObservationError, "page_indexing_example_url disagrees"):
            validate(self.root)

    def test_visibility_binding_includes_page_indexing_aggregate(self) -> None:
        """The visibility binding phrases must include the page indexing
        aggregate counts and lag state."""
        artifact = json.loads(self.files["SEARCH_CONSOLE_OBSERVATIONS.json"])
        projection = validate_artifact(artifact)
        phrases = visibility_binding_phrases(projection)
        phrase_texts = [p[0] for p in phrases]
        self.assertIn("4 indexed", phrase_texts)
        self.assertIn("1 not indexed", phrase_texts)
        self.assertIn("lagging Page indexing aggregate", phrase_texts)
        self.assertIn("Crawled — currently not indexed", phrase_texts)

    def test_roadmap_binding_includes_page_indexing_aggregate(self) -> None:
        """The roadmap binding phrases must include the page indexing
        aggregate counts and lag state."""
        artifact = json.loads(self.files["SEARCH_CONSOLE_OBSERVATIONS.json"])
        projection = validate_artifact(artifact)
        phrases = roadmap_binding_phrases(projection)
        phrase_texts = [p[0] for p in phrases]
        self.assertIn("4 indexed", phrase_texts)
        self.assertIn("1 not indexed", phrase_texts)
        self.assertIn("lagging Page indexing aggregate", phrase_texts)

    def test_corrupted_visibility_page_indexing_count_fails(self) -> None:
        """Corrupting the page indexing aggregate count in the visibility
        Current interpretation while leaving the artifact unchanged must
        fail."""
        path = self.root / "docs" / "SEARCH_VISIBILITY.md"
        text = path.read_text()
        corrupted = text.replace("4 indexed and 1 not indexed", "3 indexed and 2 not indexed", 1)
        self.assertNotEqual(text, corrupted)
        path.write_text(corrupted)
        with self.assertRaisesRegex(ObservationError, "missing bound phrase.*page indexing aggregate"):
            validate(self.root)

    def test_corrupted_roadmap_page_indexing_count_fails(self) -> None:
        """Corrupting the page indexing aggregate count in the roadmap while
        leaving the artifact unchanged must fail."""
        path = self.root / "docs" / "ROADMAP.md"
        text = path.read_text()
        corrupted = text.replace("4 indexed and 1 not indexed", "3 indexed and 2 not indexed", 1)
        self.assertNotEqual(text, corrupted)
        path.write_text(corrupted)
        with self.assertRaisesRegex(ObservationError, "missing bound phrase.*page indexing aggregate"):
            validate(self.root)


class SearchConsoleObservationEntrypointTests(unittest.TestCase):
    """Exercise the real CLI entrypoint end-to-end through a subprocess."""

    def setUp(self) -> None:
        self.repository_root = Path(__file__).resolve().parent.parent

    def _run_entrypoint(self, repository_root: Path) -> subprocess.CompletedProcess:
        return subprocess.run(
            [
                sys.executable,
                "-B",
                str(VALIDATOR_PATH),
                "--repository-root",
                str(repository_root),
            ],
            capture_output=True,
            text=True,
        )

    def test_entrypoint_passes_against_the_clean_tree(self) -> None:
        result = self._run_entrypoint(self.repository_root)
        self.assertEqual(
            result.returncode,
            0,
            f"audit exited {result.returncode}\nstdout:\n{result.stdout}"
            f"\nstderr:\n{result.stderr}",
        )
        self.assertIn("Search console observation audit passed.", result.stdout)

    def test_entrypoint_fails_on_corrupted_ledger_status(self) -> None:
        """Production-path negative test: corrupt the ledger status in
        SEARCH_VISIBILITY.md while leaving the roadmap unchanged, then
        verify the CLI entrypoint exits nonzero."""
        with tempfile.TemporaryDirectory(
            prefix="pycc-search-console-entrypoint-"
        ) as temporary:
            root = Path(temporary)
            docs = root / "docs"
            docs.mkdir(parents=True)
            for name in (
                "SEARCH_CONSOLE_OBSERVATIONS.json",
                "SEARCH_VISIBILITY.md",
                "ROADMAP.md",
            ):
                (docs / name).write_text(
                    (self.repository_root / "docs" / name).read_text()
                )
            visibility = (docs / "SEARCH_VISIBILITY.md").read_text()
            # Corrupt the URL inspection status in the Current interpretation
            # section while leaving the roadmap unchanged.
            original = (
                "All five canonical URLs now have positive URL Inspection evidence"
            )
            corrupted = (
                "All four canonical URLs now have positive URL Inspection evidence"
            )
            self.assertIn(original, visibility)
            (docs / "SEARCH_VISIBILITY.md").write_text(
                visibility.replace(original, corrupted, 1)
            )
            result = self._run_entrypoint(root)
            self.assertNotEqual(
                result.returncode,
                0,
                "audit unexpectedly passed against a corrupted ledger",
            )
            self.assertIn(
                "search console observation audit failed:",
                result.stderr,
            )


if __name__ == "__main__":
    unittest.main()
