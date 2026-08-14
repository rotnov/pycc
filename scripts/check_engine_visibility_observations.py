#!/usr/bin/env python3
"""Validate the engine-qualified visibility observation artifact and its
prose bindings in SEARCH_VISIBILITY.md and ROADMAP.md.

The artifact ``docs/ENGINE_VISIBILITY_OBSERVATIONS.json`` is the structured
source of truth for engine-qualified web-search and LLM answer-engine
visibility observations.  It is separate from indexability: it tracks whether
pycc appears in result sets or is cited in answers, not whether it is
crawlable or indexed.

This validator checks that:

1. The artifact is well-formed, sanitized, and append-only.
2. The ``query_suite`` covers the five required intent classes with stable
   IDs and correct KPI roles.
3. The ``surfaces`` map covers the five required engine surfaces with
   provider, transport, and result_window.
4. Each observation records all required fields with a valid outcome from
   the separate visibility vocabulary.
5. ``cited_in_answer`` outcomes have at least one captured cited URL.
6. ``crawlable`` and ``indexed_owner_visible`` are never treated as
   citation evidence.
7. The ``latest_projection`` agrees with the latest observation (or the
   empty template state when there are no observations).
8. The engine-visibility prose in ``SEARCH_VISIBILITY.md`` and the
   ``Public evidence and discoverability`` projection in ``docs/ROADMAP.md``
   both reference the artifact.
9. No prose fabricates visibility data or treats indexability as visibility.
"""

from __future__ import annotations

import argparse
import json
from pathlib import Path
import re
import sys
from typing import Any


ARTIFACT_PATH = Path("docs") / "ENGINE_VISIBILITY_OBSERVATIONS.json"
VISIBILITY_PATH = Path("docs") / "SEARCH_VISIBILITY.md"
ROADMAP_PATH = Path("docs") / "ROADMAP.md"

UTC_TIMESTAMP = re.compile(r"\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z\Z")

ARTIFACT_KEYS = {"artifact_version", "provenance", "query_suite", "surfaces", "observations", "latest_projection"}
PROVENANCE_KEYS = {"source", "sanitized", "collection_method", "collected_at", "sanitization_note"}

REQUIRED_QUERY_IDS = {
    "exact-entity-rotnov-pycc",
    "named-product-pycc-aot",
    "product-category-python-aot",
    "feature-intent-typed-native",
    "ai-authorship-diagnostic",
}
REQUIRED_INTENT_CLASSES = {
    "exact_entity",
    "named_product",
    "product_category",
    "feature_intent",
    "authorship_narrative",
}
REQUIRED_KPI_ROLES = {"diagnostic", "product_acquisition", "excluded"}
QUERY_SUITE_KEYS = {"id", "intent_class", "kpi_role", "prompt", "rationale"}

REQUIRED_SURFACE_IDS = {
    "chatgpt_search",
    "google_web_search",
    "bing_web_search",
    "perplexity_search",
    "unknown_web_provider",
}
SURFACE_KEYS = {"provider", "transport", "result_window"}

OUTCOMES = {
    "crawlable",
    "indexed_owner_visible",
    "returned_in_result_set",
    "cited_in_answer",
    "not_surfaced_in_observed_window",
    "unknown",
}
INDEXABILITY_OUTCOMES = {"crawlable", "indexed_owner_visible"}
VISIBILITY_OUTCOMES = {"returned_in_result_set", "cited_in_answer"}

OBSERVATION_KEYS = {
    "observed_at",
    "query_suite_id",
    "surface",
    "prompt",
    "model_product_version",
    "locale",
    "country",
    "personalization_state",
    "result_window",
    "outcome",
    "pycc_surfaced",
    "cited_urls",
    "citation_order",
    "competing_entities",
    "transport_tool",
    "query_rewriting_disclosed",
}

LATEST_PROJECTION_KEYS = {
    "observation_count",
    "surfaces_observed",
    "latest_observed_at",
    "pycc_surfaced_any",
    "pycc_cited_any",
}

QUERY_REWRITING_VALUES = {"yes", "no", "unknown"}

# Wording that conflates indexability with visibility or citation.
FORBIDDEN_INDEXABILITY_WORDING = [
    "crawlable means cited",
    "indexed means cited",
    "indexed means visible",
    "crawlable means visible",
    "indexability is visibility",
    "indexability is citation",
    "indexed owner visible means cited in answer",
    "crawlable proves citation",
    "indexed proves citation",
    "indexed proves visibility",
]


