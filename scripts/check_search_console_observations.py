#!/usr/bin/env python3
"""Bind Google Search Console prose in SEARCH_VISIBILITY.md and ROADMAP.md to
a sanitized immutable observation artifact.

The artifact ``docs/SEARCH_CONSOLE_OBSERVATIONS.json`` is the structured source
of truth for owner-only Google Search Console observations.  This validator
checks that:

1. The artifact is well-formed, sanitized, and append-only.
2. The Google Search Console history table in ``docs/SEARCH_VISIBILITY.md``
   has one row per artifact observation, identified by timestamp.
3. The ``Current interpretation`` prose in ``SEARCH_VISIBILITY.md`` projects
   the latest artifact observation.
4. The ``Public evidence and discoverability`` row in ``docs/ROADMAP.md``
   projects the same latest artifact observation.

A contradictory indexing status that survives every other check — for example,
changing ``URL is on Google`` to ``URL is not on Google`` in the ledger while
leaving the roadmap unchanged — is rejected here because both projections are
bound to the same immutable artifact.
"""

from __future__ import annotations

import argparse
import json
from pathlib import Path
import re
import sys
from typing import Any


ARTIFACT_PATH = Path("docs") / "SEARCH_CONSOLE_OBSERVATIONS.json"
VISIBILITY_PATH = Path("docs") / "SEARCH_VISIBILITY.md"
ROADMAP_PATH = Path("docs") / "ROADMAP.md"

UTC_TIMESTAMP = re.compile(r"\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z\Z")
SHA256 = re.compile(r"[0-9a-f]{64}\Z")
ARTIFACT_KEYS = {"artifact_version", "provenance", "observations", "latest_projection"}
PROVENANCE_KEYS = {
    "source",
    "sanitized",
    "collection_method",
    "collected_at",
    "sanitization_note",
}
URL_INSPECTION_KEYS = {"status", "canonical_urls_on_google", "canonical_urls_total"}
SITEMAP_KEYS = {
    "public_http_status",
    "content_type",
    "canonical_urls_count",
    "search_console_status",
    "discovered_pages",
}
PERFORMANCE_KEYS = {
    "status",
    "clicks",
    "impressions",
    "ctr_percent",
    "avg_position",
    "query_rows",
}
QUERY_ROW_KEYS = {"query", "clicks", "impressions", "avg_position"}
LATEST_PROJECTION_KEYS = {
    "url_inspection_status",
    "canonical_urls_on_google",
    "canonical_urls_total",
    "sitemap_search_console_status",
    "sitemap_discovered_pages",
    "performance_impressions",
    "performance_clicks",
    "performance_ctr_percent",
    "performance_avg_position",
    "disclosed_query",
    "disclosed_query_clicks",
    "disclosed_query_impressions",
    "disclosed_query_avg_position",
}
URL_INSPECTION_STATUSES = {"all_on_google", "partial", "not_on_google"}
SITEMAP_SEARCH_CONSOLE_STATUSES = {
    "could_not_process",
    "could_not_fetch",
    "failed",
    "processed",
}
PERFORMANCE_STATUSES = {"processing", "available"}

NUMBER_WORDS = {
    0: "zero",
    1: "one",
    2: "two",
    3: "three",
    4: "four",
    5: "five",
    6: "six",
    7: "seven",
    8: "eight",
    9: "nine",
    10: "ten",
}


class ObservationError(ValueError):
    """Raised when the Search Console artifact or its prose projection is invalid."""


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
    return value


def require_string(value: Any, description: str) -> str:
    if not isinstance(value, str) or not value.strip():
        raise ObservationError(f"{description} must be a nonempty string")
    return value


def require_bool(value: Any, description: str) -> bool:
    if not isinstance(value, bool):
        raise ObservationError(f"{description} must be a boolean")
    return value


def optional_number(value: Any, description: str) -> float | int | None:
    if value is None:
        return None
    if isinstance(value, bool) or not isinstance(value, (int, float)):
        raise ObservationError(f"{description} must be a number or null")
    return value


def validate_timestamp(value: Any, description: str) -> str:
    if not isinstance(value, str) or not UTC_TIMESTAMP.fullmatch(value):
        raise ObservationError(f"{description} must use YYYY-MM-DDTHH:MM:SSZ")
    return value


