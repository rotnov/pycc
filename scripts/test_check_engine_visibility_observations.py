#!/usr/bin/env python3
"""Mutation tests for the engine-qualified visibility observation validator.

These tests exercise the production-path validator through both direct
``validate()`` calls and the real CLI entrypoint.  The negative tests cover
the validation expectations from issue #195:

- missing or malformed artifact fields;
- missing or wrong query suite entries;
- missing or unknown surfaces;
- observations with invalid outcomes;
- ``cited_in_answer`` without captured cited URLs;
- indexability outcomes carrying cited URLs;
- non-append-only observations;
- latest_projection disagreeing with observations;
- prose conflating indexability with visibility or citation;
- missing bound phrases in SEARCH_VISIBILITY.md or ROADMAP.md.
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

VALIDATOR_PATH = Path(__file__).with_name("check_engine_visibility_observations.py")
VALIDATOR_SPEC = importlib.util.spec_from_file_location(
    "check_engine_visibility_observations", VALIDATOR_PATH
)
if VALIDATOR_SPEC is None or VALIDATOR_SPEC.loader is None:
    raise RuntimeError("could not load the engine visibility observation validator")
VALIDATOR_MODULE = importlib.util.module_from_spec(VALIDATOR_SPEC)
VALIDATOR_SPEC.loader.exec_module(VALIDATOR_MODULE)

ObservationError = VALIDATOR_MODULE.ObservationError
validate = VALIDATOR_MODULE.validate
validate_artifact = VALIDATOR_MODULE.validate_artifact
validate_query_suite = VALIDATOR_MODULE.validate_query_suite
validate_surfaces = VALIDATOR_MODULE.validate_surfaces
validate_observation = VALIDATOR_MODULE.validate_observation
check_forbidden_indexability_wording = VALIDATOR_MODULE.check_forbidden_indexability_wording
normalize_whitespace = VALIDATOR_MODULE.normalize_whitespace

REPOSITORY_ROOT = Path(__file__).resolve().parent.parent


def _load_repository_files(repository_root: Path) -> dict[str, str]:
    docs = repository_root / "docs"
    return {
        "ENGINE_VISIBILITY_OBSERVATIONS.json": (docs / "ENGINE_VISIBILITY_OBSERVATIONS.json").read_text(),
        "SEARCH_VISIBILITY.md": (docs / "SEARCH_VISIBILITY.md").read_text(),
        "ROADMAP.md": (docs / "ROADMAP.md").read_text(),
    }


def _base_artifact() -> dict[str, Any]:
    """Return a copy of the real artifact (template with no observations)."""
    return json.loads(
        (REPOSITORY_ROOT / "docs" / "ENGINE_VISIBILITY_OBSERVATIONS.json").read_text()
    )


def _artifact_with_observation(outcome: str = "not_surfaced_in_observed_window") -> dict[str, Any]:
    """Return an artifact with one observation for mutation tests."""
    artifact = _base_artifact()
    obs = {
        "observed_at": "2026-08-14T22:00:00Z",
        "query_suite_id": "named-product-pycc-aot",
        "surface": "google_web_search",
        "prompt": "pycc Python AOT compiler",
        "model_product_version": "unknown",
        "locale": "en",
        "country": "US",
        "personalization_state": "unknown",
        "result_window": "first_page",
        "outcome": outcome,
        "pycc_surfaced": outcome in ("returned_in_result_set", "cited_in_answer"),
        "cited_urls": ["https://rotnov.github.io/pycc/"] if outcome == "cited_in_answer" else [],
        "citation_order": 1 if outcome == "cited_in_answer" else None,
        "competing_entities": [] if outcome != "not_surfaced_in_observed_window" else ["example.com"],
        "transport_tool": "manual_browser",
        "query_rewriting_disclosed": "unknown",
    }
    artifact["observations"] = [obs]
    artifact["latest_projection"] = {
        "observation_count": 1,
        "surfaces_observed": ["google_web_search"],
        "latest_observed_at": "2026-08-14T22:00:00Z",
        "pycc_surfaced_any": obs["pycc_surfaced"],
        "pycc_cited_any": outcome == "cited_in_answer",
    }
    return artifact


def _write_repo(root: Path, files: dict[str, str]) -> None:
    docs = root / "docs"
    docs.mkdir(parents=True, exist_ok=True)
    (docs / "ENGINE_VISIBILITY_OBSERVATIONS.json").write_text(files["ENGINE_VISIBILITY_OBSERVATIONS.json"])
    (docs / "SEARCH_VISIBILITY.md").write_text(files["SEARCH_VISIBILITY.md"])
    (docs / "ROADMAP.md").write_text(files["ROADMAP.md"])


class TestLiveArtifact(unittest.TestCase):
    """The real repository artifact must pass validation."""

    def test_live_artifact_passes(self) -> None:
        result = validate(REPOSITORY_ROOT)
        self.assertEqual(result["observation_count"], 0)
        self.assertFalse(result["pycc_surfaced_any"])
        self.assertFalse(result["pycc_cited_any"])

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
        del artifact["query_suite"]
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


class TestQuerySuite(unittest.TestCase):
    """Query suite validation."""

    def test_rejects_missing_query(self) -> None:
        artifact = _base_artifact()
        artifact["query_suite"] = artifact["query_suite"][:-1]
        with self.assertRaises(ObservationError):
            validate_artifact(artifact)

    def test_rejects_extra_query(self) -> None:
        artifact = _base_artifact()
        artifact["query_suite"].append({
            "id": "extra-query",
            "intent_class": "exact_entity",
            "kpi_role": "diagnostic",
            "prompt": "extra",
            "rationale": "extra",
        })
        with self.assertRaises(ObservationError):
            validate_artifact(artifact)

    def test_rejects_wrong_kpi_for_exact_entity(self) -> None:
        artifact = _base_artifact()
        for q in artifact["query_suite"]:
            if q["id"] == "exact-entity-rotnov-pycc":
                q["kpi_role"] = "product_acquisition"
        with self.assertRaises(ObservationError):
            validate_artifact(artifact)

    def test_rejects_wrong_kpi_for_authorship(self) -> None:
        artifact = _base_artifact()
        for q in artifact["query_suite"]:
            if q["id"] == "ai-authorship-diagnostic":
                q["kpi_role"] = "product_acquisition"
        with self.assertRaises(ObservationError):
            validate_artifact(artifact)

    def test_rejects_duplicate_query_id(self) -> None:
        artifact = _base_artifact()
        artifact["query_suite"][0]["id"] = artifact["query_suite"][1]["id"]
        with self.assertRaises(ObservationError):
            validate_artifact(artifact)

    def test_rejects_unknown_intent_class(self) -> None:
        artifact = _base_artifact()
        artifact["query_suite"][0]["intent_class"] = "random"
        with self.assertRaises(ObservationError):
            validate_artifact(artifact)


class TestSurfaces(unittest.TestCase):
    """Surfaces validation."""

    def test_rejects_missing_surface(self) -> None:
        artifact = _base_artifact()
        del artifact["surfaces"]["google_web_search"]
        with self.assertRaises(ObservationError):
            validate_artifact(artifact)

    def test_rejects_extra_surface(self) -> None:
        artifact = _base_artifact()
        artifact["surfaces"]["extra_engine"] = {
            "provider": "extra", "transport": "extra", "result_window": "first_page"
        }
        with self.assertRaises(ObservationError):
            validate_artifact(artifact)

    def test_rejects_surface_missing_key(self) -> None:
        artifact = _base_artifact()
        del artifact["surfaces"]["google_web_search"]["transport"]
        with self.assertRaises(ObservationError):
            validate_artifact(artifact)


class TestObservations(unittest.TestCase):
    """Observation-level validation."""

    def test_accepts_valid_observation(self) -> None:
        artifact = _artifact_with_observation()
        validate_artifact(artifact)

    def test_rejects_unknown_surface_in_observation(self) -> None:
        artifact = _artifact_with_observation()
        artifact["observations"][0]["surface"] = "yahoo_search"
        with self.assertRaises(ObservationError):
            validate_artifact(artifact)

    def test_rejects_unknown_query_suite_id(self) -> None:
        artifact = _artifact_with_observation()
        artifact["observations"][0]["query_suite_id"] = "nonexistent"
        with self.assertRaises(ObservationError):
            validate_artifact(artifact)

    def test_rejects_invalid_outcome(self) -> None:
        artifact = _artifact_with_observation()
        artifact["observations"][0]["outcome"] = "ranked_first"
        with self.assertRaises(ObservationError):
            validate_artifact(artifact)

    def test_rejects_cited_in_answer_without_cited_urls(self) -> None:
        artifact = _artifact_with_observation(outcome="cited_in_answer")
        artifact["observations"][0]["cited_urls"] = []
        with self.assertRaises(ObservationError):
            validate_artifact(artifact)

    def test_rejects_cited_in_answer_without_citation_order(self) -> None:
        artifact = _artifact_with_observation(outcome="cited_in_answer")
        artifact["observations"][0]["citation_order"] = None
        with self.assertRaises(ObservationError):
            validate_artifact(artifact)

    def test_rejects_indexability_outcome_with_cited_urls(self) -> None:
        artifact = _artifact_with_observation(outcome="crawlable")
        artifact["observations"][0]["cited_urls"] = ["https://example.com/"]
        with self.assertRaises(ObservationError):
            validate_artifact(artifact)

    def test_rejects_non_append_order(self) -> None:
        artifact = _base_artifact()
        obs1 = _artifact_with_observation()["observations"][0]
        obs2 = dict(obs1)
        obs2["observed_at"] = "2026-08-13T22:00:00Z"
        artifact["observations"] = [obs1, obs2]
        artifact["latest_projection"] = {
            "observation_count": 2,
            "surfaces_observed": ["google_web_search"],
            "latest_observed_at": "2026-08-13T22:00:00Z",
            "pycc_surfaced_any": False,
            "pycc_cited_any": False,
        }
        with self.assertRaises(ObservationError):
            validate_artifact(artifact)

    def test_rejects_missing_observation_key(self) -> None:
        artifact = _artifact_with_observation()
        del artifact["observations"][0]["transport_tool"]
        with self.assertRaises(ObservationError):
            validate_artifact(artifact)

    def test_rejects_bad_query_rewriting_value(self) -> None:
        artifact = _artifact_with_observation()
        artifact["observations"][0]["query_rewriting_disclosed"] = "maybe"
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

    def test_rejects_wrong_pycc_surfaced_any(self) -> None:
        artifact = _artifact_with_observation(outcome="returned_in_result_set")
        artifact["latest_projection"]["pycc_surfaced_any"] = False
        with self.assertRaises(ObservationError):
            validate_artifact(artifact)

    def test_rejects_wrong_pycc_cited_any(self) -> None:
        artifact = _artifact_with_observation(outcome="cited_in_answer")
        artifact["latest_projection"]["pycc_cited_any"] = False
        with self.assertRaises(ObservationError):
            validate_artifact(artifact)

    def test_rejects_empty_template_with_non_null_latest(self) -> None:
        artifact = _base_artifact()
        artifact["latest_projection"]["latest_observed_at"] = "2026-08-14T22:00:00Z"
        with self.assertRaises(ObservationError):
            validate_artifact(artifact)

    def test_rejects_empty_template_with_surfaced_true(self) -> None:
        artifact = _base_artifact()
        artifact["latest_projection"]["pycc_surfaced_any"] = True
        with self.assertRaises(ObservationError):
            validate_artifact(artifact)


class TestProseBindings(unittest.TestCase):
    """Prose binding and forbidden wording tests."""

    def test_rejects_forbidden_indexability_wording(self) -> None:
        with self.assertRaises(ObservationError):
            check_forbidden_indexability_wording("crawlable means cited in the answer")

    def test_rejects_indexable_means_visible(self) -> None:
        with self.assertRaises(ObservationError):
            check_forbidden_indexability_wording("indexed means visible to users")

    def test_accepts_clean_prose(self) -> None:
        check_forbidden_indexability_wording(
            "The artifact tracks whether pycc appears in result sets."
        )

    def test_rejects_missing_artifact_reference_in_visibility(self) -> None:
        files = _load_repository_files(REPOSITORY_ROOT)
        files["SEARCH_VISIBILITY.md"] = files["SEARCH_VISIBILITY.md"].replace(
            "ENGINE_VISIBILITY_OBSERVATIONS.json", "REPLACED.json"
        )
        with tempfile.TemporaryDirectory() as tmp:
            _write_repo(Path(tmp), files)
            with self.assertRaises(ObservationError):
                validate(Path(tmp))

    def test_rejects_missing_artifact_reference_in_roadmap(self) -> None:
        files = _load_repository_files(REPOSITORY_ROOT)
        files["ROADMAP.md"] = files["ROADMAP.md"].replace(
            "ENGINE_VISIBILITY_OBSERVATIONS.json", "REPLACED.json"
        )
        with tempfile.TemporaryDirectory() as tmp:
            _write_repo(Path(tmp), files)
            with self.assertRaises(ObservationError):
                validate(Path(tmp))

    def test_rejects_forbidden_wording_in_visibility(self) -> None:
        files = _load_repository_files(REPOSITORY_ROOT)
        files["SEARCH_VISIBILITY.md"] += "\n\ncrawlable means cited in the answer.\n"
        with tempfile.TemporaryDirectory() as tmp:
            _write_repo(Path(tmp), files)
            with self.assertRaises(ObservationError):
                validate(Path(tmp))

    def test_rejects_forbidden_wording_in_roadmap(self) -> None:
        files = _load_repository_files(REPOSITORY_ROOT)
        files["ROADMAP.md"] += "\n\nindexed means cited in the answer.\n"
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
            (docs / "ROADMAP.md").write_text("test")
            result = subprocess.run(
                [sys.executable, str(VALIDATOR_PATH), "--repository-root", tmp],
                capture_output=True, text=True,
            )
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("failed", result.stderr)


if __name__ == "__main__":
    unittest.main()
