#!/usr/bin/env python3
"""Validate the earned-authority evidence artifact and its prose bindings.

The artifact ``docs/EARNED_AUTHORITY_EVIDENCE.json`` is the append-only
structured source of truth for external-mention evidence and the launch
readiness gate.  It reuses #163's provenance-bearing evidence envelope and
#195's provider-qualified vocabulary.  It does NOT authorize posting,
messaging third parties, or manufacturing links.

This validator checks that:

1. The artifact is well-formed, sanitized, and append-only.
2. The ``classifications`` vocabulary is the closed set from issue #209.
3. The ``launch_gate`` records a human-approved state with blocking issues
   #196 and #197, and remains closed while either is open.
4. The ``distribution_checklist`` records the per-candidate requirements and
   the automation prohibition.
5. Each observation records all required fields with a valid classification
   and link state.
6. ``self_authored_external`` and ``owned`` observations never increment an
   independent count.
7. A ``window_audit`` with zero returned rows is preserved as a window
   result, never rewritten as "0 backlinks".
8. The ``latest_projection`` agrees with the observations.
9. The earned-authority prose in ``SEARCH_VISIBILITY.md`` and the roadmap
   projection both reference the artifact and the launch gate.
10. No prose fabricates evidence, treats self-authored references as
    independent, or treats the sampled Search Console Links report as
    complete.
"""

from __future__ import annotations

import argparse
import json
from pathlib import Path
import re
import sys
from typing import Any


ARTIFACT_PATH = Path("docs") / "EARNED_AUTHORITY_EVIDENCE.json"
VISIBILITY_PATH = Path("docs") / "SEARCH_VISIBILITY.md"
ROADMAP_PATH = Path("docs") / "ROADMAP.md"

UTC_TIMESTAMP = re.compile(r"\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z\Z")

ARTIFACT_KEYS = {
    "artifact_version",
    "provenance",
    "classifications",
    "launch_gate",
    "distribution_checklist",
    "observations",
    "latest_projection",
}
PROVENANCE_KEYS = {"source", "sanitized", "collection_method", "collected_at", "sanitization_note"}

CLASSIFICATIONS = {
    "owned",
    "self_authored_external",
    "independent_editorial",
    "community",
    "directory",
    "automated_mirror",
    "spam",
    "unknown",
}

INDEPENDENT_CLASSIFICATIONS = {
    "independent_editorial",
    "community",
    "directory",
}

LAUNCH_GATE_KEYS = {
    "state",
    "last_reviewed_at",
    "human_approval_required",
    "blocking_issues",
    "criteria",
}
LAUNCH_GATE_STATES = {"open", "closed"}

DISTRIBUTION_CHECKLIST_KEYS = {
    "candidates",
    "requirements_per_candidate",
    "automation_prohibition",
}

OBSERVATION_KINDS = {"window_audit", "mention", "owner_inventory"}

LINK_STATES = {"live", "redirect", "removed", "not_applicable"}

NOFOLLOW_STATES = {"known_nofollow", "known_dofollow", "known_sponsored", "unknown", "not_applicable"}

OBSERVATION_KEYS = {
    "kind",
    "observed_at",
    "discovery_surface",
    "bounded_query",
    "source_url",
    "canonical_destination",
    "target_project_url",
    "source_owner_relationship",
    "classification",
    "link_state",
    "link_exposed",
    "nofollow_sponsored_state",
    "first_seen_at",
    "last_verified_at",
    "claim_accuracy_check",
    "source_limitations",
    "sample_truncation_status",
    "returned_rows",
}

LATEST_PROJECTION_KEYS = {
    "observation_count",
    "independent_mention_count",
    "self_authored_external_count",
    "launch_gate_state",
    "current_state",
    "owner_backlink_inventory",
    "latest_observed_at",
}

# Wording that fabricates evidence or conflates classifications.
# These are positive assertion phrases, not negated rejections — prose that
# says "does not position pycc as an AI compiler" is a guardrail, not a
# fabrication, so the forbidden list targets affirmative claims only.
FORBIDDEN_WORDING = [
    "0 backlinks",
    "zero backlinks",
    "no backlinks exist",
    "no backlinks anywhere",
    "search console links report is complete",
    "links report is comprehensive",
    "links report is exhaustive",
    "self-authored is independent",
    "self authored is independent",
    "owner backlink inventory is complete",
    "owner backlink inventory is exhaustive",
    "owner backlink inventory is comprehensive",
    "pycc is an ai compiler",
    "pycc is a ai compiler",
    "pycc is an ai-native compiler",
]

