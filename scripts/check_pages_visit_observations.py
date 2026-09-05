#!/usr/bin/env python3
"""Validate the GitHub Pages visit observation artifact and its prose
bindings in SEARCH_VISIBILITY.md, WEBSITE.md, and ROADMAP.md.

The artifact ``docs/PAGES_VISIT_OBSERVATIONS.json`` is the structured
source of truth for owner-facing GitHub Pages visit analytics.  It is
separate from Google Search Console (Google Search clicks/impressions),
GitHub repository traffic (repository views/clones), and direct-provider
SERP/answer observations (engine-qualified visibility).  None of those
signals measures visits to the GitHub Pages site from Yandex, DuckDuckGo,
Perplexity, ChatGPT, other answer engines, ordinary referrals, or direct
navigation.

This validator checks that:

1. The artifact is well-formed, sanitized, and append-only.
2. The ``measurement_contract`` records the reporting timezone, canonical
   pages, primary conversion, source classes, collection-status
   vocabulary, data-minimization boundary, and separation rules.
3. Each observation records all required fields with valid values from
   the contract's vocabularies.
4. ``collection_status`` values of ``blocked``, ``delayed``,
   ``unauthorized``, ``provider_error``, or ``unknown`` are never
   treated as zero pageviews or zero visits.
5. The ``latest_projection`` agrees with the latest observation (or the
   empty template state when there are no observations).
6. The Pages-visit prose in ``SEARCH_VISIBILITY.md``, ``WEBSITE.md``,
   and ``ROADMAP.md`` all reference the artifact.
7. No prose fabricates visit data, equates GitHub repository views with
   Pages visits, equates Search Console clicks with all-provider visits,
   or attributes a visit to a query the evidence does not expose.
"""

from __future__ import annotations

import argparse
import json
from pathlib import Path
import re
import sys
from typing import Any


ARTIFACT_PATH = Path("docs") / "PAGES_VISIT_OBSERVATIONS.json"
VISIBILITY_PATH = Path("docs") / "SEARCH_VISIBILITY.md"
WEBSITE_PATH = Path("docs") / "WEBSITE.md"
ROADMAP_PATH = Path("docs") / "ROADMAP.md"

UTC_TIMESTAMP = re.compile(r"\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z\Z")
DATE_ONLY = re.compile(r"\d{4}-\d{2}-\d{2}")

ARTIFACT_KEYS = {
    "artifact_version",
    "provenance",
    "measurement_contract",
    "observations",
    "latest_projection",
}
PROVENANCE_KEYS = {
    "source",
    "sanitized",
    "collection_method",
    "collected_at",
    "sanitization_note",
}

MEASUREMENT_CONTRACT_KEYS = {
    "reporting_timezone",
    "canonical_pages",
    "primary_conversion",
    "source_classes",
    "collection_status_values",
    "data_minimization_boundary",
    "separation_rules",
}
PRIMARY_CONVERSION_KEYS = {"description", "target_url"}
DATA_MINIMIZATION_KEYS = {"forbidden_fields", "note"}

REQUIRED_CANONICAL_PAGES = {
    "https://rotnov.github.io/pycc/",
    "https://rotnov.github.io/pycc/status/",
    "https://rotnov.github.io/pycc/architecture/",
    "https://rotnov.github.io/pycc/python-aot-compilers/",
    "https://rotnov.github.io/pycc/ai-native/",
        "https://rotnov.github.io/pycc/language-support/",
        "https://rotnov.github.io/pycc/diagnostics/",
}

REQUIRED_SOURCE_CLASSES = {
    "google",
    "yandex",
    "bing",
    "duckduckgo",
    "perplexity",
    "chatgpt",
    "other_answer_engine",
    "other_search",
    "referral",
    "direct_or_unattributed",
    "unknown",
}

REQUIRED_COLLECTION_STATUS_VALUES = {
    "available",
    "delayed",
    "blocked",
    "unauthorized",
    "provider_error",
    "unknown",
}

