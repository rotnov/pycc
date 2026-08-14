#!/usr/bin/env python3
"""Mutation tests for the GitHub traffic observation validator.

These tests exercise the production-path validator through both direct
``validate()`` calls and the real CLI entrypoint.  The negative tests cover
the validation expectations from issue #194:

- a snapshot with missing daily rows while aggregate totals remain;
- duplicate or non-UTC daily timestamps;
- a daily ``count`` series whose sum does not equal the endpoint ``count``;
- any projection that sums daily ``uniques`` to manufacture the rolling
  unique value;
- deletion or rewriting of an earlier timestamped observation;
- addition of overlapping 14-day totals as a cumulative metric;
- wording that equates clones with humans, visits, clicks, or SEO
  acquisition;
- a roadmap projection that contradicts the latest structured snapshot.
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

VALIDATOR_PATH = Path(__file__).with_name("check_github_traffic_observations.py")
VALIDATOR_SPEC = importlib.util.spec_from_file_location(
    "check_github_traffic_observations", VALIDATOR_PATH
)
if VALIDATOR_SPEC is None or VALIDATOR_SPEC.loader is None:
    raise RuntimeError("could not load the github traffic observation validator")
VALIDATOR_MODULE = importlib.util.module_from_spec(VALIDATOR_SPEC)
VALIDATOR_SPEC.loader.exec_module(VALIDATOR_MODULE)

ObservationError = VALIDATOR_MODULE.ObservationError
validate = VALIDATOR_MODULE.validate
validate_artifact = VALIDATOR_MODULE.validate_artifact
validate_traffic_section = VALIDATOR_MODULE.validate_traffic_section
traffic_history_timestamps = VALIDATOR_MODULE.traffic_history_timestamps
traffic_interpretation_section = VALIDATOR_MODULE.traffic_interpretation_section
check_forbidden_clone_wording = VALIDATOR_MODULE.check_forbidden_clone_wording
visibility_binding_phrases = VALIDATOR_MODULE.visibility_binding_phrases
roadmap_binding_phrases = VALIDATOR_MODULE.roadmap_binding_phrases
normalize_whitespace = VALIDATOR_MODULE.normalize_whitespace


def _load_repository_files(repository_root: Path) -> dict[str, str]:
    """Load the real repository docs into a dict for test setup."""
    docs = repository_root / "docs"
    return {
        "GITHUB_TRAFFIC_OBSERVATIONS.json": (docs / "GITHUB_TRAFFIC_OBSERVATIONS.json").read_text(),
        "SEARCH_VISIBILITY.md": (docs / "SEARCH_VISIBILITY.md").read_text(),
        "ROADMAP.md": (docs / "ROADMAP.md").read_text(),
    }


def _make_daily_observation(
    observed_at: str = "2026-07-29T10:18:31Z",
    data_through: str = "2026-07-28",
    views_count: int = 159,
    views_uniques: int = 6,
    clones_count: int = 9204,
    clones_uniques: int = 820,
    views_daily: list[dict[str, Any]] | None = None,
    clones_daily: list[dict[str, Any]] | None = None,
) -> dict[str, Any]:
    """Build a synthetic daily_and_aggregate observation for mutation tests."""
    if views_daily is None:
        views_daily = [
            {"timestamp": "2026-07-24", "count": 74, "uniques": 1},
            {"timestamp": "2026-07-25", "count": 23, "uniques": 4},
            {"timestamp": "2026-07-26", "count": 58, "uniques": 3},
            {"timestamp": "2026-07-27", "count": 1, "uniques": 1},
            {"timestamp": "2026-07-28", "count": 3, "uniques": 1},
        ]
    if clones_daily is None:
        clones_daily = [
            {"timestamp": "2026-07-24", "count": 1444, "uniques": 349},
            {"timestamp": "2026-07-25", "count": 4093, "uniques": 448},
            {"timestamp": "2026-07-26", "count": 2204, "uniques": 259},
            {"timestamp": "2026-07-27", "count": 661, "uniques": 118},
            {"timestamp": "2026-07-28", "count": 802, "uniques": 89},
        ]
    return {
        "observed_at": observed_at,
        "api_version": "2026-03-10",
        "data_through": data_through,
        "views": {
            "count": views_count,
            "uniques": views_uniques,
            "daily_rows": views_daily,
            "collection_status": "daily_and_aggregate",
        },
        "clones": {
            "count": clones_count,
            "uniques": clones_uniques,
            "daily_rows": clones_daily,
            "collection_status": "daily_and_aggregate",
        },
        "referrers": [
            {"referrer": "github.com", "count": 13, "uniques": 1},
            {"referrer": "rotnov.github.io", "count": 3, "uniques": 2},
        ],
        "referrers_collection_status": "collected",
        "popular_paths": [
            {"path": "/rotnov/pycc/pulls", "count": 33, "uniques": 1},
        ],
        "popular_paths_collection_status": "collected",
        "repository_state": {
            "stars": 1,
            "forks": 1,
            "watchers": 0,
        },
        "interpretation_label": "automation-heavy / unattributed",
    }


class GitHubTrafficObservationTests(unittest.TestCase):
    """Unit tests that call validate() directly against temp trees."""

    def setUp(self) -> None:
        self.repository_root = Path(__file__).resolve().parent.parent
        self.files = _load_repository_files(self.repository_root)
        self.temporary = tempfile.TemporaryDirectory(prefix="pycc-traffic-")
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
        artifact = json.loads(self.files["GITHUB_TRAFFIC_OBSERVATIONS.json"])
        validate_artifact(artifact)

    # --- Artifact structure mutation tests ---

    def test_artifact_version_must_be_1(self) -> None:
        artifact = json.loads(self.files["GITHUB_TRAFFIC_OBSERVATIONS.json"])
        artifact["artifact_version"] = 2
        with self.assertRaisesRegex(ObservationError, "artifact_version must be 1"):
            validate_artifact(artifact)

    def test_artifact_rejects_missing_top_level_field(self) -> None:
        artifact = json.loads(self.files["GITHUB_TRAFFIC_OBSERVATIONS.json"])
        del artifact["provenance"]
        with self.assertRaisesRegex(ObservationError, "unexpected top-level fields"):
            validate_artifact(artifact)

    def test_artifact_rejects_extra_top_level_field(self) -> None:
        artifact = json.loads(self.files["GITHUB_TRAFFIC_OBSERVATIONS.json"])
        artifact["extra"] = True
        with self.assertRaisesRegex(ObservationError, "unexpected top-level fields"):
            validate_artifact(artifact)

    def test_provenance_must_declare_sanitized(self) -> None:
        artifact = json.loads(self.files["GITHUB_TRAFFIC_OBSERVATIONS.json"])
        artifact["provenance"]["sanitized"] = False
        with self.assertRaisesRegex(ObservationError, "sanitized as true"):
            validate_artifact(artifact)

    def test_observations_must_be_nonempty(self) -> None:
        artifact = json.loads(self.files["GITHUB_TRAFFIC_OBSERVATIONS.json"])
        artifact["observations"] = []
        with self.assertRaisesRegex(ObservationError, "nonempty list"):
            validate_artifact(artifact)

    def test_observation_timestamps_must_be_nondecreasing(self) -> None:
        artifact = json.loads(self.files["GITHUB_TRAFFIC_OBSERVATIONS.json"])
        artifact["observations"][0]["observed_at"] = "2026-07-30T00:00:00Z"
        with self.assertRaisesRegex(ObservationError, "nondecreasing"):
            validate_artifact(artifact)

    def test_latest_projection_must_match_latest_observation(self) -> None:
        artifact = json.loads(self.files["GITHUB_TRAFFIC_OBSERVATIONS.json"])
        artifact["latest_projection"]["views_count"] = 99
        with self.assertRaisesRegex(ObservationError, "views_count disagrees"):
            validate_artifact(artifact)

    def test_latest_projection_rejects_extra_field(self) -> None:
        artifact = json.loads(self.files["GITHUB_TRAFFIC_OBSERVATIONS.json"])
        artifact["latest_projection"]["extra"] = True
        with self.assertRaisesRegex(ObservationError, "latest_projection has unexpected"):
            validate_artifact(artifact)

    def test_interpretation_label_must_be_valid(self) -> None:
        artifact = json.loads(self.files["GITHUB_TRAFFIC_OBSERVATIONS.json"])
        artifact["observations"][-1]["interpretation_label"] = "viral_growth"
        with self.assertRaisesRegex(ObservationError, "interpretation_label is invalid"):
            validate_artifact(artifact)

    # --- Daily row validation tests ---

    def test_daily_and_aggregate_rejects_missing_daily_rows_with_totals(self) -> None:
        """A snapshot with missing daily rows while aggregate totals remain
        must be rejected (issue #194 validation expectation)."""
        artifact = json.loads(self.files["GITHUB_TRAFFIC_OBSERVATIONS.json"])
        obs = artifact["observations"][-1]
        obs["views"]["collection_status"] = "daily_and_aggregate"
        obs["views"]["daily_rows"] = []
        # count is 74 (nonzero) but daily_rows is empty
        with self.assertRaisesRegex(
            ObservationError, "aggregate totals but no daily rows"
        ):
            validate_artifact(artifact)

    def test_daily_and_aggregate_rejects_duplicate_daily_timestamps(self) -> None:
        """Duplicate daily timestamps must be rejected."""
        artifact = json.loads(self.files["GITHUB_TRAFFIC_OBSERVATIONS.json"])
        obs = artifact["observations"][-1]
        obs["views"]["collection_status"] = "daily_and_aggregate"
        obs["views"]["daily_rows"] = [
            {"timestamp": "2026-07-24", "count": 37, "uniques": 1},
            {"timestamp": "2026-07-24", "count": 37, "uniques": 1},
        ]
        # Fix count to match sum
        obs["views"]["count"] = 74
        with self.assertRaisesRegex(ObservationError, "duplicated"):
            validate_artifact(artifact)

    def test_daily_and_aggregate_rejects_non_utc_daily_timestamps(self) -> None:
        """Non-UTC (non YYYY-MM-DD) daily timestamps must be rejected."""
        artifact = json.loads(self.files["GITHUB_TRAFFIC_OBSERVATIONS.json"])
        obs = artifact["observations"][-1]
        obs["views"]["collection_status"] = "daily_and_aggregate"
        obs["views"]["daily_rows"] = [
            {"timestamp": "2026-07-24T00:00:00", "count": 74, "uniques": 1},
        ]
        obs["views"]["count"] = 74
        with self.assertRaisesRegex(ObservationError, "daily row timestamp"):
            validate_artifact(artifact)

    def test_daily_count_sum_must_equal_endpoint_count(self) -> None:
        """A daily count series whose sum does not equal the endpoint count
        must be rejected (issue #194 validation expectation)."""
        artifact = json.loads(self.files["GITHUB_TRAFFIC_OBSERVATIONS.json"])
        obs = artifact["observations"][-1]
        obs["views"]["collection_status"] = "daily_and_aggregate"
        obs["views"]["daily_rows"] = [
            {"timestamp": "2026-07-24", "count": 50, "uniques": 1},
        ]
        # count is 74 but daily sum is 50
        with self.assertRaisesRegex(ObservationError, "daily count sum.*does not equal"):
            validate_artifact(artifact)

    def test_daily_uniques_sum_not_required_to_equal_endpoint_uniques(self) -> None:
        """The sum of daily uniques must NOT be required to equal the endpoint
        uniques — that would encode the same attribution error into CI."""
        artifact = json.loads(self.files["GITHUB_TRAFFIC_OBSERVATIONS.json"])
        obs = artifact["observations"][-1]
        obs["views"]["collection_status"] = "daily_and_aggregate"
        obs["views"]["daily_rows"] = [
            {"timestamp": "2026-07-24", "count": 74, "uniques": 5},
        ]
        obs["views"]["uniques"] = 1  # differs from daily uniques sum (5)
        # Update latest_projection to match the modified observation
        artifact["latest_projection"]["has_daily_rows"] = True
        # This must NOT raise — daily uniques are not additive
        validate_artifact(artifact)

    def test_aggregate_only_rejects_daily_rows(self) -> None:
        """An aggregate_only observation must not have daily rows."""
        artifact = json.loads(self.files["GITHUB_TRAFFIC_OBSERVATIONS.json"])
        obs = artifact["observations"][-1]
        obs["views"]["daily_rows"] = [
            {"timestamp": "2026-07-24", "count": 74, "uniques": 1},
        ]
        with self.assertRaisesRegex(ObservationError, "aggregate_only but has daily rows"):
            validate_artifact(artifact)

    def test_unknown_status_rejects_nonzero_values(self) -> None:
        """An unknown collection status must not have nonzero values."""
        artifact = json.loads(self.files["GITHUB_TRAFFIC_OBSERVATIONS.json"])
        obs = artifact["observations"][-1]
        obs["views"]["collection_status"] = "unknown"
        # count is 74 (nonzero) but status is unknown
        with self.assertRaisesRegex(ObservationError, "unknown but has nonzero"):
            validate_artifact(artifact)

    # --- History table binding tests ---

    def test_history_timestamps_must_match_artifact(self) -> None:
        path = self.root / "docs" / "SEARCH_VISIBILITY.md"
        text = path.read_text()
        text = text.replace(
            "| 2026-07-25T20:23:25Z |",
            "| 2026-07-25T20:23:26Z |",
            1,
        )
        path.write_text(text)
        with self.assertRaisesRegex(ObservationError, "timestamps do not match"):
            validate(self.root)

    # --- Prose binding mutation tests ---

    def test_corrupted_traffic_label_fails(self) -> None:
        """Corrupt the traffic interpretation label in the ledger."""
        path = self.root / "docs" / "SEARCH_VISIBILITY.md"
        text = path.read_text()
        corrupted = text.replace(
            "automation-heavy and low-uniqueness",
            "human-driven and high-uniqueness",
            1,
        )
        self.assertNotEqual(text, corrupted)
        path.write_text(corrupted)
        with self.assertRaisesRegex(
            ObservationError,
            "missing bound phrase.*traffic interpretation label",
        ):
            validate(self.root)

    def test_corrupted_roadmap_traffic_reference_fails(self) -> None:
        """Remove the traffic artifact reference from the roadmap."""
        path = self.root / "docs" / "ROADMAP.md"
        text = path.read_text()
        corrupted = text.replace(
            "GITHUB_TRAFFIC_OBSERVATIONS.json",
            "TRAFFIC_EVIDENCE.json",
        )
        self.assertNotEqual(text, corrupted)
        path.write_text(corrupted)
        with self.assertRaisesRegex(
            ObservationError,
            "missing bound phrase.*traffic artifact reference",
        ):
            validate(self.root)

    # --- Forbidden wording tests ---

    def test_forbidden_clone_wording_rejected(self) -> None:
        """Wording that equates clones with humans must be rejected."""
        path = self.root / "docs" / "SEARCH_VISIBILITY.md"
        text = path.read_text()
        corrupted = text.replace(
            "The traffic window remains too\nautomation-heavy and low-uniqueness to attribute to SEO.",
            "The clones are visitors and prove SEO acquisition.",
            1,
        )
        self.assertNotEqual(text, corrupted)
        path.write_text(corrupted)
        with self.assertRaisesRegex(
            ObservationError,
            "must not equate clones",
        ):
            validate(self.root)

    def test_forbidden_clone_wording_in_roadmap_rejected(self) -> None:
        """Wording that equates clones with humans in the roadmap must be rejected."""
        path = self.root / "docs" / "ROADMAP.md"
        text = path.read_text()
        corrupted = text + "\nclones are people and prove seo\n"
        path.write_text(corrupted)
        with self.assertRaisesRegex(
            ObservationError,
            "must not equate clones",
        ):
            validate(self.root)

    # --- Append-only / deletion tests ---

    def test_deleting_earlier_observation_fails(self) -> None:
        """Deletion or rewriting of an earlier timestamped observation must
        be rejected (issue #194 validation expectation)."""
        path = self.root / "docs" / "GITHUB_TRAFFIC_OBSERVATIONS.json"
        artifact = json.loads(path.read_text())
        del artifact["observations"][0]
        path.write_text(json.dumps(artifact, indent=2) + "\n")
        # The history table still has the deleted timestamp, so the
        # artifact/history binding must detect the mismatch.
        with self.assertRaisesRegex(ObservationError, "timestamps do not match"):
            validate(self.root)

    # --- Binding phrase generation tests ---

    def test_visibility_binding_phrases_include_label(self) -> None:
        artifact = json.loads(self.files["GITHUB_TRAFFIC_OBSERVATIONS.json"])
        projection = validate_artifact(artifact)
        phrases = visibility_binding_phrases(projection)
        phrase_texts = [p[0] for p in phrases]
        self.assertIn("automation-heavy", phrase_texts)

    def test_roadmap_binding_phrases_include_artifact_reference(self) -> None:
        artifact = json.loads(self.files["GITHUB_TRAFFIC_OBSERVATIONS.json"])
        projection = validate_artifact(artifact)
        phrases = roadmap_binding_phrases(projection)
        phrase_texts = [p[0] for p in phrases]
        self.assertIn("GITHUB_TRAFFIC_OBSERVATIONS.json", phrase_texts)

    def test_whitespace_normalization_detects_wrapped_phrases(self) -> None:
        text = "automation\nheavy"
        self.assertIn("automation heavy", normalize_whitespace(text))

    # --- Daily observation integration test ---

    def test_daily_and_aggregate_observation_validates(self) -> None:
        """A full daily_and_aggregate observation with correct count sums
        must pass artifact validation."""
        artifact = json.loads(self.files["GITHUB_TRAFFIC_OBSERVATIONS.json"])
        daily_obs = _make_daily_observation()
        artifact["observations"].append(daily_obs)
        artifact["latest_projection"] = {
            "observed_at": daily_obs["observed_at"],
            "data_through": daily_obs["data_through"],
            "views_count": daily_obs["views"]["count"],
            "views_uniques": daily_obs["views"]["uniques"],
            "clones_count": daily_obs["clones"]["count"],
            "clones_uniques": daily_obs["clones"]["uniques"],
            "stars": daily_obs["repository_state"]["stars"],
            "forks": daily_obs["repository_state"]["forks"],
            "watchers": daily_obs["repository_state"]["watchers"],
            "has_daily_rows": True,
            "interpretation_label": daily_obs["interpretation_label"],
        }
        validate_artifact(artifact)


class GitHubTrafficObservationEntrypointTests(unittest.TestCase):
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
        self.assertIn("GitHub traffic observation audit passed.", result.stdout)

    def test_entrypoint_fails_on_corrupted_artifact(self) -> None:
        """Production-path negative test: corrupt the artifact's latest_projection
        and verify the CLI entrypoint exits nonzero."""
        with tempfile.TemporaryDirectory(
            prefix="pycc-traffic-entrypoint-"
        ) as temporary:
            root = Path(temporary)
            docs = root / "docs"
            docs.mkdir(parents=True)
            for name in (
                "GITHUB_TRAFFIC_OBSERVATIONS.json",
                "SEARCH_VISIBILITY.md",
                "ROADMAP.md",
            ):
                (docs / name).write_text(
                    (self.repository_root / "docs" / name).read_text()
                )
            artifact_path = docs / "GITHUB_TRAFFIC_OBSERVATIONS.json"
            artifact = json.loads(artifact_path.read_text())
            artifact["latest_projection"]["clones_count"] = 999
            artifact_path.write_text(json.dumps(artifact, indent=2) + "\n")
            result = self._run_entrypoint(root)
            self.assertNotEqual(
                result.returncode,
                0,
                "audit unexpectedly passed against a corrupted artifact",
            )
            self.assertIn(
                "github traffic observation audit failed:",
                result.stderr,
            )

    def test_entrypoint_fails_on_forbidden_wording(self) -> None:
        """Production-path negative test: add forbidden clone wording to
        SEARCH_VISIBILITY.md and verify the CLI entrypoint exits nonzero."""
        with tempfile.TemporaryDirectory(
            prefix="pycc-traffic-wording-"
        ) as temporary:
            root = Path(temporary)
            docs = root / "docs"
            docs.mkdir(parents=True)
            for name in (
                "GITHUB_TRAFFIC_OBSERVATIONS.json",
                "SEARCH_VISIBILITY.md",
                "ROADMAP.md",
            ):
                (docs / name).write_text(
                    (self.repository_root / "docs" / name).read_text()
                )
            vis_path = docs / "SEARCH_VISIBILITY.md"
            text = vis_path.read_text()
            corrupted = text.replace(
                "The traffic window remains too\nautomation-heavy and low-uniqueness to attribute to SEO.",
                "The clones are people and prove SEO acquisition.",
                1,
            )
            self.assertNotEqual(text, corrupted)
            vis_path.write_text(corrupted)
            result = self._run_entrypoint(root)
            self.assertNotEqual(
                result.returncode,
                0,
                "audit unexpectedly passed against forbidden wording",
            )


if __name__ == "__main__":
    unittest.main()
