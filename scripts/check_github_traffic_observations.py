#!/usr/bin/env python3
"""Bind GitHub Traffic API prose in SEARCH_VISIBILITY.md and ROADMAP.md to
a sanitized immutable daily traffic observation artifact.

The artifact ``docs/GITHUB_TRAFFIC_OBSERVATIONS.json`` is the structured source
of truth for owner-only GitHub Traffic API observations.  GitHub's traffic
endpoints return both a rolling 14-day aggregate total and a per-day
breakdown; this artifact preserves both so a future monitor can reconcile a
daily ``count`` series to the endpoint total without subtracting incompatible
rolling windows.

This validator checks that:

1. The artifact is well-formed, sanitized, and append-only.
2. Each observation records ``observed_at``, API version, data-through date,
   endpoint totals for views and clones (``count`` and ``uniques`` separate),
   daily rows, referrers, popular paths, and repository stars/forks/watchers.
3. Unavailable or unauthorized data is marked ``unknown`` or
   ``not_collected``, never silently zero.
4. For observations that preserve daily rows (``daily_and_aggregate``
   collection status), the sum of daily ``count`` values equals the endpoint
   ``count``.
5. Daily ``uniques`` are never summed to manufacture the rolling ``uniques``
   value — that would encode the same attribution error into CI.
6. The GitHub traffic history table in ``docs/SEARCH_VISIBILITY.md`` has one
   row per artifact observation, identified by timestamp.
7. The traffic interpretation prose in ``SEARCH_VISIBILITY.md`` and the
   ``Public evidence and discoverability`` projection in ``docs/ROADMAP.md``
   both reflect the latest artifact observation.
8. No prose equates clones with humans, visits, clicks, or SEO acquisition.
"""

from __future__ import annotations

import argparse
import json
from pathlib import Path
import re
import sys
from typing import Any


ARTIFACT_PATH = Path("docs") / "GITHUB_TRAFFIC_OBSERVATIONS.json"
VISIBILITY_PATH = Path("docs") / "SEARCH_VISIBILITY.md"
ROADMAP_PATH = Path("docs") / "ROADMAP.md"

UTC_TIMESTAMP = re.compile(r"\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z\Z")
UTC_DATE = re.compile(r"\d{4}-\d{2}-\d{2}\Z")
ARTIFACT_KEYS = {"artifact_version", "provenance", "observations", "latest_projection"}
PROVENANCE_KEYS = {
    "source",
    "sanitized",
    "collection_method",
    "collected_at",
    "sanitization_note",
    "api_version",
    "api_documentation",
}
OBSERVATION_KEYS = {
    "observed_at",
    "api_version",
    "data_through",
    "views",
    "clones",
    "referrers",
    "referrers_collection_status",
    "popular_paths",
    "popular_paths_collection_status",
    "repository_state",
    "interpretation_label",
}
TRAFFIC_SECTION_KEYS = {"count", "uniques", "daily_rows", "collection_status"}
REPOSITORY_STATE_KEYS = {"stars", "forks", "watchers"}
LATEST_PROJECTION_KEYS = {
    "observed_at",
    "data_through",
    "views_count",
    "views_uniques",
    "clones_count",
    "clones_uniques",
    "stars",
    "forks",
    "watchers",
    "has_daily_rows",
    "interpretation_label",
}
DAILY_ROW_KEYS = {"timestamp", "count", "uniques"}
REFERRER_KEYS = {"referrer", "count", "uniques"}
POPULAR_PATH_KEYS = {"path", "count", "uniques"}
COLLECTION_STATUSES = {"aggregate_only", "daily_and_aggregate", "unknown", "not_collected"}
REFERRER_COLLECTION_STATUSES = {"collected", "not_collected", "unknown"}
INTERPRETATION_LABELS = {
    "no activity",
    "automation-heavy / unattributed",
    "human-visible",
    "mixed",
}

# Wording that equates clones with humans, visits, clicks, or SEO acquisition.
# These phrases must never appear in the traffic interpretation prose.
FORBIDDEN_CLONE_WORDING = [
    "clones are people",
    "clones are visitors",
    "clones are humans",
    "clones are clicks",
    "clone count is human",
    "clone count is visitors",
    "clones prove seo",
    "clones prove search",
    "clones are search-driven",
    "clones are organic",
    "clone burst is human discovery",
    "clone burst is seo",
]


class ObservationError(ValueError):
    """Raised when the GitHub traffic artifact or its prose projection is invalid."""


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