class ObservationError(ValueError):
    """Raised when the engine visibility artifact or its prose projection is invalid."""


def load_json(path: Path) -> dict[str, Any]:
    try:
        text = path.read_text()
    except OSError as error:
        raise ObservationError(f"could not read {path}: {error}") from error
    try:
        value = json.loads(text)
    except json.JSONDecodeError as error:
        raise ObservationError(f"could not parse JSON from {path}: {error}") from error
    if not isinstance(value, dict):
        raise ObservationError(f"expected a JSON object in {path}")
    return value


def require_string(value: Any, description: str) -> str:
    if not isinstance(value, str) or not value.strip():
        raise ObservationError(f"{description} must be a nonempty string")
    return value


def require_bool(value: Any, description: str) -> bool:
    if not isinstance(value, bool):
        raise ObservationError(f"{description} must be a boolean")
    return value


def validate_timestamp(value: Any, description: str) -> str:
    if not isinstance(value, str) or not UTC_TIMESTAMP.fullmatch(value):
        raise ObservationError(f"{description} must use YYYY-MM-DDTHH:MM:SSZ")
    return value


def validate_query_suite(queries: Any) -> dict[str, dict[str, Any]]:
    if not isinstance(queries, list) or len(queries) != len(REQUIRED_QUERY_IDS):
        raise ObservationError(
            f"query_suite must have exactly {len(REQUIRED_QUERY_IDS)} entries"
        )
    by_id: dict[str, dict[str, Any]] = {}
    for entry in queries:
        if not isinstance(entry, dict) or set(entry) != QUERY_SUITE_KEYS:
            raise ObservationError("query_suite entry has unexpected fields")
        qid = require_string(entry["id"], "query_suite id")
        if qid not in REQUIRED_QUERY_IDS:
            raise ObservationError(f"query_suite has unexpected id {qid!r}")
        if qid in by_id:
            raise ObservationError(f"query_suite id {qid!r} is duplicated")
        intent = require_string(entry["intent_class"], "query_suite intent_class")
        if intent not in REQUIRED_INTENT_CLASSES:
            raise ObservationError(f"query_suite intent_class {intent!r} is invalid")
        kpi = require_string(entry["kpi_role"], "query_suite kpi_role")
        if kpi not in REQUIRED_KPI_ROLES:
            raise ObservationError(f"query_suite kpi_role {kpi!r} is invalid")
        require_string(entry["prompt"], "query_suite prompt")
        require_string(entry["rationale"], "query_suite rationale")
        by_id[qid] = entry
    missing = REQUIRED_QUERY_IDS - set(by_id)
    if missing:
        raise ObservationError(f"query_suite is missing required ids: {missing}")
    # Validate KPI role assignments
    if by_id["exact-entity-rotnov-pycc"]["kpi_role"] != "diagnostic":
        raise ObservationError("exact entity query must have kpi_role diagnostic")
    if by_id["ai-authorship-diagnostic"]["kpi_role"] != "excluded":
        raise ObservationError("authorship diagnostic query must have kpi_role excluded")
    for product_id in ("named-product-pycc-aot", "product-category-python-aot", "feature-intent-typed-native"):
        if by_id[product_id]["kpi_role"] != "product_acquisition":
            raise ObservationError(f"{product_id} must have kpi_role product_acquisition")
    return by_id


def validate_surfaces(surfaces: Any) -> dict[str, dict[str, Any]]:
    if not isinstance(surfaces, dict) or set(surfaces) != REQUIRED_SURFACE_IDS:
        raise ObservationError(
            f"surfaces must have exactly the required ids: {REQUIRED_SURFACE_IDS}"
        )
    for sid, surface in surfaces.items():
        if not isinstance(surface, dict) or set(surface) != SURFACE_KEYS:
            raise ObservationError(f"surface {sid!r} has unexpected fields")
        require_string(surface["provider"], f"surface {sid} provider")
        require_string(surface["transport"], f"surface {sid} transport")
        require_string(surface["result_window"], f"surface {sid} result_window")
    return surfaces


