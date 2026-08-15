#!/usr/bin/env python3
"""Mutation tests for the GitHub Pages visit observation validator.

These tests exercise the production-path validator through both direct
``validate()`` calls and the real CLI entrypoint.  The negative tests
cover the validation expectations from issue #208:

- missing or malformed artifact fields;
- missing or wrong measurement contract fields;
- observations with invalid collection statuses;
- a non-zeroable status (blocked/delayed/unauthorized) converted to zero;
- non-append-only observations;
- latest_projection disagreeing with observations;
- prose conflating repository views with Pages visits, GSC clicks with
  all-provider visits, or attributing a visit to a query the evidence
  does not expose;
- missing bound phrases in SEARCH_VISIBILITY.md, WEBSITE.md, or ROADMAP.md.
"""

from __future__ import annotations

import copy
import importlib.util
import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from typing import Any

VALIDATOR_PATH = Path(__file__).with_name("check_pages_visit_observations.py")
VALIDATOR_SPEC = importlib.util.spec_from_file_location(
    "check_pages_visit_observations", VALIDATOR_PATH
)
if VALIDATOR_SPEC is None or VALIDATOR_SPEC.loader is None:
    raise RuntimeError("could not load the pages visit observation validator")
VALIDATOR_MODULE = importlib.util.module_from_spec(VALIDATOR_SPEC)
VALIDATOR_SPEC.loader.exec_module(VALIDATOR_MODULE)

ObservationError = VALIDATOR_MODULE.ObservationError
validate = VALIDATOR_MODULE.validate
validate_artifact = VALIDATOR_MODULE.validate_artifact
validate_measurement_contract = VALIDATOR_MODULE.validate_measurement_contract
validate_observation = VALIDATOR_MODULE.validate_observation
check_forbidden_conflation_wording = VALIDATOR_MODULE.check_forbidden_conflation_wording
normalize_whitespace = VALIDATOR_MODULE.normalize_whitespace

REPOSITORY_ROOT = Path(__file__).resolve().parent.parent


def _load_repository_files(repository_root: Path) -> dict[str, str]:
    docs = repository_root / "docs"
    return {
        "PAGES_VISIT_OBSERVATIONS.json": (docs / "PAGES_VISIT_OBSERVATIONS.json").read_text(),
        "SEARCH_VISIBILITY.md": (docs / "SEARCH_VISIBILITY.md").read_text(),
        "WEBSITE.md": (docs / "WEBSITE.md").read_text(),
        "ROADMAP.md": (docs / "ROADMAP.md").read_text(),
    }


def _base_artifact() -> dict[str, Any]:
    """Return a copy of the real artifact (template with no observations)."""
    return json.loads(
        (REPOSITORY_ROOT / "docs" / "PAGES_VISIT_OBSERVATIONS.json").read_text()
    )


def _artifact_with_observation(
    collection_status: str = "available",
    pageviews: int | None = 100,
    visits: int | None = 50,
) -> dict[str, Any]:
    """Return an artifact with one observation for mutation tests."""
    artifact = _base_artifact()
    obs = {
        "observed_at": "2026-08-15T00:00:00Z",
        "data_through_date": "2026-08-14",
        "reporting_timezone": "UTC",
        "provider": "plausible",
        "provider_integration_version": "1.0",
        "collection_status": collection_status,
        "pageviews": pageviews,
        "visits": visits,
        "uniques": 45,
        "unique_definition": "provider-defined unique visitor based on hashed IP and user agent with 24-hour rolling window",
        "per_page": [
            {
                "page": "https://rotnov.github.io/pycc/",
                "pageviews": 40,
                "visits": 20,
            },
            {
                "page": "https://rotnov.github.io/pycc/python-aot-compilers/",
                "pageviews": 30,
                "visits": 15,
            },
        ],
        "source_classes": [
            {"source_class": "google", "visits": 15, "pageviews": 30},
            {"source_class": "direct_or_unattributed", "visits": 10, "pageviews": 20},
        ],
        "primary_conversion_clicks": 5,
        "country": [{"country": "United States", "visits": 10}],
        "device": [{"device": "Mobile", "visits": 30}],
        "sampling_truncation_metadata": {
            "sampled": False,
            "truncated": False,
            "blocker_loss_acknowledged": True,
            "row_limit": None,
        },
        "freshness_note": "data updated 2 hours before observation",
    }
    artifact["observations"] = [obs]
    artifact["latest_projection"] = {
        "observation_count": 1,
        "latest_observed_at": "2026-08-15T00:00:00Z",
        "collection_status": collection_status,
        "total_pageviews": pageviews,
        "total_visits": visits,
        "total_uniques": 45,
        "primary_conversion_clicks": 5,
        "entry_pages_observed": [
            "https://rotnov.github.io/pycc/",
            "https://rotnov.github.io/pycc/python-aot-compilers/",
        ],
        "source_classes_observed": ["google", "direct_or_unattributed"],
    }
    return artifact