# Phrases the prose must contain so a contradictory projection is caught.
VISIBILITY_BINDING_PHRASES = [
    ("EARNED_AUTHORITY_EVIDENCE.json", "earned-authority artifact reference"),
    ("launch readiness gate", "launch readiness gate reference"),
    ("no_independent_mention_in_observed_windows", "current-state projection"),
    ("owner_backlink_inventory", "owner backlink inventory state"),
    ("self_authored_external", "self-authored classification vocabulary"),
    ("independent_editorial", "independent editorial classification vocabulary"),
    ("append-only", "append-only evidence contract"),
]

ROADMAP_BINDING_PHRASES = [
    ("EARNED_AUTHORITY_EVIDENCE.json", "earned-authority artifact reference in roadmap"),
    ("launch readiness gate", "launch readiness gate reference in roadmap"),
    ("no_independent_mention_in_observed_windows", "current-state projection in roadmap"),
]


class EvidenceError(ValueError):
    """Raised when the earned-authority artifact or its prose projection is invalid."""


def load_json(path: Path) -> dict[str, Any]:
    try:
        text = path.read_text()
    except OSError as error:
        raise EvidenceError(f"could not read {path}: {error}") from error
    try:
        value = json.loads(text)
    except json.JSONDecodeError as error:
        raise EvidenceError(f"could not parse JSON from {path}: {error}") from error
    if not isinstance(value, dict):
        raise EvidenceError(f"expected a JSON object in {path}")
    return value


def require_string(value: Any, description: str) -> str:
    if not isinstance(value, str) or not value.strip():
        raise EvidenceError(f"{description} must be a nonempty string")
    return value


def require_bool(value: Any, description: str) -> bool:
    if not isinstance(value, bool):
        raise EvidenceError(f"{description} must be a boolean")
    return value


def validate_timestamp(value: Any, description: str) -> str:
    if not isinstance(value, str) or not UTC_TIMESTAMP.fullmatch(value):
        raise EvidenceError(f"{description} must use YYYY-MM-DDTHH:MM:SSZ")
    return value


def validate_optional_url(value: Any, description: str) -> str | None:
    if value is None:
        return None
    if not isinstance(value, str) or not value.strip():
        raise EvidenceError(f"{description} must be a nonempty string or null")
    return value


def validate_classifications(classifications: Any) -> set[str]:
    if not isinstance(classifications, list):
        raise EvidenceError("classifications must be a list")
    if set(classifications) != CLASSIFICATIONS:
        raise EvidenceError(
            f"classifications must be exactly the closed set {CLASSIFICATIONS}, got {set(classifications)}"
        )
    if len(classifications) != len(CLASSIFICATIONS):
        raise EvidenceError("classifications has duplicates")
    return set(classifications)


def validate_launch_gate(launch_gate: Any) -> dict[str, Any]:
    if not isinstance(launch_gate, dict) or set(launch_gate) != LAUNCH_GATE_KEYS:
        raise EvidenceError("launch_gate has unexpected fields")
    state = require_string(launch_gate["state"], "launch_gate state")
    if state not in LAUNCH_GATE_STATES:
        raise EvidenceError(f"launch_gate state {state!r} is invalid")
    validate_timestamp(launch_gate["last_reviewed_at"], "launch_gate last_reviewed_at")
    if not require_bool(launch_gate["human_approval_required"], "launch_gate human_approval_required"):
        raise EvidenceError("launch_gate human_approval_required must be true")
    blocking = launch_gate["blocking_issues"]
    if not isinstance(blocking, dict):
        raise EvidenceError("launch_gate blocking_issues must be an object")
    for issue_num in ("196", "197"):
        if issue_num not in blocking:
            raise EvidenceError(f"launch_gate blocking_issues must include issue {issue_num}")
        status = require_string(blocking[issue_num], f"launch_gate blocking_issues[{issue_num}]")
        if status not in ("open", "closed"):
            raise EvidenceError(f"launch_gate blocking_issues[{issue_num}] must be open or closed")
    criteria = launch_gate["criteria"]
    if not isinstance(criteria, list) or not criteria:
        raise EvidenceError("launch_gate criteria must be a nonempty list")
    for criterion in criteria:
        require_string(criterion, "launch_gate criterion")
    # Gate must remain closed while #196 or #197 is open.
    if state == "open" and (blocking["196"] == "open" or blocking["197"] == "open"):
        raise EvidenceError(
            "launch gate cannot be open while #196 or #197 remains open"
        )
    return launch_gate


