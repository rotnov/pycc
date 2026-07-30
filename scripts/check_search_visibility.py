#!/usr/bin/env python3
"""Validate the search-intent registry and its documentation projections."""

from __future__ import annotations

import argparse
from datetime import datetime, timezone
import hashlib
import json
from pathlib import Path
import re
from typing import Any


GITHUB_SURFACE = "github_repository_search"
GOOGLE_SURFACE = "google_web"
LIFECYCLES = {"active", "diagnostic", "retired"}
KPI_ROLES = {"product_acquisition", "diagnostic", "excluded"}
REQUIRED_QUERY_KEYS = {
    "id",
    "surface",
    "raw_query",
    "semantic_identity",
    "semantic_identity_version",
    "intent_class",
    "lifecycle",
    "kpi_role",
    "rationale",
    "activated_at",
    "retired_at",
    "alias_of",
}
REQUIRED_MEASUREMENT_KEYS = {
    "snapshot_id",
    "observed_at",
    "query_id",
    "surface",
    "provider",
    "request_parameters",
    "sort_contract",
    "result_window",
    "returned_results",
    "api_total",
    "target_rank",
    "incomplete_results",
    "ordered_corpus_sha256",
}
SIMPLE_TERM = re.compile(r"[A-Za-z0-9]+\Z")
SHA256 = re.compile(r"[0-9a-f]{64}\Z")
UTC_TIMESTAMP = re.compile(r"\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z\Z")
BOOLEAN_TERMS = {"AND", "NOT", "OR"}


class ContractError(ValueError):
    """Raised when a checked search-visibility contract is inconsistent."""


def parse_utc_timestamp(value: Any, description: str) -> datetime:
    """Parse the canonical second-precision UTC timestamp used by the ledger."""

    if not isinstance(value, str) or not UTC_TIMESTAMP.fullmatch(value):
        raise ContractError(f"{description} must be an ISO 8601 UTC timestamp")
    try:
        parsed = datetime.fromisoformat(value.removesuffix("Z") + "+00:00")
    except ValueError as error:
        raise ContractError(
            f"{description} must be an ISO 8601 UTC timestamp"
        ) from error
    if parsed.tzinfo != timezone.utc or parsed.microsecond:
        raise ContractError(
            f"{description} must use second precision and the Z timezone"
        )
    return parsed


def ascii_lower(value: str) -> str:
    """Lower ASCII without collapsing unreviewed Unicode distinctions."""

    return value.translate(
        str.maketrans("ABCDEFGHIJKLMNOPQRSTUVWXYZ", "abcdefghijklmnopqrstuvwxyz")
    )


def semantic_identity(surface: str, raw_query: str, version: str) -> str:
    """Derive the provider-specific identity supported by the current evidence."""

    if not raw_query or raw_query != raw_query.strip():
        raise ContractError("Query text must be nonempty and have no edge whitespace")

    if surface == GITHUB_SURFACE:
        terms = raw_query.split()
        simple = bool(terms) and all(
            SIMPLE_TERM.fullmatch(term) and term not in BOOLEAN_TERMS for term in terms
        )
        if simple:
            normalized = " ".join(sorted(ascii_lower(term) for term in terms))
            return f"{version}:bag:{normalized}"
        return f"{version}:syntax:{raw_query}"

    if surface == GOOGLE_SURFACE:
        return f"{version}:raw:{raw_query}"

    raise ContractError(f"Unsupported query surface: {surface!r}")


def section(markdown: str, heading: str) -> str:
    marker = f"## {heading}\n"
    if markdown.count(marker) != 1:
        raise ContractError(f"Expected exactly one {marker.strip()!r} section")
    tail = markdown.split(marker, 1)[1]
    return tail.split("\n## ", 1)[0]


def github_history_rows(markdown: str) -> list[list[str]]:
    rows: list[list[str]] = []
    for line in section(markdown, "GitHub repository search history").splitlines():
        if not line.startswith("|"):
            continue
        cells = [cell.strip() for cell in line.strip().strip("|").split("|")]
        if not cells or cells[0] == "Observed at (UTC)":
            continue
        if set(cells[0]) <= {"-", ":"}:
            continue
        if len(cells) != 6:
            raise ContractError(f"Malformed GitHub history row: {line}")
        rows.append(cells)
    return rows


def history_digest(rows: list[list[str]]) -> str:
    payload = json.dumps(
        rows,
        ensure_ascii=False,
        separators=(",", ":"),
    ).encode()
    return hashlib.sha256(payload).hexdigest()


