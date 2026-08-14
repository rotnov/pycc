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
ARTIFACT_KEYS = {
    "artifact_version",
    "provenance",
    "page_indexing_observations",
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
    "aggregation_type",
    "clicks",
    "impressions",
    "ctr_percent",
    "avg_position",
    "query_rows",
    "report_scope",
    "dimension_tables",
}
QUERY_ROW_KEYS = {"query", "clicks", "impressions", "avg_position"}
PAGE_INDEXING_OBSERVATION_KEYS = {
    "collected_at",
    "property",
    "transport",
    "locale",
    "page_scope",
    "sitemap_filter",
    "report_last_updated",
    "data_freshness",
    "totals",
    "filters",
    "reason_rows",
}
PAGE_INDEXING_TOTALS_KEYS = {"indexed", "not_indexed"}
PAGE_INDEXING_REASON_ROW_KEYS = {
    "reason",
    "source",
    "validation_state",
    "affected_pages",
    "first_detected",
    "examples",
    "row_limit",
    "example_limit",
    "sampled",
    "truncated",
}
PAGE_INDEXING_EXAMPLE_KEYS = {"url", "last_crawl"}
PAGE_INDEXING_DATA_FRESHNESS_STATES = {
    "fresh",
    "report_lag_or_unreconciled",
    "unknown",
}
PAGE_INDEXING_VALIDATION_STATES = {
    "not_started",
    "in_progress",
    "passed",
    "failed",
    "unknown",
}
DATE_ONLY = re.compile(r"\d{4}-\d{2}-\d{2}\Z")
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
    "performance_aggregation_type",
    "page_dimension_impressions_sum",
    "page_dimension_row_count",
    "page_dimension_missing_row_count",
    "search_appearance_state",
    "page_indexing_report_last_updated",
    "page_indexing_indexed",
    "page_indexing_not_indexed",
    "page_indexing_data_freshness",
    "page_indexing_reason",
    "page_indexing_example_url",
    "page_indexing_example_last_crawl",
}
URL_INSPECTION_STATUSES = {"all_on_google", "partial", "not_on_google"}
SITEMAP_SEARCH_CONSOLE_STATUSES = {
    "could_not_process",
    "could_not_fetch",
    "failed",
    "processed",
}
PERFORMANCE_STATUSES = {"processing", "available"}
PERFORMANCE_AGGREGATION_TYPES = {"property", "page", "device", "country", "date", "search_appearance"}
REPORT_SCOPE_KEYS = {
    "property",
    "search_type",
    "selected_date_range",
    "data_start_date",
    "data_through_date",
    "granularity",
    "filters",
    "transport",
    "locale",
    "freshness_note",
}
DIMENSION_TABLE_KEYS = {
    "dimension",
    "aggregation_type",
    "rows",
    "displayed_row_sum",
    "missing_rows",
    "row_limit",
    "export_method",
    "truncated",
    "anonymized_rows_withheld",
}
DIMENSION_TABLE_NAMES = {"page", "device", "country", "date", "search_appearance"}
SEARCH_APPEARANCE_STATES = {"no_data", "has_data"}
MISSING_ROW_SEMANTICS = {"not_returned_in_observed_table"}
DIMENSION_ROW_KEY_MAP = {
    "page": "page",
    "device": "device",
    "country": "country",
    "date": "date",
}

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


def validate_date_only(value: Any, description: str) -> str:
    if not isinstance(value, str) or not DATE_ONLY.fullmatch(value):
        raise ObservationError(f"{description} must use YYYY-MM-DD")
    return value


def optional_string(value: Any, description: str) -> str | None:
    if value is None:
        return None
    if not isinstance(value, str) or not value.strip():
        raise ObservationError(f"{description} must be a nonempty string or null")
    return value


def number_word(n: int) -> str:
    if n in NUMBER_WORDS:
        return NUMBER_WORDS[n]
    return str(n)