def _write_repo(root: Path, files: dict[str, str]) -> None:
    docs = root / "docs"
    docs.mkdir(parents=True, exist_ok=True)
    (docs / "PAGES_VISIT_OBSERVATIONS.json").write_text(files["PAGES_VISIT_OBSERVATIONS.json"])
    (docs / "SEARCH_VISIBILITY.md").write_text(files["SEARCH_VISIBILITY.md"])
    (docs / "WEBSITE.md").write_text(files["WEBSITE.md"])
    (docs / "ROADMAP.md").write_text(files["ROADMAP.md"])


class TestLiveArtifact(unittest.TestCase):
    """The real repository artifact must pass validation."""

    def test_live_artifact_passes(self) -> None:
        result = validate(REPOSITORY_ROOT)
        self.assertEqual(result["observation_count"], 0)
        self.assertEqual(result["collection_status"], "unknown")

    def test_cli_passes(self) -> None:
        result = subprocess.run(
            [sys.executable, str(VALIDATOR_PATH), "--repository-root", str(REPOSITORY_ROOT)],
            capture_output=True, text=True,
        )
        self.assertEqual(result.returncode, 0, result.stderr)


class TestArtifactStructure(unittest.TestCase):
    """Artifact top-level and provenance validation."""

    def test_rejects_missing_top_level_key(self) -> None:
        artifact = _base_artifact()
        del artifact["measurement_contract"]
        with self.assertRaises(ObservationError):
            validate_artifact(artifact)

    def test_rejects_extra_top_level_key(self) -> None:
        artifact = _base_artifact()
        artifact["extra"] = True
        with self.assertRaises(ObservationError):
            validate_artifact(artifact)

    def test_rejects_wrong_artifact_version(self) -> None:
        artifact = _base_artifact()
        artifact["artifact_version"] = 2
        with self.assertRaises(ObservationError):
            validate_artifact(artifact)

    def test_rejects_provenance_not_sanitized(self) -> None:
        artifact = _base_artifact()
        artifact["provenance"]["sanitized"] = False
        with self.assertRaises(ObservationError):
            validate_artifact(artifact)

    def test_rejects_missing_provenance_key(self) -> None:
        artifact = _base_artifact()
        del artifact["provenance"]["sanitization_note"]
        with self.assertRaises(ObservationError):
            validate_artifact(artifact)

    def test_rejects_bad_timestamp(self) -> None:
        artifact = _base_artifact()
        artifact["provenance"]["collected_at"] = "2026-08-14"
        with self.assertRaises(ObservationError):
            validate_artifact(artifact)