REQUIRED_FORBIDDEN_FIELDS = {
    "names",
    "email_or_account_identifiers",
    "form_contents",
    "full_ip_addresses",
    "full_user_agents",
    "cookies",
    "fingerprints",
    "persistent_cross_site_ids",
    "session_replay",
    "arbitrary_query_strings",
    "arbitrary_fragments",
    "raw_search_queries_not_from_provider_report",
}

OBSERVATION_KEYS = {
    "observed_at",
    "data_through_date",
    "reporting_timezone",
    "provider",
    "provider_integration_version",
    "collection_status",
    "pageviews",
    "visits",
    "uniques",
    "unique_definition",
    "per_page",
    "source_classes",
    "primary_conversion_clicks",
    "country",
    "device",
    "sampling_truncation_metadata",
    "freshness_note",
}

SAMPLING_METADATA_KEYS = {
    "sampled",
    "truncated",
    "blocker_loss_acknowledged",
    "row_limit",
}

LATEST_PROJECTION_KEYS = {
    "observation_count",
    "latest_observed_at",
    "collection_status",
    "total_pageviews",
    "total_visits",
    "total_uniques",
    "primary_conversion_clicks",
    "entry_pages_observed",
    "source_classes_observed",
}

# Collection statuses that must never be converted to zero.
NON_ZEROABLE_STATUSES = {
    "blocked",
    "delayed",
    "unauthorized",
    "provider_error",
    "unknown",
}

# Wording that conflates different discovery signals with Pages visits.
FORBIDDEN_CONFLATION_WORDING = [
    "repository views are pages visits",
    "repository views are website visits",
    "github views are pages visits",
    "github views are website visits",
    "search console clicks are all visits",
    "search console clicks are all-provider visits",
    "gsc clicks are all visits",
    "gsc clicks are all-provider visits",
    "search console clicks are all organic traffic",
    "gsc clicks are all organic traffic",
    "repository views equal pages visits",
    "repository views equal website visits",
    "serp proves a visit",
    "serp result proves a visit",
    "citation proves a visit",
    "answer citation proves a visit",
    "indexed means visited",
    "crawlable means visited",
    "ranked means visited",
    "analytics is a ranking factor",
    "analytics caused visibility",
    "pageviews caused ranking",
    "visits caused ranking",
    "analytics proves ranking",
    "analytics is evidence that llms.txt",
    "analytics is evidence that schema",
    "analytics is evidence that a content edit",
]


class ObservationError(ValueError):
    """Raised when the Pages visit artifact or its prose projection is invalid."""


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


def validate_date(value: Any, description: str) -> str:
    if not isinstance(value, str) or not DATE_ONLY.fullmatch(value):
        raise ObservationError(f"{description} must use YYYY-MM-DD")
    return value


def validate_optional_int(value: Any, description: str) -> int | None:
    if value is None:
        return None
    if not isinstance(value, int) or isinstance(value, bool) or value < 0:
        raise ObservationError(f"{description} must be a non-negative integer or null")
    return value