def number_word(n: int) -> str:
    if n in NUMBER_WORDS:
        return NUMBER_WORDS[n]
    return str(n)


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

    observations = artifact["observations"]
    if not isinstance(observations, list) or not observations:
        raise ObservationError("observations must be a nonempty list")

    previous_ts: str | None = None
    observed_timestamps: list[str] = []
    for obs in observations:
        if not isinstance(obs, dict):
            raise ObservationError("each observation must be an object")
        ts = validate_timestamp(obs.get("observed_at"), "observation observed_at")
        if previous_ts is not None and ts < previous_ts:
            raise ObservationError(
                "observation timestamps must be nondecreasing (append-only)"
            )
        previous_ts = ts
        observed_timestamps.append(ts)

        url_inspection = obs.get("url_inspection")
        if not isinstance(url_inspection, dict) or set(url_inspection) < URL_INSPECTION_KEYS:
            extra = set(url_inspection) - URL_INSPECTION_KEYS - {"breadcrumbs_valid", "note"} if isinstance(url_inspection, dict) else set()
            if extra or not isinstance(url_inspection, dict):
                raise ObservationError("url_inspection has unexpected fields")
        if isinstance(url_inspection, dict):
            status = url_inspection.get("status")
            if status not in URL_INSPECTION_STATUSES:
                raise ObservationError("url_inspection status is invalid")
            require_integer(
                url_inspection.get("canonical_urls_on_google"),
                "url_inspection canonical_urls_on_google",
            )
            require_integer(
                url_inspection.get("canonical_urls_total"),
                "url_inspection canonical_urls_total",
            )

        sitemap = obs.get("sitemap")
        if not isinstance(sitemap, dict) or set(sitemap) != SITEMAP_KEYS:
            raise ObservationError("sitemap has unexpected fields")
        if isinstance(sitemap, dict):
            require_integer(sitemap.get("public_http_status"), "sitemap public_http_status")
            require_string(sitemap.get("content_type"), "sitemap content_type")
            require_integer(sitemap.get("canonical_urls_count"), "sitemap canonical_urls_count")
            sc_status = sitemap.get("search_console_status")
            if sc_status not in SITEMAP_SEARCH_CONSOLE_STATUSES:
                raise ObservationError("sitemap search_console_status is invalid")
            require_integer(sitemap.get("discovered_pages"), "sitemap discovered_pages")

        performance = obs.get("performance")
        if not isinstance(performance, dict) or set(performance) != PERFORMANCE_KEYS:
            raise ObservationError("performance has unexpected fields")
        if isinstance(performance, dict):
            perf_status = performance.get("status")
            if perf_status not in PERFORMANCE_STATUSES:
                raise ObservationError("performance status is invalid")
            optional_number(performance.get("clicks"), "performance clicks")
            optional_number(performance.get("impressions"), "performance impressions")
            optional_number(performance.get("ctr_percent"), "performance ctr_percent")
            optional_number(performance.get("avg_position"), "performance avg_position")
            query_rows = performance.get("query_rows")
            if not isinstance(query_rows, list):
                raise ObservationError("performance query_rows must be a list")
            for row in query_rows:
                if not isinstance(row, dict) or set(row) != QUERY_ROW_KEYS:
                    raise ObservationError("query row has unexpected fields")
                require_string(row.get("query"), "query row query")
                require_integer(row.get("clicks"), "query row clicks")
                require_integer(row.get("impressions"), "query row impressions")
                optional_number(row.get("avg_position"), "query row avg_position")

    latest_projection = artifact["latest_projection"]
    if not isinstance(latest_projection, dict) or set(latest_projection) != LATEST_PROJECTION_KEYS:
        raise ObservationError("latest_projection has unexpected fields")

    latest = observations[-1]
    lp = latest_projection
    if lp["url_inspection_status"] != latest["url_inspection"]["status"]:
        raise ObservationError("latest_projection url_inspection_status disagrees with latest observation")
    if lp["canonical_urls_on_google"] != latest["url_inspection"]["canonical_urls_on_google"]:
        raise ObservationError("latest_projection canonical_urls_on_google disagrees with latest observation")
    if lp["canonical_urls_total"] != latest["url_inspection"]["canonical_urls_total"]:
        raise ObservationError("latest_projection canonical_urls_total disagrees with latest observation")
    if lp["sitemap_search_console_status"] != latest["sitemap"]["search_console_status"]:
        raise ObservationError("latest_projection sitemap_search_console_status disagrees with latest observation")
    if lp["sitemap_discovered_pages"] != latest["sitemap"]["discovered_pages"]:
        raise ObservationError("latest_projection sitemap_discovered_pages disagrees with latest observation")
    latest_perf = latest["performance"]
    if lp["performance_impressions"] != latest_perf.get("impressions"):
        raise ObservationError("latest_projection performance_impressions disagrees with latest observation")
    if lp["performance_clicks"] != latest_perf.get("clicks"):
        raise ObservationError("latest_projection performance_clicks disagrees with latest observation")
    if lp["performance_ctr_percent"] != latest_perf.get("ctr_percent"):
        raise ObservationError("latest_projection performance_ctr_percent disagrees with latest observation")
    if lp["performance_avg_position"] != latest_perf.get("avg_position"):
        raise ObservationError("latest_projection performance_avg_position disagrees with latest observation")
    if latest_perf.get("query_rows"):
        first_row = latest_perf["query_rows"][0]
        if lp["disclosed_query"] != first_row["query"]:
            raise ObservationError("latest_projection disclosed_query disagrees with latest observation")
        if lp["disclosed_query_clicks"] != first_row["clicks"]:
            raise ObservationError("latest_projection disclosed_query_clicks disagrees with latest observation")
        if lp["disclosed_query_impressions"] != first_row["impressions"]:
            raise ObservationError("latest_projection disclosed_query_impressions disagrees with latest observation")
        if lp["disclosed_query_avg_position"] != first_row["avg_position"]:
            raise ObservationError("latest_projection disclosed_query_avg_position disagrees with latest observation")
    else:
        for key in (
            "disclosed_query",
            "disclosed_query_clicks",
            "disclosed_query_impressions",
            "disclosed_query_avg_position",
        ):
            if lp.get(key) is not None:
                raise ObservationError(
                    f"latest_projection {key} must be null when no query rows exist"
                )

    return lp