class TestMeasurementContract(unittest.TestCase):
    """Measurement contract validation."""

    def test_rejects_missing_contract_key(self) -> None:
        artifact = _base_artifact()
        del artifact["measurement_contract"]["separation_rules"]
        with self.assertRaises(ObservationError):
            validate_artifact(artifact)

    def test_rejects_wrong_timezone(self) -> None:
        artifact = _base_artifact()
        artifact["measurement_contract"]["reporting_timezone"] = "PST"
        with self.assertRaises(ObservationError):
            validate_artifact(artifact)

    def test_rejects_missing_canonical_page(self) -> None:
        artifact = _base_artifact()
        artifact["measurement_contract"]["canonical_pages"] = artifact["measurement_contract"]["canonical_pages"][:-1]
        with self.assertRaises(ObservationError):
            validate_artifact(artifact)

    def test_rejects_wrong_primary_conversion_url(self) -> None:
        artifact = _base_artifact()
        artifact["measurement_contract"]["primary_conversion"]["target_url"] = "https://example.com"
        with self.assertRaises(ObservationError):
            validate_artifact(artifact)

    def test_rejects_missing_source_class(self) -> None:
        artifact = _base_artifact()
        artifact["measurement_contract"]["source_classes"] = [
            sc for sc in artifact["measurement_contract"]["source_classes"]
            if sc != "yandex"
        ]
        with self.assertRaises(ObservationError):
            validate_artifact(artifact)

    def test_rejects_missing_collection_status_value(self) -> None:
        artifact = _base_artifact()
        artifact["measurement_contract"]["collection_status_values"] = [
            v for v in artifact["measurement_contract"]["collection_status_values"]
            if v != "blocked"
        ]
        with self.assertRaises(ObservationError):
            validate_artifact(artifact)

    def test_rejects_missing_forbidden_field(self) -> None:
        artifact = _base_artifact()
        artifact["measurement_contract"]["data_minimization_boundary"]["forbidden_fields"] = [
            f for f in artifact["measurement_contract"]["data_minimization_boundary"]["forbidden_fields"]
            if f != "cookies"
        ]
        with self.assertRaises(ObservationError):
            validate_artifact(artifact)

    def test_rejects_too_few_separation_rules(self) -> None:
        artifact = _base_artifact()
        artifact["measurement_contract"]["separation_rules"] = artifact["measurement_contract"]["separation_rules"][:4]
        with self.assertRaises(ObservationError):
            validate_artifact(artifact)


class TestObservations(unittest.TestCase):
    """Observation-level validation."""

    def test_accepts_valid_observation(self) -> None:
        artifact = _artifact_with_observation()
        validate_artifact(artifact)

    def test_rejects_missing_observation_key(self) -> None:
        artifact = _artifact_with_observation()
        del artifact["observations"][0]["freshness_note"]
        with self.assertRaises(ObservationError):
            validate_artifact(artifact)

    def test_rejects_invalid_collection_status(self) -> None:
        artifact = _artifact_with_observation()
        artifact["observations"][0]["collection_status"] = "pending"
        artifact["latest_projection"]["collection_status"] = "pending"
        with self.assertRaises(ObservationError):
            validate_artifact(artifact)

    def test_rejects_blocked_status_with_zero_pageviews(self) -> None:
        artifact = _artifact_with_observation(collection_status="blocked", pageviews=0, visits=None)
        artifact["latest_projection"]["collection_status"] = "blocked"
        artifact["latest_projection"]["total_pageviews"] = 0
        with self.assertRaises(ObservationError):
            validate_artifact(artifact)

    def test_rejects_delayed_status_with_zero_visits(self) -> None:
        artifact = _artifact_with_observation(collection_status="delayed", pageviews=None, visits=0)
        artifact["latest_projection"]["collection_status"] = "delayed"
        artifact["latest_projection"]["total_visits"] = 0
        with self.assertRaises(ObservationError):
            validate_artifact(artifact)

    def test_rejects_unauthorized_status_with_zero_both(self) -> None:
        artifact = _artifact_with_observation(collection_status="unauthorized", pageviews=0, visits=0)
        artifact["latest_projection"]["collection_status"] = "unauthorized"
        artifact["latest_projection"]["total_pageviews"] = 0
        artifact["latest_projection"]["total_visits"] = 0
        with self.assertRaises(ObservationError):
            validate_artifact(artifact)

    def test_accepts_blocked_status_with_null_counts(self) -> None:
        artifact = _artifact_with_observation(collection_status="blocked", pageviews=None, visits=None)
        artifact["latest_projection"]["collection_status"] = "blocked"
        artifact["latest_projection"]["total_pageviews"] = None
        artifact["latest_projection"]["total_visits"] = None
        validate_artifact(artifact)

    def test_rejects_unknown_source_class_in_observation(self) -> None:
        artifact = _artifact_with_observation()
        artifact["observations"][0]["source_classes"][0]["source_class"] = "yahoo"
        artifact["latest_projection"]["source_classes_observed"] = ["yahoo", "direct_or_unattributed"]
        with self.assertRaises(ObservationError):
            validate_artifact(artifact)

    def test_rejects_non_append_order(self) -> None:
        artifact = _base_artifact()
        obs1 = _artifact_with_observation()["observations"][0]
        obs2 = dict(obs1)
        obs2["observed_at"] = "2026-08-14T00:00:00Z"
        artifact["observations"] = [obs1, obs2]
        artifact["latest_projection"] = {
            "observation_count": 2,
            "latest_observed_at": "2026-08-14T00:00:00Z",
            "collection_status": "available",
            "total_pageviews": 100,
            "total_visits": 50,
            "total_uniques": 45,
            "primary_conversion_clicks": 5,
            "entry_pages_observed": [
                "https://rotnov.github.io/pycc/",
                "https://rotnov.github.io/pycc/python-aot-compilers/",
            ],
            "source_classes_observed": ["google", "direct_or_unattributed"],
        }
        with self.assertRaises(ObservationError):
            validate_artifact(artifact)

    def test_rejects_bad_timezone_in_observation(self) -> None:
        artifact = _artifact_with_observation()
        artifact["observations"][0]["reporting_timezone"] = "EST"
        with self.assertRaises(ObservationError):
            validate_artifact(artifact)

    def test_rejects_bad_sampling_metadata(self) -> None:
        artifact = _artifact_with_observation()
        del artifact["observations"][0]["sampling_truncation_metadata"]["blocker_loss_acknowledged"]
        with self.assertRaises(ObservationError):
            validate_artifact(artifact)