def validate_measurement_contract(contract: Any) -> dict[str, Any]:
    if not isinstance(contract, dict) or set(contract) != MEASUREMENT_CONTRACT_KEYS:
        raise ObservationError("measurement_contract has unexpected fields")

    tz = require_string(contract["reporting_timezone"], "measurement_contract reporting_timezone")
    if tz != "UTC":
        raise ObservationError("measurement_contract reporting_timezone must be UTC")

    pages = contract["canonical_pages"]
    if not isinstance(pages, list) or set(pages) != REQUIRED_CANONICAL_PAGES:
        raise ObservationError(
            "measurement_contract canonical_pages must list exactly the required canonical URLs"
        )

    pc = contract["primary_conversion"]
    if not isinstance(pc, dict) or set(pc) != PRIMARY_CONVERSION_KEYS:
        raise ObservationError("measurement_contract primary_conversion has unexpected fields")
    require_string(pc["description"], "primary_conversion description")
    if pc["target_url"] != "https://github.com/rotnov/pycc":
        raise ObservationError(
            "primary_conversion target_url must be https://github.com/rotnov/pycc"
        )

    classes = contract["source_classes"]
    if not isinstance(classes, list) or set(classes) != REQUIRED_SOURCE_CLASSES:
        raise ObservationError(
            "measurement_contract source_classes must list exactly the required source classes"
        )

    statuses = contract["collection_status_values"]
    if not isinstance(statuses, list) or set(statuses) != REQUIRED_COLLECTION_STATUS_VALUES:
        raise ObservationError(
            "measurement_contract collection_status_values must list exactly the required values"
        )

    dmb = contract["data_minimization_boundary"]
    if not isinstance(dmb, dict) or set(dmb) != DATA_MINIMIZATION_KEYS:
        raise ObservationError("data_minimization_boundary has unexpected fields")
    forbidden = dmb["forbidden_fields"]
    if not isinstance(forbidden, list) or set(forbidden) != REQUIRED_FORBIDDEN_FIELDS:
        raise ObservationError(
            "data_minimization_boundary forbidden_fields must list exactly the required fields"
        )
    require_string(dmb["note"], "data_minimization_boundary note")

    rules = contract["separation_rules"]
    if not isinstance(rules, list) or len(rules) < 5:
        raise ObservationError(
            "measurement_contract separation_rules must have at least 5 rules"
        )
    for rule in rules:
        require_string(rule, "separation_rules entry")

    return contract


def validate_per_page(per_page: Any) -> list[dict[str, Any]]:
    if not isinstance(per_page, list):
        raise ObservationError("per_page must be a list")
    for entry in per_page:
        if not isinstance(entry, dict) or set(entry) != {"page", "pageviews", "visits"}:
            raise ObservationError("per_page entry has unexpected fields")
        require_string(entry["page"], "per_page page")
        validate_optional_int(entry["pageviews"], "per_page pageviews")
        validate_optional_int(entry["visits"], "per_page visits")
    return per_page


def validate_source_class_entries(entries: Any) -> list[dict[str, Any]]:
    if not isinstance(entries, list):
        raise ObservationError("source_classes must be a list")
    for entry in entries:
        if not isinstance(entry, dict) or set(entry) != {"source_class", "visits", "pageviews"}:
            raise ObservationError("source_classes entry has unexpected fields")
        sc = require_string(entry["source_class"], "source_class entry source_class")
        if sc not in REQUIRED_SOURCE_CLASSES:
            raise ObservationError(f"source_class {sc!r} is not in the allowed vocabulary")
        validate_optional_int(entry["visits"], "source_class entry visits")
        validate_optional_int(entry["pageviews"], "source_class entry pageviews")
    return entries


def validate_dimension_entries(entries: Any, dim_name: str, key: str) -> list[dict[str, Any]]:
    if not isinstance(entries, list):
        raise ObservationError(f"{dim_name} must be a list")
    for entry in entries:
        if not isinstance(entry, dict) or set(entry) != {key, "visits"}:
            raise ObservationError(f"{dim_name} entry has unexpected fields")
        require_string(entry[key], f"{dim_name} entry {key}")
        validate_optional_int(entry["visits"], f"{dim_name} entry visits")
    return entries


def validate_sampling_metadata(meta: Any) -> dict[str, Any]:
    if not isinstance(meta, dict) or set(meta) != SAMPLING_METADATA_KEYS:
        raise ObservationError("sampling_truncation_metadata has unexpected fields")
    require_bool(meta["sampled"], "sampling_truncation_metadata sampled")
    require_bool(meta["truncated"], "sampling_truncation_metadata truncated")
    require_bool(
        meta["blocker_loss_acknowledged"],
        "sampling_truncation_metadata blocker_loss_acknowledged",
    )
    if meta["row_limit"] is not None:
        if not isinstance(meta["row_limit"], int) or isinstance(meta["row_limit"], bool) or meta["row_limit"] < 0:
            raise ObservationError("sampling_truncation_metadata row_limit must be a non-negative integer or null")
    return meta