def validate_distribution_checklist(checklist: Any) -> dict[str, Any]:
    if not isinstance(checklist, dict) or set(checklist) != DISTRIBUTION_CHECKLIST_KEYS:
        raise EvidenceError("distribution_checklist has unexpected fields")
    if not isinstance(checklist["candidates"], list):
        raise EvidenceError("distribution_checklist candidates must be a list")
    for candidate in checklist["candidates"]:
        if not isinstance(candidate, dict):
            raise EvidenceError("distribution_checklist candidate must be an object")
    requirements = checklist["requirements_per_candidate"]
    if not isinstance(requirements, list) or not requirements:
        raise EvidenceError("distribution_checklist requirements_per_candidate must be a nonempty list")
    for req in requirements:
        require_string(req, "distribution_checklist requirement")
    require_string(
        checklist["automation_prohibition"],
        "distribution_checklist automation_prohibition",
    )
    return checklist


def validate_observation(obs: Any) -> dict[str, Any]:
    if not isinstance(obs, dict) or set(obs) != OBSERVATION_KEYS:
        raise EvidenceError("observation has unexpected fields")
    kind = require_string(obs["kind"], "observation kind")
    if kind not in OBSERVATION_KINDS:
        raise EvidenceError(f"observation kind {kind!r} is invalid")
    ts = validate_timestamp(obs["observed_at"], "observation observed_at")
    require_string(obs["discovery_surface"], "observation discovery_surface")
    require_string(obs["bounded_query"], "observation bounded_query")
    validate_optional_url(obs["source_url"], "observation source_url")
    validate_optional_url(obs["canonical_destination"], "observation canonical_destination")
    require_string(obs["target_project_url"], "observation target_project_url")
    require_string(obs["source_owner_relationship"], "observation source_owner_relationship")
    classification = require_string(obs["classification"], "observation classification")
    if classification not in CLASSIFICATIONS:
        raise EvidenceError(f"observation classification {classification!r} is invalid")
    link_state = require_string(obs["link_state"], "observation link_state")
    if link_state not in LINK_STATES:
        raise EvidenceError(f"observation link_state {link_state!r} is invalid")
    if not require_bool(obs["link_exposed"], "observation link_exposed"):
        pass
    nofollow = require_string(obs["nofollow_sponsored_state"], "observation nofollow_sponsored_state")
    if nofollow not in NOFOLLOW_STATES:
        raise EvidenceError(f"observation nofollow_sponsored_state {nofollow!r} is invalid")
    validate_timestamp(obs["first_seen_at"], "observation first_seen_at")
    validate_timestamp(obs["last_verified_at"], "observation last_verified_at")
    require_string(obs["claim_accuracy_check"], "observation claim_accuracy_check")
    require_string(obs["source_limitations"], "observation source_limitations")
    require_string(obs["sample_truncation_status"], "observation sample_truncation_status")
    returned_rows = obs["returned_rows"]
    if returned_rows is not None:
        if not isinstance(returned_rows, int) or isinstance(returned_rows, bool) or returned_rows < 0:
            raise EvidenceError("observation returned_rows must be a non-negative integer or null")
    return obs