def search_console_history_timestamps(markdown: str) -> list[str]:
    """Extract timestamps from the Google Search Console history table."""
    lines = markdown.replace("\r\n", "\n").replace("\r", "\n").split("\n")
    in_section = False
    saw_header = False
    saw_delimiter = False
    timestamps: list[str] = []
    for line in lines:
        stripped = line.strip()
        if stripped.startswith("## ") and "Google Search Console history" in stripped:
            in_section = True
            continue
        if in_section and stripped.startswith("## ") and "Google Search Console history" not in stripped:
            break
        if not in_section:
            continue
        if not stripped.startswith("|"):
            continue
        cells = [cell.strip() for cell in stripped[1:-1].split("|")] if stripped.endswith("|") else []
        if not cells:
            continue
        if cells[0] == "Observed at (UTC)":
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
        raise ObservationError("Google Search Console history table is missing or incomplete")
    if not timestamps:
        raise ObservationError("Google Search Console history table has no data rows")
    return timestamps


def visibility_binding_phrases(projection: dict[str, Any]) -> list[tuple[str, str]]:
    """Generate (phrase, description) pairs that must appear in SEARCH_VISIBILITY.md."""
    phrases: list[tuple[str, str]] = []
    total = projection["canonical_urls_total"]
    if projection["url_inspection_status"] == "all_on_google":
        phrases.append((
            f"{number_word(total)} canonical URLs",
            "URL inspection canonical count in Current interpretation",
        ))
        phrases.append((
            "positive URL Inspection evidence",
            "URL inspection status in Current interpretation",
        ))
    perf_impressions = projection["performance_impressions"]
    perf_clicks = projection["performance_clicks"]
    if perf_impressions is not None:
        phrases.append((
            f"{perf_impressions} impressions",
            "performance impressions in Current interpretation",
        ))
    if perf_clicks is not None:
        phrases.append((
            f"{perf_clicks} clicks",
            "performance clicks in Current interpretation",
        ))
    query = projection["disclosed_query"]
    if query is not None:
        phrases.append((query, "disclosed query in Current interpretation"))
    query_impressions = projection["disclosed_query_impressions"]
    if query_impressions is not None:
        phrases.append((
            f"{query_impressions} impressions",
            "disclosed query impressions in Current interpretation",
        ))
    query_clicks = projection["disclosed_query_clicks"]
    if query_clicks is not None:
        phrases.append((
            f"{query_clicks} clicks",
            "disclosed query clicks in Current interpretation",
        ))
    query_avg_pos = projection["disclosed_query_avg_position"]
    if query_avg_pos is not None:
        phrases.append((
            f"average position {query_avg_pos}",
            "disclosed query average position in Current interpretation",
        ))
    if projection["sitemap_search_console_status"] in ("failed", "could_not_fetch", "could_not_process"):
        phrases.append((
            "Sitemap processing remains unsuccessful",
            "sitemap status in Current interpretation",
        ))
    return phrases