def require_integer(value: Any, description: str) -> int:
    if not isinstance(value, int) or isinstance(value, bool):
        raise ObservationError(f"{description} must be an integer")
    if value < 0:
        raise ObservationError(f"{description} must be non-negative")
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
    if not isinstance(value, str) or not UTC_DATE.fullmatch(value):
        raise ObservationError(f"{description} must use YYYY-MM-DD")
    return value


def validate_traffic_section(
    section: Any, name: str
) -> dict[str, Any]:
    """Validate a views or clones section with count, uniques, daily_rows."""
    if not isinstance(section, dict) or set(section) != TRAFFIC_SECTION_KEYS:
        raise ObservationError(f"{name} has unexpected fields")
    require_integer(section["count"], f"{name} count")
    require_integer(section["uniques"], f"{name} uniques")
    status = section["collection_status"]
    if status not in COLLECTION_STATUSES:
        raise ObservationError(f"{name} collection_status is invalid")
    daily_rows = section["daily_rows"]
    if not isinstance(daily_rows, list):
        raise ObservationError(f"{name} daily_rows must be a list")

    if status == "daily_and_aggregate":
        if not daily_rows and section["count"] > 0:
            raise ObservationError(
                f"{name} has aggregate totals but no daily rows (daily_and_aggregate)"
            )
        seen_dates: set[str] = set()
        daily_count_sum = 0
        for row in daily_rows:
            if not isinstance(row, dict) or set(row) != DAILY_ROW_KEYS:
                raise ObservationError(f"{name} daily row has unexpected fields")
            validate_date(row["timestamp"], f"{name} daily row timestamp")
            if row["timestamp"] in seen_dates:
                raise ObservationError(
                    f"{name} daily row timestamp is duplicated"
                )
            seen_dates.add(row["timestamp"])
            require_integer(row["count"], f"{name} daily row count")
            require_integer(row["uniques"], f"{name} daily row uniques")
            daily_count_sum += row["count"]
        if daily_rows and daily_count_sum != section["count"]:
            raise ObservationError(
                f"{name} daily count sum ({daily_count_sum}) does not equal "
                f"endpoint count ({section['count']})"
            )
    elif status == "aggregate_only":
        if daily_rows:
            raise ObservationError(
                f"{name} is aggregate_only but has daily rows"
            )
    elif status == "unknown":
        if section["count"] != 0 or section["uniques"] != 0:
            raise ObservationError(
                f"{name} is unknown but has nonzero count or uniques"
            )
        if daily_rows:
            raise ObservationError(
                f"{name} is unknown but has daily rows"
            )

    return section


def validate_referrers(
    referrers: Any, status: Any
) -> list[dict[str, Any]]:
    if not isinstance(referrers, list):
        raise ObservationError("referrers must be a list")
    if status not in REFERRER_COLLECTION_STATUSES:
        raise ObservationError("referrers_collection_status is invalid")
    if status == "not_collected" and referrers:
        raise ObservationError(
            "referrers_collection_status is not_collected but referrers is nonempty"
        )
    for entry in referrers:
        if not isinstance(entry, dict) or set(entry) != REFERRER_KEYS:
            raise ObservationError("referrer entry has unexpected fields")
        require_string(entry["referrer"], "referrer entry referrer")
        require_integer(entry["count"], "referrer entry count")
        require_integer(entry["uniques"], "referrer entry uniques")
    return referrers


def validate_popular_paths(
    paths: Any, status: Any
) -> list[dict[str, Any]]:
    if not isinstance(paths, list):
        raise ObservationError("popular_paths must be a list")
    if status not in REFERRER_COLLECTION_STATUSES:
        raise ObservationError("popular_paths_collection_status is invalid")
    if status == "not_collected" and paths:
        raise ObservationError(
            "popular_paths_collection_status is not_collected but popular_paths is nonempty"
        )
    for entry in paths:
        if not isinstance(entry, dict) or set(entry) != POPULAR_PATH_KEYS:
            raise ObservationError("popular path entry has unexpected fields")
        require_string(entry["path"], "popular path entry path")
        require_integer(entry["count"], "popular path entry count")
        require_integer(entry["uniques"], "popular path entry uniques")
    return paths