def validate_history_deltas(rows: list[list[str]]) -> None:
    """Ensure a delta never claims precision absent from the rank evidence."""

    previous_ranks: dict[str, int | None] = {}
    for row in rows:
        raw_query = row[1]
        rank_text = row[2]
        if rank_text == ">50":
            rank = None
        else:
            try:
                rank = int(rank_text)
            except ValueError as error:
                raise ContractError(f"Invalid history rank: {rank_text!r}") from error
            if rank < 1:
                raise ContractError(f"Invalid history rank: {rank_text!r}")

        if raw_query not in previous_ranks:
            expected_delta = "—"
        else:
            previous = previous_ranks[raw_query]
            if rank is None:
                expected_delta = "—"
            elif previous is None:
                expected_delta = "new"
            else:
                movement = previous - rank
                expected_delta = f"+{movement}" if movement > 0 else str(movement)
        if row[3] != expected_delta:
            raise ContractError(
                f"History delta for {raw_query} must be {expected_delta!r}, "
                f"found {row[3]!r}"
            )
        previous_ranks[raw_query] = rank


def load_json(path: Path) -> dict[str, Any]:
    try:
        value = json.loads(path.read_text())
    except (OSError, json.JSONDecodeError) as error:
        raise ContractError(
            f"Could not read valid JSON from {path}: {error}"
        ) from error
    if not isinstance(value, dict):
        raise ContractError(f"Expected a JSON object in {path}")
    return value


def validate_registry(
    registry: dict[str, Any],
) -> tuple[
    dict[tuple[str, str], dict[str, Any]],
    dict[str, dict[str, Any]],
]:
    if registry.get("registry_version") != 1:
        raise ContractError("Search query registry_version must be 1")

    versions = registry.get("semantic_identity_versions")
    surfaces = registry.get("surfaces")
    if not isinstance(versions, dict) or set(versions) != {
        GITHUB_SURFACE,
        GOOGLE_SURFACE,
    }:
        raise ContractError("Registry must define both semantic identity versions")
    if not isinstance(surfaces, dict) or set(surfaces) != {
        GITHUB_SURFACE,
        GOOGLE_SURFACE,
    }:
        raise ContractError("Registry must define GitHub and Google surfaces")

    queries = registry.get("queries")
    if not isinstance(queries, list) or not queries:
        raise ContractError("Registry queries must be a nonempty list")

    by_key: dict[tuple[str, str], dict[str, Any]] = {}
    by_id: dict[str, dict[str, Any]] = {}
    active_product_identities: dict[tuple[str, str], str] = {}
    for query in queries:
        if not isinstance(query, dict) or set(query) != REQUIRED_QUERY_KEYS:
            raise ContractError("Every query must have the exact registered field set")
        for key in (
            "id",
            "surface",
            "raw_query",
            "semantic_identity",
            "semantic_identity_version",
            "intent_class",
            "lifecycle",
            "kpi_role",
            "rationale",
            "activated_at",
        ):
            if not isinstance(query[key], str) or not query[key].strip():
                raise ContractError(f"Query field {key!r} must be a nonempty string")

        query_id = query["id"]
        surface = query["surface"]
        raw_query = query["raw_query"]
        if query_id in by_id:
            raise ContractError(f"Duplicate query id: {query_id}")
        key = (surface, raw_query)
        if key in by_key:
            raise ContractError(f"Duplicate exact query on one surface: {key!r}")
        if surface not in versions:
            raise ContractError(f"Unknown query surface: {surface!r}")
        version = versions[surface]
        if query["semantic_identity_version"] != version:
            raise ContractError(f"Wrong semantic identity version for {query_id}")
        expected_identity = semantic_identity(surface, raw_query, version)
        if query["semantic_identity"] != expected_identity:
            raise ContractError(f"Wrong semantic identity for {query_id}")
        if query["lifecycle"] not in LIFECYCLES:
            raise ContractError(f"Invalid lifecycle for {query_id}")
        if query["kpi_role"] not in KPI_ROLES:
            raise ContractError(f"Invalid KPI role for {query_id}")
        if query["lifecycle"] == "retired" and not query["retired_at"]:
            raise ContractError(f"Retired query {query_id} needs retired_at")
        if query["lifecycle"] != "retired" and query["retired_at"] is not None:
            raise ContractError(f"Non-retired query {query_id} cannot have retired_at")
        activated_at = parse_utc_timestamp(
            query["activated_at"], f"Query {query_id} activated_at"
        )
        if query["retired_at"] is not None:
            retired_at = parse_utc_timestamp(
                query["retired_at"], f"Query {query_id} retired_at"
            )
            if retired_at < activated_at:
                raise ContractError(f"Query {query_id} retires before activation")

        if query["kpi_role"] == "product_acquisition":
            if query["lifecycle"] != "active":
                raise ContractError(f"Product KPI query {query_id} must be active")
            if query["intent_class"] == "authorship_narrative":
                raise ContractError("Authorship narrative cannot enter the product KPI")
            if query["alias_of"] is not None:
                raise ContractError(
                    "An alias cannot increase the product KPI query count"
                )
            identity_key = (surface, expected_identity)
            if identity_key in active_product_identities:
                earlier = active_product_identities[identity_key]
                raise ContractError(
                    f"Product KPI semantic identity is double-counted by {earlier} and {query_id}"
                )
            active_product_identities[identity_key] = query_id

        by_id[query_id] = query
        by_key[key] = query

    for query in queries:
        alias_of = query["alias_of"]
        if alias_of is not None:
            if not isinstance(alias_of, str) or alias_of not in by_id:
                raise ContractError(f"Unknown alias target for {query['id']}")
            if query["kpi_role"] != "diagnostic":
                raise ContractError(f"Alias {query['id']} must remain diagnostic")
            target = by_id[alias_of]
            if (
                query["surface"] != target["surface"]
                or query["semantic_identity"] != target["semantic_identity"]
            ):
                raise ContractError(
                    f"Alias {query['id']} must share its target's semantic identity"
                )

    retired = by_key.get((GITHUB_SURFACE, "AI-native compiler"))
    if retired is None:
        raise ContractError("Registry must preserve the AI-native compiler experiment")
    if (
        retired["intent_class"] != "authorship_narrative"
        or retired["lifecycle"] != "retired"
        or retired["kpi_role"] != "excluded"
    ):
        raise ContractError(
            "AI-native compiler must be a retired authorship diagnostic"
        )

    return by_key, by_id