def roadmap_binding_phrases(projection: dict[str, Any]) -> list[tuple[str, str]]:
    """Generate (phrase, description) pairs that must appear in ROADMAP.md."""
    phrases: list[tuple[str, str]] = []
    total = projection["canonical_urls_total"]
    if projection["url_inspection_status"] == "all_on_google":
        phrases.append((
            f"{number_word(total)} canonical website URLs",
            "URL inspection canonical count in roadmap",
        ))
        phrases.append((
            "positive Google URL Inspection evidence",
            "URL inspection status in roadmap",
        ))
    perf_impressions = projection["performance_impressions"]
    perf_clicks = projection["performance_clicks"]
    perf_ctr = projection["performance_ctr_percent"]
    perf_avg_pos = projection["performance_avg_position"]
    if perf_impressions is not None:
        phrases.append((
            f"{perf_impressions} impressions",
            "performance impressions in roadmap",
        ))
    if perf_clicks is not None:
        phrases.append((
            f"{perf_clicks} clicks",
            "performance clicks in roadmap",
        ))
    if perf_ctr is not None:
        phrases.append((
            f"{perf_ctr}% CTR",
            "performance CTR in roadmap",
        ))
    if perf_avg_pos is not None:
        phrases.append((
            f"average position {perf_avg_pos}",
            "performance average position in roadmap",
        ))
    query = projection["disclosed_query"]
    if query is not None:
        phrases.append((query, "disclosed query in roadmap"))
    query_impressions = projection["disclosed_query_impressions"]
    if query_impressions is not None:
        phrases.append((
            f"{query_impressions} impressions",
            "disclosed query impressions in roadmap",
        ))
    query_clicks = projection["disclosed_query_clicks"]
    if query_clicks is not None:
        phrases.append((
            f"{query_clicks} clicks",
            "disclosed query clicks in roadmap",
        ))
    query_avg_pos = projection["disclosed_query_avg_position"]
    if query_avg_pos is not None:
        phrases.append((
            f"average position {query_avg_pos}",
            "disclosed query average position in roadmap",
        ))
    if projection["sitemap_search_console_status"] in ("failed", "could_not_fetch", "could_not_process"):
        phrases.append((
            "unsuccessful sitemap processing",
            "sitemap status in roadmap",
        ))
    return phrases


def current_interpretation_section(markdown: str) -> str:
    """Extract the Current interpretation section from SEARCH_VISIBILITY.md."""
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


def normalize_whitespace(text: str) -> str:
    """Collapse runs of whitespace to single spaces for phrase matching."""
    return re.sub(r"\s+", " ", text)


def validate_bindings(
    text: str,
    phrases: list[tuple[str, str]],
    document_name: str,
) -> None:
    """Check that every binding phrase appears in the text.

    Whitespace is normalized so a phrase that wraps across lines in the
    Markdown source is still detected.
    """
    normalized = normalize_whitespace(text)
    for phrase, description in phrases:
        if normalize_whitespace(phrase) not in normalized:
            raise ObservationError(
                f"{document_name} is missing bound phrase for {description!r}: "
                f"expected {phrase!r}"
            )


def validate(
    repository_root: Path,
) -> dict[str, Any]:
    """Validate the Search Console artifact and its prose projections."""
    artifact = load_json(repository_root / ARTIFACT_PATH)
    projection = validate_artifact(artifact)

    visibility_text = (repository_root / VISIBILITY_PATH).read_text()
    history_ts = search_console_history_timestamps(visibility_text)
    artifact_ts = [obs["observed_at"] for obs in artifact["observations"]]
    if history_ts != artifact_ts:
        raise ObservationError(
            "Google Search Console history table timestamps do not match "
            "the artifact observations"
        )

    interpretation = current_interpretation_section(visibility_text)
    vis_phrases = visibility_binding_phrases(projection)
    validate_bindings(interpretation, vis_phrases, "SEARCH_VISIBILITY.md Current interpretation")

    roadmap_text = (repository_root / ROADMAP_PATH).read_text()
    road_phrases = roadmap_binding_phrases(projection)
    validate_bindings(roadmap_text, road_phrases, "ROADMAP.md")

    return projection


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Validate Google Search Console observation artifact and prose bindings"
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
            f"search console observation audit failed: {error}"
        ) from error
    print("Search console observation audit passed.")


if __name__ == "__main__":
    main()