def validate_observation(obs: Any) -> dict[str, Any]:
    if not isinstance(obs, dict) or set(obs) != OBSERVATION_KEYS:
        raise ObservationError("observation has unexpected fields")
    ts = validate_timestamp(obs["observed_at"], "observation observed_at")
    validate_date(obs["data_through_date"], "observation data_through_date")
    require_string(obs["reporting_timezone"], "observation reporting_timezone")
    if obs["reporting_timezone"] != "UTC":
        raise ObservationError("observation reporting_timezone must be UTC")
    require_string(obs["provider"], "observation provider")
    require_string(obs["provider_integration_version"], "observation provider_integration_version")
    status = require_string(obs["collection_status"], "observation collection_status")
    if status not in REQUIRED_COLLECTION_STATUS_VALUES:
        raise ObservationError(f"collection_status {status!r} is invalid")

    pageviews = validate_optional_int(obs["pageviews"], "observation pageviews")
    visits = validate_optional_int(obs["visits"], "observation visits")
    uniques = validate_optional_int(obs["uniques"], "observation uniques")
    require_string(obs["unique_definition"], "observation unique_definition")

    # A non-zeroable status must not claim zero pageviews or zero visits.
    if status in NON_ZEROABLE_STATUSES:
        if pageviews == 0 or visits == 0:
            raise ObservationError(
                f"collection_status {status!r} must not be converted to zero; "
                f"use null instead of 0 for pageviews/visits"
            )

    validate_per_page(obs["per_page"])
    validate_source_class_entries(obs["source_classes"])
    validate_optional_int(obs["primary_conversion_clicks"], "observation primary_conversion_clicks")
    validate_dimension_entries(obs["country"], "country", "country")
    validate_dimension_entries(obs["device"], "device", "device")
    validate_sampling_metadata(obs["sampling_truncation_metadata"])
    require_string(obs["freshness_note"], "observation freshness_note")
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

    validate_measurement_contract(artifact["measurement_contract"])

    observations = artifact["observations"]
    if not isinstance(observations, list):
        raise ObservationError("observations must be a list")

    previous_ts: str | None = None
    for obs in observations:
        validated = validate_observation(obs)
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
    if not isinstance(lp["entry_pages_observed"], list):
        raise ObservationError("latest_projection entry_pages_observed must be a list")
    if not isinstance(lp["source_classes_observed"], list):
        raise ObservationError("latest_projection source_classes_observed must be a list")

    if observations:
        latest = observations[-1]
        if lp["latest_observed_at"] != latest["observed_at"]:
            raise ObservationError(
                "latest_projection latest_observed_at disagrees with latest observation"
            )
        if lp["collection_status"] != latest["collection_status"]:
            raise ObservationError(
                "latest_projection collection_status disagrees with latest observation"
            )
        if lp["total_pageviews"] != latest["pageviews"]:
            raise ObservationError(
                "latest_projection total_pageviews disagrees with latest observation"
            )
        if lp["total_visits"] != latest["visits"]:
            raise ObservationError(
                "latest_projection total_visits disagrees with latest observation"
            )
        if lp["total_uniques"] != latest["uniques"]:
            raise ObservationError(
                "latest_projection total_uniques disagrees with latest observation"
            )
        if lp["primary_conversion_clicks"] != latest["primary_conversion_clicks"]:
            raise ObservationError(
                "latest_projection primary_conversion_clicks disagrees with latest observation"
            )
        observed_sources = sorted({e["source_class"] for e in latest["source_classes"]})
        if sorted(lp["source_classes_observed"]) != observed_sources:
            raise ObservationError(
                "latest_projection source_classes_observed does not match latest observation"
            )
        observed_pages = sorted({e["page"] for e in latest["per_page"]})
        if sorted(lp["entry_pages_observed"]) != observed_pages:
            raise ObservationError(
                "latest_projection entry_pages_observed does not match latest observation"
            )
    else:
        if lp["latest_observed_at"] is not None:
            raise ObservationError(
                "latest_projection latest_observed_at must be null when no observations"
            )
        if lp["collection_status"] != "unknown":
            raise ObservationError(
                "latest_projection collection_status must be unknown when no observations"
            )
        if lp["total_pageviews"] is not None:
            raise ObservationError(
                "latest_projection total_pageviews must be null when no observations"
            )
        if lp["total_visits"] is not None:
            raise ObservationError(
                "latest_projection total_visits must be null when no observations"
            )
        if lp["total_uniques"] is not None:
            raise ObservationError(
                "latest_projection total_uniques must be null when no observations"
            )
        if lp["primary_conversion_clicks"] is not None:
            raise ObservationError(
                "latest_projection primary_conversion_clicks must be null when no observations"
            )
        if lp["entry_pages_observed"]:
            raise ObservationError(
                "latest_projection entry_pages_observed must be empty when no observations"
            )
        if lp["source_classes_observed"]:
            raise ObservationError(
                "latest_projection source_classes_observed must be empty when no observations"
            )

    return lp


