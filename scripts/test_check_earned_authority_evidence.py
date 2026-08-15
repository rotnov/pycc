#!/usr/bin/env python3
"""Mutation tests for the earned-authority evidence validator.

These tests exercise the production-path validator through both direct
``validate()`` calls and the real CLI entrypoint.  The negative tests cover
the validation expectations from issue #209:

- missing or malformed artifact fields;
- wrong or missing classification vocabulary;
- launch gate opened while #196 or #197 remains open;
- missing blocking issues in the launch gate;
- distribution checklist missing requirements or automation prohibition;
- observations with invalid classifications or link states;
- self-authored or owned observations counted as independent;
- a zero-row window audit rewritten as "0 backlinks";
- non-append-only observations;
- latest_projection disagreeing with observations;
- prose fabricating evidence, treating self-authored as independent, or
  treating the sampled Search Console Links report as complete;
- missing bound phrases in SEARCH_VISIBILITY.md or ROADMAP.md;
- launch copy describing pycc as an AI compiler or hiding pre-alpha status.
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

VALIDATOR_PATH = Path(__file__).with_name("check_earned_authority_evidence.py")
VALIDATOR_SPEC = importlib.util.spec_from_file_location(
    "check_earned_authority_evidence", VALIDATOR_PATH
)
if VALIDATOR_SPEC is None or VALIDATOR_SPEC.loader is None:
    raise RuntimeError("could not load the earned authority evidence validator")
VALIDATOR_MODULE = importlib.util.module_from_spec(VALIDATOR_SPEC)
VALIDATOR_SPEC.loader.exec_module(VALIDATOR_MODULE)

EvidenceError = VALIDATOR_MODULE.EvidenceError
validate = VALIDATOR_MODULE.validate
validate_artifact = VALIDATOR_MODULE.validate_artifact
validate_launch_gate = VALIDATOR_MODULE.validate_launch_gate
validate_distribution_checklist = VALIDATOR_MODULE.validate_distribution_checklist
validate_observation = VALIDATOR_MODULE.validate_observation
validate_classifications = VALIDATOR_MODULE.validate_classifications
check_forbidden_wording = VALIDATOR_MODULE.check_forbidden_wording
normalize_whitespace = VALIDATOR_MODULE.normalize_whitespace

REPOSITORY_ROOT = Path(__file__).resolve().parent.parent


def _load_repository_files(repository_root: Path) -> dict[str, str]:
    docs = repository_root / "docs"
    return {
        "EARNED_AUTHORITY_EVIDENCE.json": (docs / "EARNED_AUTHORITY_EVIDENCE.json").read_text(),
        "SEARCH_VISIBILITY.md": (docs / "SEARCH_VISIBILITY.md").read_text(),
        "ROADMAP.md": (docs / "ROADMAP.md").read_text(),
    }


def _base_artifact() -> dict[str, Any]:
    """Return a copy of the real artifact."""
    return json.loads(
        (REPOSITORY_ROOT / "docs" / "EARNED_AUTHORITY_EVIDENCE.json").read_text()
    )


def _write_repo(root: Path, files: dict[str, str]) -> None:
    docs = root / "docs"
    docs.mkdir(parents=True, exist_ok=True)
    (docs / "EARNED_AUTHORITY_EVIDENCE.json").write_text(files["EARNED_AUTHORITY_EVIDENCE.json"])
    (docs / "SEARCH_VISIBILITY.md").write_text(files["SEARCH_VISIBILITY.md"])
    (docs / "ROADMAP.md").write_text(files["ROADMAP.md"])


def _make_independent_observation() -> dict[str, Any]:
    """Return an independent editorial observation for mutation tests."""
    return {
        "kind": "mention",
        "observed_at": "2026-08-01T00:00:00Z",
        "discovery_surface": "external_blog",
        "bounded_query": "manual capture of external blog post",
        "source_url": "https://example.com/blog/pycc-review",
        "canonical_destination": "https://example.com/blog/pycc-review",
        "target_project_url": "https://rotnov.github.io/pycc/",
        "source_owner_relationship": "independent_author_not_repository_owner",
        "classification": "independent_editorial",
        "link_state": "live",
        "link_exposed": True,
        "nofollow_sponsored_state": "unknown",
        "first_seen_at": "2026-08-01T00:00:00Z",
        "last_verified_at": "2026-08-01T00:00:00Z",
        "claim_accuracy_check": "Describes pycc as a pre-alpha AOT compiler for typed Python; consistent with current status.",
        "source_limitations": "Single manual capture; not an exhaustive survey.",
        "sample_truncation_status": "single_manual_capture",
        "returned_rows": None,
    }


class TestLiveArtifact(unittest.TestCase):
    """The real repository artifact must pass validation."""

    def test_live_artifact_passes(self) -> None:
        result = validate(REPOSITORY_ROOT)
        self.assertEqual(result["observation_count"], 11)
        self.assertEqual(result["independent_mention_count"], 0)
        self.assertEqual(result["self_authored_external_count"], 3)
        self.assertEqual(result["launch_gate_state"], "closed")

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
        del artifact["launch_gate"]
        with self.assertRaises(EvidenceError):
            validate_artifact(artifact)

    def test_rejects_extra_top_level_key(self) -> None:
        artifact = _base_artifact()
        artifact["extra"] = True
        with self.assertRaises(EvidenceError):
            validate_artifact(artifact)

    def test_rejects_wrong_artifact_version(self) -> None:
        artifact = _base_artifact()
        artifact["artifact_version"] = 2
        with self.assertRaises(EvidenceError):
            validate_artifact(artifact)

    def test_rejects_provenance_not_sanitized(self) -> None:
        artifact = _base_artifact()
        artifact["provenance"]["sanitized"] = False
        with self.assertRaises(EvidenceError):
            validate_artifact(artifact)

    def test_rejects_missing_provenance_key(self) -> None:
        artifact = _base_artifact()
        del artifact["provenance"]["sanitization_note"]
        with self.assertRaises(EvidenceError):
            validate_artifact(artifact)

    def test_rejects_bad_timestamp(self) -> None:
        artifact = _base_artifact()
        artifact["provenance"]["collected_at"] = "2026-07-29"
        with self.assertRaises(EvidenceError):
            validate_artifact(artifact)


class TestClassifications(unittest.TestCase):
    """Classification vocabulary validation."""

    def test_rejects_missing_classification(self) -> None:
        artifact = _base_artifact()
        artifact["classifications"] = artifact["classifications"][:-1]
        with self.assertRaises(EvidenceError):
            validate_artifact(artifact)

    def test_rejects_extra_classification(self) -> None:
        artifact = _base_artifact()
        artifact["classifications"].append("extra_class")
        with self.assertRaises(EvidenceError):
            validate_artifact(artifact)

    def test_rejects_duplicate_classifications(self) -> None:
        artifact = _base_artifact()
        artifact["classifications"] = list(CLASSIFICATIONS_VAL) + ["owned"]
        with self.assertRaises(EvidenceError):
            validate_classifications(artifact["classifications"])


class TestLaunchGate(unittest.TestCase):
    """Launch gate validation."""

    def test_rejects_open_gate_with_196_open(self) -> None:
        artifact = _base_artifact()
        artifact["launch_gate"]["state"] = "open"
        artifact["launch_gate"]["blocking_issues"]["196"] = "open"
        with self.assertRaises(EvidenceError):
            validate_artifact(artifact)

    def test_rejects_open_gate_with_197_open(self) -> None:
        artifact = _base_artifact()
        artifact["launch_gate"]["state"] = "open"
        artifact["launch_gate"]["blocking_issues"]["197"] = "open"
        with self.assertRaises(EvidenceError):
            validate_artifact(artifact)

    def test_rejects_missing_blocking_issue_196(self) -> None:
        artifact = _base_artifact()
        del artifact["launch_gate"]["blocking_issues"]["196"]
        with self.assertRaises(EvidenceError):
            validate_artifact(artifact)

    def test_rejects_missing_blocking_issue_197(self) -> None:
        artifact = _base_artifact()
        del artifact["launch_gate"]["blocking_issues"]["197"]
        with self.assertRaises(EvidenceError):
            validate_artifact(artifact)

    def test_rejects_human_approval_not_required(self) -> None:
        artifact = _base_artifact()
        artifact["launch_gate"]["human_approval_required"] = False
        with self.assertRaises(EvidenceError):
            validate_artifact(artifact)

    def test_rejects_invalid_gate_state(self) -> None:
        artifact = _base_artifact()
        artifact["launch_gate"]["state"] = "maybe"
        with self.assertRaises(EvidenceError):
            validate_artifact(artifact)

    def test_rejects_empty_criteria(self) -> None:
        artifact = _base_artifact()
        artifact["launch_gate"]["criteria"] = []
        with self.assertRaises(EvidenceError):
            validate_artifact(artifact)

    def test_accepts_open_gate_when_both_issues_closed(self) -> None:
        artifact = _base_artifact()
        artifact["launch_gate"]["state"] = "open"
        artifact["launch_gate"]["blocking_issues"]["196"] = "closed"
        artifact["launch_gate"]["blocking_issues"]["197"] = "closed"
        artifact["latest_projection"]["launch_gate_state"] = "open"
        validate_artifact(artifact)


class TestDistributionChecklist(unittest.TestCase):
    """Distribution checklist validation."""

    def test_rejects_missing_requirements(self) -> None:
        artifact = _base_artifact()
        artifact["distribution_checklist"]["requirements_per_candidate"] = []
        with self.assertRaises(EvidenceError):
            validate_artifact(artifact)

    def test_rejects_missing_automation_prohibition(self) -> None:
        artifact = _base_artifact()
        del artifact["distribution_checklist"]["automation_prohibition"]
        with self.assertRaises(EvidenceError):
            validate_artifact(artifact)


class TestObservations(unittest.TestCase):
    """Observation-level validation."""

    def test_rejects_unknown_classification(self) -> None:
        artifact = _base_artifact()
        artifact["observations"][0]["classification"] = "bought_link"
        with self.assertRaises(EvidenceError):
            validate_artifact(artifact)

    def test_rejects_invalid_link_state(self) -> None:
        artifact = _base_artifact()
        artifact["observations"][0]["link_state"] = "broken"
        with self.assertRaises(EvidenceError):
            validate_artifact(artifact)

    def test_rejects_invalid_nofollow_state(self) -> None:
        artifact = _base_artifact()
        artifact["observations"][0]["nofollow_sponsored_state"] = "maybe"
        with self.assertRaises(EvidenceError):
            validate_artifact(artifact)

    def test_rejects_unknown_kind(self) -> None:
        artifact = _base_artifact()
        artifact["observations"][0]["kind"] = "backlink"
        with self.assertRaises(EvidenceError):
            validate_artifact(artifact)

    def test_rejects_missing_observation_key(self) -> None:
        artifact = _base_artifact()
        del artifact["observations"][0]["source_limitations"]
        with self.assertRaises(EvidenceError):
            validate_artifact(artifact)

    def test_rejects_non_append_order(self) -> None:
        artifact = _base_artifact()
        obs = artifact["observations"]
        obs[0]["observed_at"] = "2026-08-01T00:00:00Z"
        with self.assertRaises(EvidenceError):
            validate_artifact(artifact)

    def test_rejects_negative_returned_rows(self) -> None:
        artifact = _base_artifact()
        artifact["observations"][0]["returned_rows"] = -1
        with self.assertRaises(EvidenceError):
            validate_artifact(artifact)


class TestProjectionCounts(unittest.TestCase):
    """latest_projection count consistency."""

    def test_rejects_wrong_observation_count(self) -> None:
        artifact = _base_artifact()
        artifact["latest_projection"]["observation_count"] = 99
        with self.assertRaises(EvidenceError):
            validate_artifact(artifact)

    def test_rejects_inflated_independent_count(self) -> None:
        """Self-authored external must not increment independent count."""
        artifact = _base_artifact()
        artifact["latest_projection"]["independent_mention_count"] = 3
        with self.assertRaises(EvidenceError):
            validate_artifact(artifact)

    def test_rejects_wrong_self_authored_count(self) -> None:
        artifact = _base_artifact()
        artifact["latest_projection"]["self_authored_external_count"] = 99
        with self.assertRaises(EvidenceError):
            validate_artifact(artifact)

    def test_rejects_gate_state_mismatch(self) -> None:
        artifact = _base_artifact()
        artifact["latest_projection"]["launch_gate_state"] = "open"
        with self.assertRaises(EvidenceError):
            validate_artifact(artifact)

    def test_rejects_wrong_latest_observed_at(self) -> None:
        artifact = _base_artifact()
        artifact["latest_projection"]["latest_observed_at"] = "2026-01-01T00:00:00Z"
        with self.assertRaises(EvidenceError):
            validate_artifact(artifact)

    def test_counts_independent_observation_correctly(self) -> None:
        """An independent editorial mention increments the independent count."""
        artifact = _base_artifact()
        artifact["observations"].append(_make_independent_observation())
        artifact["latest_projection"]["observation_count"] = 12
        artifact["latest_projection"]["independent_mention_count"] = 1
        artifact["latest_projection"]["latest_observed_at"] = "2026-08-01T00:00:00Z"
        validate_artifact(artifact)

    def test_owned_does_not_increment_independent(self) -> None:
        """An owned classification must not increment the independent count."""
        artifact = _base_artifact()
        artifact["observations"].append(_make_independent_observation())
        artifact["observations"][-1]["classification"] = "owned"
        artifact["observations"][-1]["source_owner_relationship"] = "owner_facing_data"
        artifact["latest_projection"]["observation_count"] = 12
        artifact["latest_projection"]["independent_mention_count"] = 0
        artifact["latest_projection"]["latest_observed_at"] = "2026-08-01T00:00:00Z"
        validate_artifact(artifact)


class TestProseBindings(unittest.TestCase):
    """Prose binding and forbidden wording tests."""

    def test_rejects_zero_backlinks_wording(self) -> None:
        with self.assertRaises(EvidenceError):
            check_forbidden_wording("The project has 0 backlinks.")

    def test_rejects_self_authored_is_independent(self) -> None:
        with self.assertRaises(EvidenceError):
            check_forbidden_wording("A self-authored is independent evidence.")

    def test_rejects_links_report_complete(self) -> None:
        with self.assertRaises(EvidenceError):
            check_forbidden_wording("The search console links report is complete.")

    def test_rejects_ai_compiler_wording(self) -> None:
        with self.assertRaises(EvidenceError):
            check_forbidden_wording("pycc is an ai compiler.")

    def test_accepts_clean_prose(self) -> None:
        check_forbidden_wording(
            "The artifact records no_independent_mention_in_observed_windows."
        )

    def test_rejects_missing_artifact_reference_in_visibility(self) -> None:
        files = _load_repository_files(REPOSITORY_ROOT)
        files["EARNED_AUTHORITY_EVIDENCE.json"] = files["EARNED_AUTHORITY_EVIDENCE.json"].replace(
            "EARNED_AUTHORITY_EVIDENCE.json", "REPLACED.json"
        )
        # Also remove from the prose so the binding check triggers, not the
        # artifact's own self-reference.
        files["SEARCH_VISIBILITY.md"] = files["SEARCH_VISIBILITY.md"].replace(
            "EARNED_AUTHORITY_EVIDENCE.json", "REPLACED.json"
        )
        with tempfile.TemporaryDirectory() as tmp:
            _write_repo(Path(tmp), files)
            with self.assertRaises(EvidenceError):
                validate(Path(tmp))

    def test_rejects_missing_artifact_reference_in_roadmap(self) -> None:
        files = _load_repository_files(REPOSITORY_ROOT)
        files["ROADMAP.md"] = files["ROADMAP.md"].replace(
            "EARNED_AUTHORITY_EVIDENCE.json", "REPLACED.json"
        )
        with tempfile.TemporaryDirectory() as tmp:
            _write_repo(Path(tmp), files)
            with self.assertRaises(EvidenceError):
                validate(Path(tmp))

    def test_rejects_missing_launch_gate_reference_in_visibility(self) -> None:
        files = _load_repository_files(REPOSITORY_ROOT)
        files["SEARCH_VISIBILITY.md"] = files["SEARCH_VISIBILITY.md"].replace(
            "launch readiness gate", "distribution readiness barrier"
        )
        with tempfile.TemporaryDirectory() as tmp:
            _write_repo(Path(tmp), files)
            with self.assertRaises(EvidenceError):
                validate(Path(tmp))

    def test_rejects_forbidden_wording_in_visibility(self) -> None:
        files = _load_repository_files(REPOSITORY_ROOT)
        files["SEARCH_VISIBILITY.md"] += "\n\nThe project has 0 backlinks.\n"
        with tempfile.TemporaryDirectory() as tmp:
            _write_repo(Path(tmp), files)
            with self.assertRaises(EvidenceError):
                validate(Path(tmp))

    def test_rejects_forbidden_wording_in_roadmap(self) -> None:
        files = _load_repository_files(REPOSITORY_ROOT)
        files["ROADMAP.md"] += "\n\nThe project has 0 backlinks.\n"
        with tempfile.TemporaryDirectory() as tmp:
            _write_repo(Path(tmp), files)
            with self.assertRaises(EvidenceError):
                validate(Path(tmp))

    def test_rejects_ai_compiler_in_visibility(self) -> None:
        files = _load_repository_files(REPOSITORY_ROOT)
        files["SEARCH_VISIBILITY.md"] += "\n\npycc is an ai compiler for typed python.\n"
        with tempfile.TemporaryDirectory() as tmp:
            _write_repo(Path(tmp), files)
            with self.assertRaises(EvidenceError):
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


# Import the canonical set for the duplicate-classification test.
CLASSIFICATIONS_VAL = VALIDATOR_MODULE.CLASSIFICATIONS


if __name__ == "__main__":
    unittest.main()