def validate_observation(
    obs: Any,
    query_by_id: dict[str, dict[str, Any]],
    surfaces: dict[str, dict[str, Any]],
) -> dict[str, Any]:
    if not isinstance(obs, dict) or set(obs) != OBSERVATION_KEYS:
        raise ObservationError("observation has unexpected fields")
    ts = validate_timestamp(obs["observed_at"], "observation observed_at")
    qsid = require_string(obs["query_suite_id"], "observation query_suite_id")
    if qsid not in query_by_id:
        raise ObservationError(f"observation references unknown query_suite_id {qsid!r}")
    surface = require_string(obs["surface"], "observation surface")
    if surface not in surfaces:
        raise ObservationError(f"observation references unknown surface {surface!r}")
    require_string(obs["prompt"], "observation prompt")
    require_string(obs["model_product_version"], "observation model_product_version")
    require_string(obs["locale"], "observation locale")
    require_string(obs["country"], "observation country")
    require_string(obs["personalization_state"], "observation personalization_state")
    require_string(obs["result_window"], "observation result_window")
    outcome = require_string(obs["outcome"], "observation outcome")
    if outcome not in OUTCOMES:
        raise ObservationError(f"observation outcome {outcome!r} is invalid")
    if not require_bool(obs["pycc_surfaced"], "observation pycc_surfaced"):
        pass
    cited_urls = obs["cited_urls"]
    if not isinstance(cited_urls, list):
        raise ObservationError("observation cited_urls must be a list")
    for url in cited_urls:
        if not isinstance(url, str) or not url.strip():
            raise ObservationError("observation cited_urls entries must be nonempty strings")
    if outcome == "cited_in_answer" and not cited_urls:
        raise ObservationError(
            "cited_in_answer outcome requires at least one captured cited URL"
        )
    if outcome in INDEXABILITY_OUTCOMES and cited_urls:
        raise ObservationError(
            f"indexability outcome {outcome!r} must not carry cited URLs"
        )
    citation_order = obs["citation_order"]
    if citation_order is not None:
        if not isinstance(citation_order, int) or isinstance(citation_order, bool) or citation_order < 0:
            raise ObservationError("observation citation_order must be a non-negative integer or null")
    if outcome == "cited_in_answer" and citation_order is None:
        raise ObservationError(
            "cited_in_answer outcome must record citation_order"
        )
    competing = obs["competing_entities"]
    if not isinstance(competing, list):
        raise ObservationError("observation competing_entities must be a list")
    for entity in competing:
        if not isinstance(entity, str) or not entity.strip():
            raise ObservationError("observation competing_entities entries must be nonempty strings")
    require_string(obs["transport_tool"], "observation transport_tool")
    rewriting = require_string(obs["query_rewriting_disclosed"], "observation query_rewriting_disclosed")
    if rewriting not in QUERY_REWRITING_VALUES:
        raise ObservationError(f"query_rewriting_disclosed must be one of {QUERY_REWRITING_VALUES}")
    return obs