def validate_artifact(artifact: dict[str, Any]) -> dict[str, Any]:
    if set(artifact) != ARTIFACT_KEYS:
        raise EvidenceError("artifact has unexpected top-level fields")
    if not isinstance(artifact["artifact_version"], int) or artifact["artifact_version"] != 1:
        raise EvidenceError("artifact_version must be 1")

    provenance = artifact["provenance"]
    if not isinstance(provenance, dict) or set(provenance) != PROVENANCE_KEYS:
        raise EvidenceError("provenance has unexpected fields")
    require_string(provenance["source"], "provenance source")
    if not require_bool(provenance["sanitized"], "provenance sanitized"):
        raise EvidenceError("provenance must declare sanitized as true")
    require_string(provenance["collection_method"], "provenance collection_method")
    validate_timestamp(provenance["collected_at"], "provenance collected_at")
    require_string(provenance["sanitization_note"], "provenance sanitization_note")

    validate_classifications(artifact["classifications"])
    launch_gate = validate_launch_gate(artifact["launch_gate"])
    validate_distribution_checklist(artifact["distribution_checklist"])

    observations = artifact["observations"]
    if not isinstance(observations, list):
        raise EvidenceError("observations must be a list")

    previous_ts: str | None = None
    for obs in observations:
        validated = validate_observation(obs)
        ts = validated["observed_at"]
        if previous_ts is not None and ts < previous_ts:
            raise EvidenceError(
                "observation timestamps must be nondecreasing (append-only)"
            )
        previous_ts = ts

    latest_projection = artifact["latest_projection"]
    if not isinstance(latest_projection, dict) or set(latest_projection) != LATEST_PROJECTION_KEYS:
        raise EvidenceError("latest_projection has unexpected fields")

    lp = latest_projection
    if not isinstance(lp["observation_count"], int) or lp["observation_count"] < 0:
        raise EvidenceError("latest_projection observation_count must be a non-negative integer")
    if lp["observation_count"] != len(observations):
        raise EvidenceError(
            f"latest_projection observation_count ({lp['observation_count']}) "
            f"does not match observations length ({len(observations)})"
        )
    if not isinstance(lp["independent_mention_count"], int) or lp["independent_mention_count"] < 0:
        raise EvidenceError("latest_projection independent_mention_count must be a non-negative integer")
    if not isinstance(lp["self_authored_external_count"], int) or lp["self_authored_external_count"] < 0:
        raise EvidenceError("latest_projection self_authored_external_count must be a non-negative integer")
    gate_state = require_string(lp["launch_gate_state"], "latest_projection launch_gate_state")
    if gate_state not in LAUNCH_GATE_STATES:
        raise EvidenceError(f"latest_projection launch_gate_state {gate_state!r} is invalid")
    require_string(lp["current_state"], "latest_projection current_state")
    require_string(lp["owner_backlink_inventory"], "latest_projection owner_backlink_inventory")

    # Verify projection counts match observations.
    actual_independent = sum(
        1 for obs in observations
        if obs["classification"] in INDEPENDENT_CLASSIFICATIONS and obs["kind"] == "mention"
    )
    actual_self_authored = sum(
        1 for obs in observations
        if obs["classification"] == "self_authored_external" and obs["kind"] == "mention"
    )
    if lp["independent_mention_count"] != actual_independent:
        raise EvidenceError(
            f"latest_projection independent_mention_count ({lp['independent_mention_count']}) "
            f"does not match observations ({actual_independent})"
        )
    if lp["self_authored_external_count"] != actual_self_authored:
        raise EvidenceError(
            f"latest_projection self_authored_external_count ({lp['self_authored_external_count']}) "
            f"does not match observations ({actual_self_authored})"
        )
    if lp["launch_gate_state"] != launch_gate["state"]:
        raise EvidenceError(
            "latest_projection launch_gate_state disagrees with launch_gate state"
        )
    if observations:
        latest = observations[-1]
        if lp["latest_observed_at"] != latest["observed_at"]:
            raise EvidenceError(
                "latest_projection latest_observed_at disagrees with latest observation"
            )
    else:
        if lp["latest_observed_at"] is not None:
            raise EvidenceError(
                "latest_projection latest_observed_at must be null when no observations"
            )

    return lp


def normalize_whitespace(text: str) -> str:
    return re.sub(r"\s+", " ", text)


def check_forbidden_wording(text: str) -> None:
    normalized = normalize_whitespace(text).lower()
    for phrase in FORBIDDEN_WORDING:
        if phrase in normalized:
            raise EvidenceError(
                f"prose must not fabricate evidence or conflate classifications: "
                f"forbidden phrase {phrase!r}"
            )


def validate_bindings(
    text: str,
    phrases: list[tuple[str, str]],
    document_name: str,
) -> None:
    normalized = normalize_whitespace(text)
    for phrase, description in phrases:
        if normalize_whitespace(phrase) not in normalized:
            raise EvidenceError(
                f"{document_name} is missing bound phrase for {description!r}: "
                f"expected {phrase!r}"
            )


def validate(repository_root: Path) -> dict[str, Any]:
    artifact = load_json(repository_root / ARTIFACT_PATH)
    projection = validate_artifact(artifact)

    visibility_text = (repository_root / VISIBILITY_PATH).read_text()
    check_forbidden_wording(visibility_text)
    validate_bindings(visibility_text, VISIBILITY_BINDING_PHRASES, "SEARCH_VISIBILITY.md")

    roadmap_text = (repository_root / ROADMAP_PATH).read_text()
    check_forbidden_wording(roadmap_text)
    validate_bindings(roadmap_text, ROADMAP_BINDING_PHRASES, "ROADMAP.md")

    return projection


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Validate earned-authority evidence artifact and prose bindings"
    )
    parser.add_argument(
        "--repository-root",
        type=Path,
        default=Path(__file__).resolve().parent.parent,
        help="Path to the repository root (default: parent of this script)",
    )
    args = parser.parse_args()
    try:
        validate(args.repository_root.resolve())
    except (EvidenceError, OSError) as error:
        raise SystemExit(
            f"earned authority evidence audit failed: {error}"
        ) from error
    print("Earned authority evidence audit passed.")


if __name__ == "__main__":
    main()