def validate_page_indexing_observations(
    page_indexing_observations: Any,
) -> list[dict[str, Any]]:
    """Validate the Page indexing aggregate observation series.

    The Page indexing report is a separate report-level dataset from per-URL
    inspection, sitemap processing, and performance. Each snapshot preserves
    the report scope, report last-updated date, totals, and reason rows
    independently. Snapshots are append-only: ``collected_at`` timestamps must
    be nondecreasing and earlier snapshots are never rewritten or deleted.
    """
    if not isinstance(page_indexing_observations, list) or not page_indexing_observations:
        raise ObservationError("page_indexing_observations must be a nonempty list")

    previous_ts: str | None = None
    for snap in page_indexing_observations:
        if not isinstance(snap, dict) or set(snap) != PAGE_INDEXING_OBSERVATION_KEYS:
            raise ObservationError("page_indexing observation has unexpected fields")
        ts = validate_timestamp(snap["collected_at"], "page_indexing collected_at")
        if previous_ts is not None and ts < previous_ts:
            raise ObservationError(
                "page_indexing collected_at timestamps must be nondecreasing (append-only)"
            )
        previous_ts = ts
        require_string(snap["property"], "page_indexing property")
        require_string(snap["transport"], "page_indexing transport")
        require_string(snap["locale"], "page_indexing locale")
        require_string(snap["page_scope"], "page_indexing page_scope")
        optional_string(snap["sitemap_filter"], "page_indexing sitemap_filter")
        validate_date_only(snap["report_last_updated"], "page_indexing report_last_updated")
        freshness = snap["data_freshness"]
        if freshness not in PAGE_INDEXING_DATA_FRESHNESS_STATES:
            raise ObservationError("page_indexing data_freshness is invalid")

        totals = snap["totals"]
        if not isinstance(totals, dict) or set(totals) != PAGE_INDEXING_TOTALS_KEYS:
            raise ObservationError("page_indexing totals has unexpected fields")
        require_integer(totals["indexed"], "page_indexing totals indexed")
        require_integer(totals["not_indexed"], "page_indexing totals not_indexed")

        filters = snap["filters"]
        if not isinstance(filters, list):
            raise ObservationError("page_indexing filters must be a list")

        reason_rows = snap["reason_rows"]
        if not isinstance(reason_rows, list):
            raise ObservationError("page_indexing reason_rows must be a list")
        for row in reason_rows:
            if not isinstance(row, dict) or set(row) != PAGE_INDEXING_REASON_ROW_KEYS:
                raise ObservationError("page_indexing reason row has unexpected fields")
            require_string(row["reason"], "page_indexing reason row reason")
            require_string(row["source"], "page_indexing reason row source")
            vstate = row["validation_state"]
            if vstate not in PAGE_INDEXING_VALIDATION_STATES:
                raise ObservationError("page_indexing reason row validation_state is invalid")
            require_integer(row["affected_pages"], "page_indexing reason row affected_pages")
            validate_date_only(row["first_detected"], "page_indexing reason row first_detected")
            examples = row["examples"]
            if not isinstance(examples, list) or not examples:
                raise ObservationError(
                    "page_indexing reason row examples must be a nonempty list"
                )
            for ex in examples:
                if not isinstance(ex, dict) or set(ex) != PAGE_INDEXING_EXAMPLE_KEYS:
                    raise ObservationError("page_indexing example has unexpected fields")
                require_string(ex["url"], "page_indexing example url")
                validate_date_only(ex["last_crawl"], "page_indexing example last_crawl")
            if row["row_limit"] is not None:
                require_integer(row["row_limit"], "page_indexing reason row row_limit")
            if row["example_limit"] is not None:
                require_integer(row["example_limit"], "page_indexing reason row example_limit")
            if not isinstance(row["sampled"], bool):
                raise ObservationError("page_indexing reason row sampled must be a boolean")
            if not isinstance(row["truncated"], bool):
                raise ObservationError("page_indexing reason row truncated must be a boolean")

    return page_indexing_observations



def validate_report_scope(scope: Any, description: str) -> None:
    """Validate a Search Console report scope object."""
    if not isinstance(scope, dict) or set(scope) != REPORT_SCOPE_KEYS:
        raise ObservationError(f"{description} has unexpected fields")
    require_string(scope["property"], f"{description} property")
    require_string(scope["search_type"], f"{description} search_type")
    require_string(scope["selected_date_range"], f"{description} selected_date_range")
    validate_date_only(scope["data_start_date"], f"{description} data_start_date")
    validate_date_only(scope["data_through_date"], f"{description} data_through_date")
    require_string(scope["granularity"], f"{description} granularity")
    require_string(scope["filters"], f"{description} filters")
    require_string(scope["transport"], f"{description} transport")
    require_string(scope["locale"], f"{description} locale")
    require_string(scope["freshness_note"], f"{description} freshness_note")