def validate_artifact(artifact: dict[str, Any]) -> dict[str, Any]:
    """Validate the structured artifact and return the latest projection."""
    if set(artifact) != ARTIFACT_KEYS:
        raise ObservationError("artifact has unexpected top-level fields")
    if require_integer(artifact["artifact_version"], "artifact_version") != 1:
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
    require_string(provenance["api_version"], "provenance api_version")
    require_string(provenance["api_documentation"], "provenance api_documentation")

    observations = artifact["observations"]
    if not isinstance(observations, list) or not observations:
        raise ObservationError("observations must be a nonempty list")

    previous_ts: str | None = None
    for obs in observations:
        if not isinstance(obs, dict) or set(obs) != OBSERVATION_KEYS:
            raise ObservationError("observation has unexpected fields")
        ts = validate_timestamp(obs["observed_at"], "observation observed_at")
        if previous_ts is not None and ts < previous_ts:
            raise ObservationError(
                "observation timestamps must be nondecreasing (append-only)"
            )
        previous_ts = ts

        require_string(obs["api_version"], "observation api_version")
        validate_date(obs["data_through"], "observation data_through")

        validate_traffic_section(obs["views"], "views")
        validate_traffic_section(obs["clones"], "clones")

        validate_referrers(obs["referrers"], obs["referrers_collection_status"])
        validate_popular_paths(
            obs["popular_paths"], obs["popular_paths_collection_status"]
        )

        repo_state = obs["repository_state"]
        if not isinstance(repo_state, dict) or set(repo_state) != REPOSITORY_STATE_KEYS:
            raise ObservationError("repository_state has unexpected fields")
        require_integer(repo_state["stars"], "repository_state stars")
        require_integer(repo_state["forks"], "repository_state forks")
        require_integer(repo_state["watchers"], "repository_state watchers")

        label = obs["interpretation_label"]
        if label not in INTERPRETATION_LABELS:
            raise ObservationError("interpretation_label is invalid")

    latest_projection = artifact["latest_projection"]
    if not isinstance(latest_projection, dict) or set(latest_projection) != LATEST_PROJECTION_KEYS:
        raise ObservationError("latest_projection has unexpected fields")

    latest = observations[-1]
    lp = latest_projection
    if lp["observed_at"] != latest["observed_at"]:
        raise ObservationError("latest_projection observed_at disagrees with latest observation")
    if lp["data_through"] != latest["data_through"]:
        raise ObservationError("latest_projection data_through disagrees with latest observation")
    if lp["views_count"] != latest["views"]["count"]:
        raise ObservationError("latest_projection views_count disagrees with latest observation")
    if lp["views_uniques"] != latest["views"]["uniques"]:
        raise ObservationError("latest_projection views_uniques disagrees with latest observation")
    if lp["clones_count"] != latest["clones"]["count"]:
        raise ObservationError("latest_projection clones_count disagrees with latest observation")
    if lp["clones_uniques"] != latest["clones"]["uniques"]:
        raise ObservationError("latest_projection clones_uniques disagrees with latest observation")
    if lp["stars"] != latest["repository_state"]["stars"]:
        raise ObservationError("latest_projection stars disagrees with latest observation")
    if lp["forks"] != latest["repository_state"]["forks"]:
        raise ObservationError("latest_projection forks disagrees with latest observation")
    if lp["watchers"] != latest["repository_state"]["watchers"]:
        raise ObservationError("latest_projection watchers disagrees with latest observation")
    expected_has_daily = latest["views"]["collection_status"] == "daily_and_aggregate" or (
        latest["clones"]["collection_status"] == "daily_and_aggregate"
    )
    if lp["has_daily_rows"] != expected_has_daily:
        raise ObservationError("latest_projection has_daily_rows disagrees with latest observation")
    if lp["interpretation_label"] != latest["interpretation_label"]:
        raise ObservationError(
            "latest_projection interpretation_label disagrees with latest observation"
        )

    return lp


def traffic_history_timestamps(markdown: str) -> list[str]:
    """Extract timestamps from the GitHub traffic history table."""
    lines = markdown.replace("\r\n", "\n").replace("\r", "\n").split("\n")
    in_section = False
    saw_header = False
    saw_delimiter = False
    timestamps: list[str] = []
    for line in lines:
        stripped = line.strip()
        if stripped.startswith("## ") and "GitHub traffic history" in stripped:
            in_section = True
            continue
        if in_section and stripped.startswith("## ") and "GitHub traffic history" not in stripped:
            break
        if not in_section:
            continue
        if not stripped.startswith("|"):
            continue
        cells = [cell.strip() for cell in stripped[1:-1].split("|")] if stripped.endswith("|") else []
        if not cells:
            continue
        if cells[0] == "Collected at (UTC)":
            saw_header = True
            continue
        if saw_header and not saw_delimiter and all(re.fullmatch(r":?-+:?", c) for c in cells):
            saw_delimiter = True
            continue
        if not saw_header or not saw_delimiter:
            continue
        if len(cells) >= 4 and UTC_TIMESTAMP.fullmatch(cells[0]):
            timestamps.append(cells[0])
    if not saw_header or not saw_delimiter:
        raise ObservationError("GitHub traffic history table is missing or incomplete")
    if not timestamps:
        raise ObservationError("GitHub traffic history table has no data rows")
    return timestamps