def validate_artifact(artifact: dict[str, Any]) -> dict[str, Any]:
    if set(artifact) != ARTIFACT_KEYS:
        raise ObservationError("artifact has unexpected top-level fields")
    if not isinstance(artifact["artifact_version"], int) or artifact["artifact_version"] != 1:
        raise ObservationError("artifact_version must be 1")

    provenance = artifact["provenance"]
    if not isinstance(provenance, dict) or set(provenance) != PROVENANCE_KEYS:
        raise ObservationError("provenance has unexpected fields")
    require_string(provenance["source"], "provenance source")
    if not require_bool(provenance["sanitized"], "provenance sanitized"):
        raise ObservationError("provenance must declare sanitized as true")
    require_string(provenance["collection_method"], "provenance collection_method")
    validate_timestamp(provenance["collected_at"], "provenance collected_at")
    require_string(provenance["sanitization_note"], "provenance sanitization_note")

    query_by_id = validate_query_suite(artifact["query_suite"])
    surfaces = validate_surfaces(artifact["surfaces"])

    observations = artifact["observations"]
    if not isinstance(observations, list):
        raise ObservationError("observations must be a list")

    previous_ts: str | None = None
    for obs in observations:
        validated = validate_observation(obs, query_by_id, surfaces)
        ts = validated["observed_at"]
        if previous_ts is not None and ts < previous_ts:
            raise ObservationError(
                "observation timestamps must be nondecreasing (append-only)"
            )
        previous_ts = ts

    latest_projection = artifact["latest_projection"]
    if not isinstance(latest_projection, dict) or set(latest_projection) != LATEST_PROJECTION_KEYS:
        raise ObservationError("latest_projection has unexpected fields")

    lp = latest_projection
    if not isinstance(lp["observation_count"], int) or lp["observation_count"] < 0:
        raise ObservationError("latest_projection observation_count must be a non-negative integer")
    if lp["observation_count"] != len(observations):
        raise ObservationError(
            f"latest_projection observation_count ({lp['observation_count']}) "
            f"does not match observations length ({len(observations)})"
        )
    if not isinstance(lp["surfaces_observed"], list):
        raise ObservationError("latest_projection surfaces_observed must be a list")
    if not isinstance(lp["pycc_surfaced_any"], bool):
        raise ObservationError("latest_projection pycc_surfaced_any must be a boolean")
    if not isinstance(lp["pycc_cited_any"], bool):
        raise ObservationError("latest_projection pycc_cited_any must be a boolean")

    if observations:
        latest = observations[-1]
        if lp["latest_observed_at"] != latest["observed_at"]:
            raise ObservationError(
                "latest_projection latest_observed_at disagrees with latest observation"
            )
        observed_surfaces = sorted({obs["surface"] for obs in observations})
        if sorted(lp["surfaces_observed"]) != observed_surfaces:
            raise ObservationError(
                "latest_projection surfaces_observed does not match observed surfaces"
            )
        if lp["pycc_surfaced_any"] != any(obs["pycc_surfaced"] for obs in observations):
            raise ObservationError(
                "latest_projection pycc_surfaced_any disagrees with observations"
            )
        if lp["pycc_cited_any"] != any(
            obs["outcome"] == "cited_in_answer" for obs in observations
        ):
            raise ObservationError(
                "latest_projection pycc_cited_any disagrees with observations"
            )
    else:
        if lp["latest_observed_at"] is not None:
            raise ObservationError(
                "latest_projection latest_observed_at must be null when no observations"
            )
        if lp["surfaces_observed"]:
            raise ObservationError(
                "latest_projection surfaces_observed must be empty when no observations"
            )
        if lp["pycc_surfaced_any"] or lp["pycc_cited_any"]:
            raise ObservationError(
                "latest_projection must be false when no observations"
            )

    return lp


def normalize_whitespace(text: str) -> str:
    return re.sub(r"\s+", " ", text)


def check_forbidden_indexability_wording(text: str) -> None:
    normalized = normalize_whitespace(text).lower()
    for phrase in FORBIDDEN_INDEXABILITY_WORDING:
        if phrase in normalized:
            raise ObservationError(
                f"prose must not conflate indexability with visibility or citation: "
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
            raise ObservationError(
                f"{document_name} is missing bound phrase for {description!r}: "
                f"expected {phrase!r}"
            )


def visibility_binding_phrases() -> list[tuple[str, str]]:
    return [
        ("ENGINE_VISIBILITY_OBSERVATIONS.json", "engine-visibility artifact reference"),
        ("engine-qualified", "engine-qualified visibility distinction"),
        ("crawlable", "crawlable outcome vocabulary"),
        ("cited_in_answer", "cited_in_answer outcome vocabulary"),
        ("not_surfaced_in_observed_window", "not_surfaced outcome vocabulary"),
    ]


def roadmap_binding_phrases() -> list[tuple[str, str]]:
    return [
        ("ENGINE_VISIBILITY_OBSERVATIONS.json", "engine-visibility artifact reference in roadmap"),
        ("engine-qualified", "engine-qualified visibility distinction in roadmap"),
    ]


def validate(repository_root: Path) -> dict[str, Any]:
    artifact = load_json(repository_root / ARTIFACT_PATH)
    projection = validate_artifact(artifact)

    visibility_text = (repository_root / VISIBILITY_PATH).read_text()
    check_forbidden_indexability_wording(visibility_text)
    vis_phrases = visibility_binding_phrases()
    validate_bindings(visibility_text, vis_phrases, "SEARCH_VISIBILITY.md")

    roadmap_text = (repository_root / ROADMAP_PATH).read_text()
    check_forbidden_indexability_wording(roadmap_text)
    road_phrases = roadmap_binding_phrases()
    validate_bindings(roadmap_text, road_phrases, "ROADMAP.md")

    return projection


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Validate engine-qualified visibility observation artifact and prose bindings"
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
    except (ObservationError, OSError) as error:
        raise SystemExit(
            f"engine visibility observation audit failed: {error}"
        ) from error
    print("Engine visibility observation audit passed.")


if __name__ == "__main__":
    main()