def validate_dimension_row(row: Any, dimension_name: str, description: str) -> None:
    """Validate a single row within a dimension table."""
    if not isinstance(row, dict):
        raise ObservationError(f"{description} row must be an object")
    row_key = DIMENSION_ROW_KEY_MAP[dimension_name]
    expected_keys = {row_key, "clicks", "impressions"}
    if set(row) != expected_keys:
        raise ObservationError(f"{description} row has unexpected fields")
    require_string(row[row_key], f"{description} row {row_key}")
    require_integer(row["clicks"], f"{description} row clicks")
    require_integer(row["impressions"], f"{description} row impressions")
    if dimension_name == "date":
        if not DATE_ONLY.fullmatch(row[row_key]):
            raise ObservationError(f"{description} row date must use YYYY-MM-DD")


def validate_dimension_table(table: Any, dimension_name: str, description: str) -> None:
    """Validate a single dimension table (page, device, country, date, search_appearance)."""
    if not isinstance(table, dict):
        raise ObservationError(f"{description} must be an object")
    allowed_keys = DIMENSION_TABLE_KEYS | (
        {"state"} if dimension_name == "search_appearance" else set()
    )
    if set(table) != allowed_keys:
        raise ObservationError(f"{description} has unexpected fields")
    if table["dimension"] != dimension_name:
        raise ObservationError(f"{description} dimension name mismatch")
    if table["aggregation_type"] != dimension_name:
        raise ObservationError(f"{description} aggregation_type mismatch")
    if not isinstance(table["rows"], list):
        raise ObservationError(f"{description} rows must be a list")
    if not isinstance(table["truncated"], bool):
        raise ObservationError(f"{description} truncated must be a boolean")
    if not isinstance(table["anonymized_rows_withheld"], bool):
        raise ObservationError(f"{description} anonymized_rows_withheld must be a boolean")
    require_string(table["export_method"], f"{description} export_method")

    # search_appearance uses a categorical "state" instead of numeric rows.
    if dimension_name == "search_appearance":
        if table["state"] not in SEARCH_APPEARANCE_STATES:
            raise ObservationError(f"{description} state is invalid")
        if table["state"] == "no_data":
            if table["rows"]:
                raise ObservationError(
                    f"{description} with state no_data must have empty rows"
                )
            if table["displayed_row_sum"] is not None:
                raise ObservationError(
                    f"{description} with state no_data must have null displayed_row_sum"
                )
        return

    # Non-search-appearance tables must have numeric rows and a displayed_row_sum.
    for row in table["rows"]:
        validate_dimension_row(row, dimension_name, description)

    row_sum = table["displayed_row_sum"]
    if not isinstance(row_sum, dict) or set(row_sum) != {"clicks", "impressions"}:
        raise ObservationError(f"{description} displayed_row_sum has unexpected fields")
    require_integer(row_sum["clicks"], f"{description} displayed_row_sum clicks")
    require_integer(row_sum["impressions"], f"{description} displayed_row_sum impressions")

    # Validate missing_rows semantics.
    missing_rows = table["missing_rows"]
    if not isinstance(missing_rows, list):
        raise ObservationError(f"{description} missing_rows must be a list")
    row_key = DIMENSION_ROW_KEY_MAP[dimension_name]
    for missing in missing_rows:
        if not isinstance(missing, dict) or set(missing) != {row_key, "semantics"}:
            raise ObservationError(f"{description} missing_rows entry has unexpected fields")
        require_string(missing[row_key], f"{description} missing_rows {row_key}")
        if missing["semantics"] not in MISSING_ROW_SEMANTICS:
            raise ObservationError(
                f"{description} missing_rows semantics must be not_returned_in_observed_table"
            )