def validate_measurements(
    registry: dict[str, Any],
    by_id: dict[str, dict[str, Any]],
) -> tuple[datetime, dict[tuple[str, str], dict[str, Any]]]:
    """Validate replay metadata required for every registry-era history row."""

    activated_at = parse_utc_timestamp(
        registry.get("registry_activated_at"), "registry_activated_at"
    )
    surfaces = registry["surfaces"]
    measurements = registry.get("measurements")
    if not isinstance(measurements, list):
        raise ContractError("Registry measurements must be a list")

    by_history_key: dict[tuple[str, str], dict[str, Any]] = {}
    snapshot_ids: set[str] = set()
    for measurement in measurements:
        if (
            not isinstance(measurement, dict)
            or set(measurement) != REQUIRED_MEASUREMENT_KEYS
        ):
            raise ContractError(
                "Every registry-era measurement must have the exact replay field set"
            )
        snapshot_id = measurement["snapshot_id"]
        if not isinstance(snapshot_id, str) or not snapshot_id.strip():
            raise ContractError("Measurement snapshot_id must be nonempty")
        if snapshot_id in snapshot_ids:
            raise ContractError(f"Duplicate measurement snapshot_id: {snapshot_id}")
        snapshot_ids.add(snapshot_id)

        observed_at_text = measurement["observed_at"]
        observed_at = parse_utc_timestamp(
            observed_at_text, f"Measurement {snapshot_id} observed_at"
        )
        if observed_at < activated_at:
            raise ContractError(
                f"Measurement {snapshot_id} predates registry activation"
            )

        query_id = measurement["query_id"]
        if query_id not in by_id:
            raise ContractError(f"Measurement {snapshot_id} has an unknown query_id")
        query = by_id[query_id]
        surface = measurement["surface"]
        if surface != query["surface"] or surface != GITHUB_SURFACE:
            raise ContractError(f"Measurement {snapshot_id} has the wrong surface")
        query_activated_at = parse_utc_timestamp(
            query["activated_at"], f"Query {query_id} activated_at"
        )
        if observed_at < query_activated_at:
            raise ContractError(f"Measurement {snapshot_id} predates query activation")
        if query["retired_at"] is not None:
            query_retired_at = parse_utc_timestamp(
                query["retired_at"], f"Query {query_id} retired_at"
            )
            if observed_at >= query_retired_at:
                raise ContractError(
                    f"Measurement {snapshot_id} is outside the query lifecycle"
                )
        surface_contract = surfaces[surface]
        if measurement["provider"] != surface_contract["provider"]:
            raise ContractError(f"Measurement {snapshot_id} has the wrong provider")
        if measurement["sort_contract"] != surface_contract["sort"]:
            raise ContractError(
                f"Measurement {snapshot_id} has the wrong sort contract"
            )
        if measurement["result_window"] != surface_contract["result_window"]:
            raise ContractError(
                f"Measurement {snapshot_id} has the wrong result window"
            )

        raw_query = query["raw_query"]
        expected_parameters = {"q": raw_query, "per_page": 50}
        if measurement["request_parameters"] != expected_parameters:
            raise ContractError(
                f"Measurement {snapshot_id} must preserve exact request parameters"
            )
        returned = measurement["returned_results"]
        total = measurement["api_total"]
        rank = measurement["target_rank"]
        window = measurement["result_window"]
        if (
            not isinstance(returned, int)
            or isinstance(returned, bool)
            or not 0 <= returned <= window
        ):
            raise ContractError(
                f"Measurement {snapshot_id} has invalid returned_results"
            )
        if not isinstance(total, int) or isinstance(total, bool) or total < returned:
            raise ContractError(f"Measurement {snapshot_id} has invalid api_total")
        if rank is not None and (
            not isinstance(rank, int)
            or isinstance(rank, bool)
            or not 1 <= rank <= returned
        ):
            raise ContractError(f"Measurement {snapshot_id} has invalid target_rank")
        if not isinstance(measurement["incomplete_results"], bool):
            raise ContractError(
                f"Measurement {snapshot_id} has invalid incomplete_results"
            )
        if not isinstance(
            measurement["ordered_corpus_sha256"], str
        ) or not SHA256.fullmatch(measurement["ordered_corpus_sha256"]):
            raise ContractError(
                f"Measurement {snapshot_id} needs an ordered-corpus SHA-256"
            )

        history_key = (observed_at_text, raw_query)
        if history_key in by_history_key:
            raise ContractError(
                f"Duplicate registry-era measurement for {history_key!r}"
            )
        by_history_key[history_key] = measurement

    return activated_at, by_history_key