def normalize_whitespace(text: str) -> str:
    """Collapse runs of whitespace to single spaces for phrase matching."""
    return re.sub(r"\s+", " ", text)


def check_forbidden_clone_wording(text: str) -> None:
    """Reject prose that equates clones with humans, visits, clicks, or SEO."""
    normalized = normalize_whitespace(text).lower()
    for phrase in FORBIDDEN_CLONE_WORDING:
        if phrase in normalized:
            raise ObservationError(
                f"traffic prose must not equate clones with humans, visits, "
                f"clicks, or SEO acquisition: forbidden phrase {phrase!r}"
            )


def traffic_interpretation_section(markdown: str) -> str:
    """Extract the traffic interpretation prose from the Current interpretation
    section — the paragraph(s) after the search projection table that mention
    traffic or clones."""
    lines = markdown.replace("\r\n", "\n").replace("\r", "\n").split("\n")
    in_section = False
    section_lines: list[str] = []
    for line in lines:
        stripped = line.strip()
        if stripped.startswith("## "):
            if in_section:
                break
            if "Current interpretation" in stripped:
                in_section = True
                continue
        if in_section:
            section_lines.append(line)
    if not in_section:
        raise ObservationError("SEARCH_VISIBILITY.md missing 'Current interpretation' section")
    return "\n".join(section_lines)


def validate_bindings(
    text: str,
    phrases: list[tuple[str, str]],
    document_name: str,
) -> None:
    """Check that every binding phrase appears in the text."""
    normalized = normalize_whitespace(text)
    for phrase, description in phrases:
        if normalize_whitespace(phrase) not in normalized:
            raise ObservationError(
                f"{document_name} is missing bound phrase for {description!r}: "
                f"expected {phrase!r}"
            )


def visibility_binding_phrases(projection: dict[str, Any]) -> list[tuple[str, str]]:
    """Generate (phrase, description) pairs for SEARCH_VISIBILITY.md traffic prose."""
    phrases: list[tuple[str, str]] = []
    label = projection["interpretation_label"]
    if label == "automation-heavy / unattributed":
        phrases.append((
            "automation-heavy",
            "traffic interpretation label in Current interpretation",
        ))
    if projection["clones_count"] > 0:
        phrases.append((
            "not treated as human discovery",
            "clone interpretation in Current interpretation",
        ))
    return phrases


def roadmap_binding_phrases(projection: dict[str, Any]) -> list[tuple[str, str]]:
    """Generate (phrase, description) pairs for ROADMAP.md traffic projection."""
    phrases: list[tuple[str, str]] = []
    label = projection["interpretation_label"]
    if label == "automation-heavy / unattributed":
        phrases.append((
            "automation-heavy",
            "traffic interpretation label in roadmap",
        ))
    phrases.append((
        "GITHUB_TRAFFIC_OBSERVATIONS.json",
        "traffic artifact reference in roadmap",
    ))
    return phrases


def validate(repository_root: Path) -> dict[str, Any]:
    """Validate the GitHub traffic artifact and its prose projections."""
    artifact = load_json(repository_root / ARTIFACT_PATH)
    projection = validate_artifact(artifact)

    visibility_text = (repository_root / VISIBILITY_PATH).read_text()
    history_ts = traffic_history_timestamps(visibility_text)
    artifact_ts = [obs["observed_at"] for obs in artifact["observations"]]
    if history_ts != artifact_ts:
        raise ObservationError(
            "GitHub traffic history table timestamps do not match "
            "the artifact observations"
        )

    interpretation = traffic_interpretation_section(visibility_text)
    check_forbidden_clone_wording(interpretation)
    vis_phrases = visibility_binding_phrases(projection)
    validate_bindings(interpretation, vis_phrases, "SEARCH_VISIBILITY.md Current interpretation")

    roadmap_text = (repository_root / ROADMAP_PATH).read_text()
    check_forbidden_clone_wording(roadmap_text)
    road_phrases = roadmap_binding_phrases(projection)
    validate_bindings(roadmap_text, road_phrases, "ROADMAP.md")

    return projection


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Validate GitHub traffic observation artifact and prose bindings"
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
            f"github traffic observation audit failed: {error}"
        ) from error
    print("GitHub traffic observation audit passed.")


if __name__ == "__main__":
    main()