def validate_dimension_tables(tables: Any, description: str) -> None:
    """Validate the dimension_tables object on a performance record.

    Each dimension table is an independent marginal observation.  The
    validator rejects synthetic multi-dimensional joins by requiring that
    each table carry exactly one dimension name and no cross-dimension keys.
    """
    if not isinstance(tables, dict) or set(tables) != DIMENSION_TABLE_NAMES:
        raise ObservationError(f"{description} has unexpected dimension tables")
    for name in DIMENSION_TABLE_NAMES:
        validate_dimension_table(tables[name], name, f"{description} {name}")


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

    page_indexing_observations = validate_page_indexing_observations(
        artifact["page_indexing_observations"]
    )

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
            agg_type = performance.get("aggregation_type")
            if agg_type is not None:
                if not isinstance(agg_type, str) or agg_type not in PERFORMANCE_AGGREGATION_TYPES:
                    raise ObservationError("performance aggregation_type is invalid")
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
            report_scope = performance.get("report_scope")
            if report_scope is not None:
                validate_report_scope(report_scope, "performance report_scope")
            dimension_tables = performance.get("dimension_tables")
            if dimension_tables is not None:
                validate_dimension_tables(dimension_tables, "performance dimension_tables")
                # The property aggregate must not be replaced by a dimension
                # sum.  Page-level impressions legitimately exceed property
                # impressions because Google counts each unique URL separately
                # in page aggregation while counting the property once in the
                # property aggregate.  Reject any artifact that overwrites the
                # property impressions with the page displayed_row_sum.
                if agg_type == "property" and performance.get("impressions") is not None:
                    page_sum = dimension_tables["page"]["displayed_row_sum"]["impressions"]
                    if performance["impressions"] == page_sum and page_sum != 0:
                        raise ObservationError(
                            "property impressions must not equal the page "
                            "displayed_row_sum — the property aggregate is a "
                            "different measure from the page-level sum"
                        )

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
    if lp["performance_aggregation_type"] != latest_perf.get("aggregation_type"):
        raise ObservationError("latest_projection performance_aggregation_type disagrees with latest observation")
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

    # --- Page indexing aggregate projection ---
    # The latest Page indexing aggregate snapshot is projected separately from
    # the per-URL inspection. The aggregate is a lagging report-level dataset;
    # it must never overwrite the newer per-URL inspection count.
    latest_pi = page_indexing_observations[-1]
    if lp["page_indexing_report_last_updated"] != latest_pi["report_last_updated"]:
        raise ObservationError(
            "latest_projection page_indexing_report_last_updated disagrees with "
            "latest page indexing observation"
        )
    if lp["page_indexing_indexed"] != latest_pi["totals"]["indexed"]:
        raise ObservationError(
            "latest_projection page_indexing_indexed disagrees with latest page indexing observation"
        )
    if lp["page_indexing_not_indexed"] != latest_pi["totals"]["not_indexed"]:
        raise ObservationError(
            "latest_projection page_indexing_not_indexed disagrees with latest page indexing observation"
        )
    if lp["page_indexing_data_freshness"] != latest_pi["data_freshness"]:
        raise ObservationError(
            "latest_projection page_indexing_data_freshness disagrees with latest page indexing observation"
        )
    if latest_pi["reason_rows"]:
        first_reason = latest_pi["reason_rows"][0]
        if lp["page_indexing_reason"] != first_reason["reason"]:
            raise ObservationError(
                "latest_projection page_indexing_reason disagrees with latest page indexing observation"
            )
        if first_reason["examples"]:
            first_example = first_reason["examples"][0]
            if lp["page_indexing_example_url"] != first_example["url"]:
                raise ObservationError(
                    "latest_projection page_indexing_example_url disagrees with latest page indexing observation"
                )
            if lp["page_indexing_example_last_crawl"] != first_example["last_crawl"]:
                raise ObservationError(
                    "latest_projection page_indexing_example_last_crawl disagrees with latest page indexing observation"
                )
        else:
            if lp["page_indexing_example_url"] is not None:
                raise ObservationError(
                    "latest_projection page_indexing_example_url must be null when no examples exist"
                )
            if lp["page_indexing_example_last_crawl"] is not None:
                raise ObservationError(
                    "latest_projection page_indexing_example_last_crawl must be null when no examples exist"
                )
    else:
        if lp["page_indexing_reason"] is not None:
            raise ObservationError(
                "latest_projection page_indexing_reason must be null when no reason rows exist"
            )
        if lp["page_indexing_example_url"] is not None:
            raise ObservationError(
                "latest_projection page_indexing_example_url must be null when no reason rows exist"
            )
        if lp["page_indexing_example_last_crawl"] is not None:
            raise ObservationError(
                "latest_projection page_indexing_example_last_crawl must be null when no reason rows exist"
            )

    # --- Cross-report reconciliation: reject conflation ---
    # A Page indexing aggregate that disagrees with the latest per-URL
    # inspection must be marked ``report_lag_or_unreconciled``. Silently
    # selecting the aggregate count to overwrite the per-URL evidence, or
    # declaring the aggregate "fresh" while it contradicts the inspection, is
    # the conflation this validator rejects.
    pi_indexed = lp["page_indexing_indexed"]
    urls_on_google = lp["canonical_urls_on_google"]
    if pi_indexed != urls_on_google and lp["page_indexing_data_freshness"] == "fresh":
        raise ObservationError(
            "page_indexing aggregate indexed count disagrees with per-URL inspection "
            "but data_freshness is 'fresh'; a lagging aggregate must be marked "
            "'report_lag_or_unreconciled', not silently reconciled"
        )

    # --- Dimension table projection ---
    # Validate dimension table projection fields against the latest observation.
    dim_tables = latest_perf.get("dimension_tables")
    if dim_tables is not None:
        page_table = dim_tables["page"]
        if lp["page_dimension_impressions_sum"] != page_table["displayed_row_sum"]["impressions"]:
            raise ObservationError(
                "latest_projection page_dimension_impressions_sum disagrees with latest observation"
            )
        if lp["page_dimension_row_count"] != len(page_table["rows"]):
            raise ObservationError(
                "latest_projection page_dimension_row_count disagrees with latest observation"
            )
        if lp["page_dimension_missing_row_count"] != len(page_table["missing_rows"]):
            raise ObservationError(
                "latest_projection page_dimension_missing_row_count disagrees with latest observation"
            )
        sa_table = dim_tables["search_appearance"]
        if lp["search_appearance_state"] != sa_table["state"]:
            raise ObservationError(
                "latest_projection search_appearance_state disagrees with latest observation"
            )
    else:
        for key in (
            "page_dimension_impressions_sum",
            "page_dimension_row_count",
            "page_dimension_missing_row_count",
            "search_appearance_state",
        ):
            if lp.get(key) is not None:
                raise ObservationError(
                    f"latest_projection {key} must be null when no dimension tables exist"
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
    # Page indexing aggregate bindings: the lagging report-level aggregate is
    # preserved separately from the per-URL inspection and must remain visible.
    pi_indexed = projection.get("page_indexing_indexed")
    pi_not_indexed = projection.get("page_indexing_not_indexed")
    if pi_indexed is not None:
        phrases.append((
            f"{pi_indexed} indexed",
            "page indexing aggregate indexed count in Current interpretation",
        ))
    if pi_not_indexed is not None:
        phrases.append((
            f"{pi_not_indexed} not indexed",
            "page indexing aggregate not-indexed count in Current interpretation",
        ))
    if projection.get("page_indexing_data_freshness") == "report_lag_or_unreconciled":
        phrases.append((
            "lagging Page indexing aggregate",
            "page indexing aggregate lag state in Current interpretation",
        ))
    pi_reason = projection.get("page_indexing_reason")
    if pi_reason is not None:
        phrases.append((
            pi_reason,
            "page indexing reason in Current interpretation",
        ))
    page_sum = projection.get("page_dimension_impressions_sum")
    if page_sum is not None:
        phrases.append((
            f"{page_sum} page-level impressions",
            "page dimension impressions sum in Current interpretation",
        ))
    page_rows = projection.get("page_dimension_row_count")
    if page_rows is not None:
        phrases.append((
            f"{page_rows} page rows",
            "page dimension row count in Current interpretation",
        ))
    sa_state = projection.get("search_appearance_state")
    if sa_state == "no_data":
        phrases.append((
            "Search appearance reports no data",
            "search appearance state in Current interpretation",
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
    # Page indexing aggregate bindings: the roadmap must honestly note the
    # lagging aggregate alongside the five positive per-URL inspections.
    pi_indexed = projection.get("page_indexing_indexed")
    pi_not_indexed = projection.get("page_indexing_not_indexed")
    if pi_indexed is not None:
        phrases.append((
            f"{pi_indexed} indexed",
            "page indexing aggregate indexed count in roadmap",
        ))
    if pi_not_indexed is not None:
        phrases.append((
            f"{pi_not_indexed} not indexed",
            "page indexing aggregate not-indexed count in roadmap",
        ))
    if projection.get("page_indexing_data_freshness") == "report_lag_or_unreconciled":
        phrases.append((
            "lagging Page indexing aggregate",
            "page indexing aggregate lag state in roadmap",
        ))
    page_sum = projection.get("page_dimension_impressions_sum")
    if page_sum is not None:
        phrases.append((
            f"{page_sum} page-level impressions",
            "page dimension impressions sum in roadmap",
        ))
    page_rows = projection.get("page_dimension_row_count")
    if page_rows is not None:
        phrases.append((
            f"{page_rows} page rows",
            "page dimension row count in roadmap",
        ))
    sa_state = projection.get("search_appearance_state")
    if sa_state == "no_data":
        phrases.append((
            "search appearance reports no data",
            "search appearance state in roadmap",
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