def validate(root: Path) -> None:
    registry_path = root / "docs" / "SEARCH_QUERY_REGISTRY.json"
    checkpoints_path = root / "docs" / "SEARCH_VISIBILITY_CHECKPOINTS.json"
    visibility_path = root / "docs" / "SEARCH_VISIBILITY.md"
    roadmap_path = root / "docs" / "ROADMAP.md"
    website_path = root / "docs" / "WEBSITE.md"
    spec_path = root / "docs" / "SPEC.md"

    registry = load_json(registry_path)
    by_key, by_id = validate_registry(registry)
    registry_activated_at, measurements = validate_measurements(registry, by_id)
    visibility = visibility_path.read_text()
    compact_visibility = " ".join(visibility.split())
    rows = github_history_rows(visibility)

    checkpoints = load_json(checkpoints_path)
    if set(checkpoints) != {"checkpoint_version", "surfaces"}:
        raise ContractError("History checkpoints must have the exact field set")
    if checkpoints["checkpoint_version"] != 1:
        raise ContractError("History checkpoint_version must be 1")
    surface_checkpoints = checkpoints["surfaces"]
    if not isinstance(surface_checkpoints, dict) or set(surface_checkpoints) != {
        GITHUB_SURFACE
    }:
        raise ContractError("History checkpoints must define the GitHub surface")
    github_checkpoints = surface_checkpoints[GITHUB_SURFACE]
    if not isinstance(github_checkpoints, list) or not github_checkpoints:
        raise ContractError("GitHub history checkpoints must be a nonempty list")

    previous_required_rows = 0
    for checkpoint in github_checkpoints:
        if not isinstance(checkpoint, dict) or set(checkpoint) != {
            "required_prefix_rows",
            "sha256",
        }:
            raise ContractError("Every history checkpoint needs rows and SHA-256")
        required_rows = checkpoint["required_prefix_rows"]
        required_digest = checkpoint["sha256"]
        if (
            not isinstance(required_rows, int)
            or isinstance(required_rows, bool)
            or required_rows <= previous_required_rows
        ):
            raise ContractError(
                "History checkpoint row counts must be positive and increasing"
            )
        if not isinstance(required_digest, str) or not SHA256.fullmatch(
            required_digest
        ):
            raise ContractError("History checkpoint needs a SHA-256 digest")
        if len(rows) < required_rows:
            raise ContractError("Historical GitHub observations were deleted")
        if history_digest(rows[:required_rows]) != required_digest:
            raise ContractError("Historical GitHub observation prefix was rewritten")
        previous_required_rows = required_rows

    if len(rows) > previous_required_rows:
        raise ContractError(
            "Latest history checkpoint must cover every committed GitHub observation"
        )
    validate_history_deltas(rows)

    for row in rows:
        observed_at_text = row[0]
        observed_at = parse_utc_timestamp(observed_at_text, "History observed_at")
        raw_query = row[1]
        if (
            len(raw_query) < 2
            or not raw_query.startswith("`")
            or not raw_query.endswith("`")
        ):
            raise ContractError(
                f"History query must preserve backticked raw text: {raw_query}"
            )
        key = (GITHUB_SURFACE, raw_query[1:-1])
        if key not in by_key:
            raise ContractError(
                f"Historical query is missing from registry: {key[1]!r}"
            )
        if observed_at >= registry_activated_at:
            measurement = measurements.get((observed_at_text, key[1]))
            if measurement is None:
                raise ContractError(
                    "Registry-era history row is missing replay metadata"
                )
            expected_rank = (
                ">50"
                if measurement["target_rank"] is None
                else str(measurement["target_rank"])
            )
            if (
                row[2] != expected_rank
                or row[4] != str(measurement["returned_results"])
                or row[5] != str(measurement["api_total"])
            ):
                raise ContractError(
                    "Registry-era history row disagrees with replay metadata"
                )

    history_keys = {(row[0], row[1][1:-1]) for row in rows}
    orphaned_measurements = set(measurements) - history_keys
    if orphaned_measurements:
        raise ContractError("Registry-era replay metadata has no matching history row")

    required_visibility = (
        "[machine-readable query registry](./SEARCH_QUERY_REGISTRY.json)",
        "Product-acquisition positions are reported separately",
        "`AI-native compiler` is a retired authorship diagnostic",
        "`ordered-corpus SHA-256`",
    )
    for phrase in required_visibility:
        if phrase not in compact_visibility:
            raise ContractError(
                f"SEARCH_VISIBILITY.md is missing contract text: {phrase}"
            )

    current = section(visibility, "Current interpretation")
    retired_sentence = "`AI-native compiler` is a retired authorship diagnostic"
    if "AI-native compiler" in current and retired_sentence not in current:
        raise ContractError("Current interpretation misclassifies AI-native compiler")
    forbidden_current = (
        "AI-native compiler` target keyword",
        "AI-native compiler` product query",
        "AI-native compiler` acquisition query",
    )
    if any(phrase in current for phrase in forbidden_current):
        raise ContractError("Retired authorship query appears in the product KPI")

    roadmap = roadmap_path.read_text()
    compact_roadmap = " ".join(roadmap.split())
    for phrase in (
        (
            "machine-readable query registry, and append-only prefix checkpoints "
            "separate product-acquisition positions"
        ),
        "retired authorship diagnostics",
    ):
        if phrase not in compact_roadmap:
            raise ContractError(
                f"ROADMAP.md is missing search-intent projection: {phrase}"
            )

    website = website_path.read_text()
    compact_website = " ".join(website.split())
    for phrase in (
        'unsupported HTML `meta name="keywords"` field is forbidden',
        "product-acquisition landing",
        "authorship/provenance page",
    ):
        if phrase not in compact_website:
            raise ContractError(f"WEBSITE.md is missing metadata contract: {phrase}")

    spec = spec_path.read_text()
    if "[SEARCH_QUERY_REGISTRY.json](./SEARCH_QUERY_REGISTRY.json)" not in spec:
        raise ContractError("SPEC.md must expose the machine-readable query registry")
    if (
        "[SEARCH_VISIBILITY_CHECKPOINTS.json](./SEARCH_VISIBILITY_CHECKPOINTS.json)"
        not in spec
    ):
        raise ContractError("SPEC.md must expose the append-only history checkpoints")


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--root",
        type=Path,
        default=Path(__file__).resolve().parent.parent,
    )
    args = parser.parse_args()
    try:
        validate(args.root.resolve())
    except (ContractError, OSError) as error:
        raise SystemExit(f"search visibility contract failed: {error}") from error
    print("Search visibility contract checks passed.")


if __name__ == "__main__":
    main()