class TestLatestProjection(unittest.TestCase):
    """latest_projection consistency checks."""

    def test_rejects_wrong_observation_count(self) -> None:
        artifact = _artifact_with_observation()
        artifact["latest_projection"]["observation_count"] = 99
        with self.assertRaises(ObservationError):
            validate_artifact(artifact)

    def test_rejects_wrong_latest_observed_at(self) -> None:
        artifact = _artifact_with_observation()
        artifact["latest_projection"]["latest_observed_at"] = "2026-01-01T00:00:00Z"
        with self.assertRaises(ObservationError):
            validate_artifact(artifact)

    def test_rejects_wrong_collection_status(self) -> None:
        artifact = _artifact_with_observation()
        artifact["latest_projection"]["collection_status"] = "blocked"
        with self.assertRaises(ObservationError):
            validate_artifact(artifact)

    def test_rejects_wrong_total_pageviews(self) -> None:
        artifact = _artifact_with_observation()
        artifact["latest_projection"]["total_pageviews"] = 999
        with self.assertRaises(ObservationError):
            validate_artifact(artifact)

    def test_rejects_empty_template_with_non_null_latest(self) -> None:
        artifact = _base_artifact()
        artifact["latest_projection"]["latest_observed_at"] = "2026-08-15T00:00:00Z"
        with self.assertRaises(ObservationError):
            validate_artifact(artifact)

    def test_rejects_empty_template_with_non_unknown_status(self) -> None:
        artifact = _base_artifact()
        artifact["latest_projection"]["collection_status"] = "available"
        with self.assertRaises(ObservationError):
            validate_artifact(artifact)

    def test_rejects_empty_template_with_non_null_pageviews(self) -> None:
        artifact = _base_artifact()
        artifact["latest_projection"]["total_pageviews"] = 0
        with self.assertRaises(ObservationError):
            validate_artifact(artifact)