def normalize_whitespace(text: str) -> str:
    return re.sub(r"\s+", " ", text)


def check_forbidden_conflation_wording(text: str) -> None:
    normalized = normalize_whitespace(text).lower()
    for phrase in FORBIDDEN_CONFLATION_WORDING:
        if phrase in normalized:
            raise ObservationError(
                f"prose must not conflate discovery signals with Pages visits: "
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
        ("PAGES_VISIT_OBSERVATIONS.json", "pages-visit artifact reference"),
        ("Pages visit", "pages-visit measurement distinction"),
        ("direct_or_unattributed", "direct_or_unattributed source class"),
        ("data_minimization_boundary", "data-minimization boundary"),
        ("primary_conversion", "primary conversion concept"),
    ]


def website_binding_phrases() -> list[tuple[str, str]]:
    return [
        ("PAGES_VISIT_OBSERVATIONS.json", "pages-visit artifact reference in website doc"),
        ("Pages visit", "pages-visit measurement distinction in website doc"),
    ]


def roadmap_binding_phrases() -> list[tuple[str, str]]:
    return [
        ("PAGES_VISIT_OBSERVATIONS.json", "pages-visit artifact reference in roadmap"),
        ("Pages visit", "pages-visit measurement distinction in roadmap"),
    ]


def validate(repository_root: Path) -> dict[str, Any]:
    artifact = load_json(repository_root / ARTIFACT_PATH)
    projection = validate_artifact(artifact)

    visibility_text = (repository_root / VISIBILITY_PATH).read_text()
    check_forbidden_conflation_wording(visibility_text)
    validate_bindings(visibility_text, visibility_binding_phrases(), "SEARCH_VISIBILITY.md")

    website_text = (repository_root / WEBSITE_PATH).read_text()
    check_forbidden_conflation_wording(website_text)
    validate_bindings(website_text, website_binding_phrases(), "WEBSITE.md")

    roadmap_text = (repository_root / ROADMAP_PATH).read_text()
    check_forbidden_conflation_wording(roadmap_text)
    validate_bindings(roadmap_text, roadmap_binding_phrases(), "ROADMAP.md")

    return projection


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Validate GitHub Pages visit observation artifact and prose bindings"
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
            f"pages visit observation audit failed: {error}"
        ) from error
    print("Pages visit observation audit passed.")


if __name__ == "__main__":
    main()