class TestProseBindings(unittest.TestCase):
    """Prose binding and forbidden wording tests."""

    def test_rejects_forbidden_conflation_wording(self) -> None:
        with self.assertRaises(ObservationError):
            check_forbidden_conflation_wording("repository views are pages visits")

    def test_rejects_gsc_clicks_all_visits(self) -> None:
        with self.assertRaises(ObservationError):
            check_forbidden_conflation_wording("search console clicks are all visits")

    def test_rejects_analytics_ranking_factor(self) -> None:
        with self.assertRaises(ObservationError):
            check_forbidden_conflation_wording("analytics is a ranking factor")

    def test_accepts_clean_prose(self) -> None:
        check_forbidden_conflation_wording(
            "The artifact tracks owner-facing Pages visit observations separately."
        )

    def test_rejects_missing_artifact_reference_in_visibility(self) -> None:
        files = _load_repository_files(REPOSITORY_ROOT)
        files["SEARCH_VISIBILITY.md"] = files["SEARCH_VISIBILITY.md"].replace(
            "PAGES_VISIT_OBSERVATIONS.json", "REPLACED.json"
        )
        with tempfile.TemporaryDirectory() as tmp:
            _write_repo(Path(tmp), files)
            with self.assertRaises(ObservationError):
                validate(Path(tmp))

    def test_rejects_missing_artifact_reference_in_website(self) -> None:
        files = _load_repository_files(REPOSITORY_ROOT)
        files["WEBSITE.md"] = files["WEBSITE.md"].replace(
            "PAGES_VISIT_OBSERVATIONS.json", "REPLACED.json"
        )
        with tempfile.TemporaryDirectory() as tmp:
            _write_repo(Path(tmp), files)
            with self.assertRaises(ObservationError):
                validate(Path(tmp))

    def test_rejects_missing_artifact_reference_in_roadmap(self) -> None:
        files = _load_repository_files(REPOSITORY_ROOT)
        files["ROADMAP.md"] = files["ROADMAP.md"].replace(
            "PAGES_VISIT_OBSERVATIONS.json", "REPLACED.json"
        )
        with tempfile.TemporaryDirectory() as tmp:
            _write_repo(Path(tmp), files)
            with self.assertRaises(ObservationError):
                validate(Path(tmp))

    def test_rejects_forbidden_wording_in_visibility(self) -> None:
        files = _load_repository_files(REPOSITORY_ROOT)
        files["SEARCH_VISIBILITY.md"] += "\n\nrepository views are pages visits.\n"
        with tempfile.TemporaryDirectory() as tmp:
            _write_repo(Path(tmp), files)
            with self.assertRaises(ObservationError):
                validate(Path(tmp))

    def test_rejects_forbidden_wording_in_website(self) -> None:
        files = _load_repository_files(REPOSITORY_ROOT)
        files["WEBSITE.md"] += "\n\nsearch console clicks are all visits.\n"
        with tempfile.TemporaryDirectory() as tmp:
            _write_repo(Path(tmp), files)
            with self.assertRaises(ObservationError):
                validate(Path(tmp))

    def test_rejects_forbidden_wording_in_roadmap(self) -> None:
        files = _load_repository_files(REPOSITORY_ROOT)
        files["ROADMAP.md"] += "\n\nanalytics is a ranking factor.\n"
        with tempfile.TemporaryDirectory() as tmp:
            _write_repo(Path(tmp), files)
            with self.assertRaises(ObservationError):
                validate(Path(tmp))


class TestCLI(unittest.TestCase):
    """CLI entrypoint tests."""

    def test_cli_with_valid_repo(self) -> None:
        result = subprocess.run(
            [sys.executable, str(VALIDATOR_PATH), "--repository-root", str(REPOSITORY_ROOT)],
            capture_output=True, text=True,
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("passed", result.stdout)

    def test_cli_with_missing_artifact(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            docs = Path(tmp) / "docs"
            docs.mkdir()
            (docs / "SEARCH_VISIBILITY.md").write_text("test")
            (docs / "WEBSITE.md").write_text("test")
            (docs / "ROADMAP.md").write_text("test")
            result = subprocess.run(
                [sys.executable, str(VALIDATOR_PATH), "--repository-root", tmp],
                capture_output=True, text=True,
            )
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("failed", result.stderr)


if __name__ == "__main__":
    unittest.main()
