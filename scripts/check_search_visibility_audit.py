#!/usr/bin/env python3
"""Audit pull-request search evidence with code from the trusted base revision."""

from __future__ import annotations

import argparse
from datetime import datetime, timezone
import hashlib
import json
from pathlib import Path
import re
from typing import Any


GITHUB_SURFACE = "github_repository_search"
ACTIVATED_AT = "2026-07-30T14:14:24Z"
UTC_TIMESTAMP = re.compile(r"\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z\Z")
SHA256 = re.compile(r"[0-9a-f]{64}\Z")
ROADMAP_CHECKPOINT = re.compile(
    r"<!-- search-history-checkpoint: github_repository_search "
    r"(?P<rows>[1-9]\d*) (?P<sha>[0-9a-f]{64}) -->\Z"
)
GITHUB_SURFACE_CONTRACT = {
    "provider": "github",
    "transport": "GET /search/repositories",
    "sort": "default_best_match",
    "result_window": 50,
    "qualifier_policy": (
        "No user: or repo: qualifier; field and topic qualifiers are "
        "diagnostic only."
    ),
}
MEASUREMENT_KEYS = {
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


class AuditError(ValueError):
    """Raised when untrusted head data violates the trusted search contract."""


def section(markdown: str, heading: str) -> str:
    marker = f"## {heading}\n"
    if markdown.count(marker) != 1:
        raise AuditError(f"expected exactly one {marker.strip()!r} section")
    return markdown.split(marker, 1)[1].split("\n## ", 1)[0]


def parse_timestamp(value: Any, description: str) -> datetime:
    if not isinstance(value, str) or not UTC_TIMESTAMP.fullmatch(value):
        raise AuditError(f"{description} must use YYYY-MM-DDTHH:MM:SSZ")
    try:
        parsed = datetime.fromisoformat(value[:-1] + "+00:00")
    except ValueError as error:
        raise AuditError(f"{description} is not a valid UTC timestamp") from error
    if parsed.tzinfo != timezone.utc:
        raise AuditError(f"{description} must use UTC")
    return parsed


def require_integer(value: Any, description: str) -> int:
    if not isinstance(value, int) or isinstance(value, bool):
        raise AuditError(f"{description} must be an integer")
    return value


def parse_rank(value: str) -> int | None:
    if value == ">50":
        return None
    if not re.fullmatch(r"[1-9]\d*", value):
        raise AuditError("history rank must be a positive integer or >50")
    return int(value)


def rank_delta(previous: int | None, current: int | None, has_previous: bool) -> str:
    if current is None or not has_previous:
        return "—"
    if previous is None:
        return "new"
    delta = previous - current
    return f"+{delta}" if delta > 0 else str(delta)


def history_rows(markdown: str) -> list[list[str]]:
    rows: list[list[str]] = []
    saw_header = False
    saw_delimiter = False
    for line in section(markdown, "GitHub repository search history").splitlines():
        if not line.startswith("|"):
            continue
        cells = [cell.strip() for cell in line.strip().strip("|").split("|")]
        if cells == [
            "Observed at (UTC)",
            "Exact query",
            "Rank",
            "Δ",
            "Results",
            "Total",
        ]:
            if saw_header:
                raise AuditError("duplicate GitHub history header")
            saw_header = True
            continue
        if len(cells) == 6 and all(re.fullmatch(r":?-+:?", cell) for cell in cells):
            if not saw_header or saw_delimiter:
                raise AuditError("misplaced GitHub history delimiter")
            saw_delimiter = True
            continue
        if not saw_header or not saw_delimiter:
            raise AuditError("GitHub history rows must follow the table delimiter")
        if len(cells) != 6:
            raise AuditError(f"malformed GitHub history row: {line}")
        parse_timestamp(cells[0], "history observed_at")
        rows.append(cells)
    if not saw_header or not saw_delimiter or not rows:
        raise AuditError("GitHub history table is incomplete")
    return rows


def history_digest(rows: list[list[str]]) -> str:
    payload = json.dumps(rows, ensure_ascii=False, separators=(",", ":")).encode()
    return hashlib.sha256(payload).hexdigest()


def load_object(path: Path) -> dict[str, Any]:
    try:
        value = json.loads(path.read_text())
    except (OSError, json.JSONDecodeError) as error:
        raise AuditError(f"could not read valid JSON from {path}: {error}") from error
    if not isinstance(value, dict):
        raise AuditError(f"expected a JSON object in {path}")
    return value


def checkpoints(head_root: Path, rows: list[list[str]]) -> list[dict[str, Any]]:
    document = load_object(
        head_root / "docs" / "SEARCH_VISIBILITY_CHECKPOINTS.json"
    )
    if set(document) != {"checkpoint_version", "surfaces"}:
        raise AuditError("checkpoint document has unexpected fields")
    if document["checkpoint_version"] != 1:
        raise AuditError("checkpoint_version must be 1")
    surfaces = document["surfaces"]
    if not isinstance(surfaces, dict) or set(surfaces) != {GITHUB_SURFACE}:
        raise AuditError("checkpoint document must define only GitHub search")
    values = surfaces[GITHUB_SURFACE]
    if not isinstance(values, list) or not values:
        raise AuditError("GitHub checkpoints must be a nonempty list")

    previous = 0
    for checkpoint in values:
        if not isinstance(checkpoint, dict) or set(checkpoint) != {
            "required_prefix_rows",
            "sha256",
        }:
            raise AuditError("checkpoint has unexpected fields")
        required = checkpoint["required_prefix_rows"]
        digest = checkpoint["sha256"]
        if (
            not isinstance(required, int)
            or isinstance(required, bool)
            or required <= previous
            or required > len(rows)
        ):
            raise AuditError("checkpoint row counts must be increasing prefixes")
        if not isinstance(digest, str) or not SHA256.fullmatch(digest):
            raise AuditError("checkpoint digest must be lowercase SHA-256")
        if history_digest(rows[:required]) != digest:
            raise AuditError("checkpoint digest does not bind its history prefix")
        previous = required
    if previous != len(rows):
        raise AuditError("latest checkpoint must cover the complete history")
    return values


def roadmap_checkpoints(head_root: Path) -> list[dict[str, Any]]:
    markdown = (head_root / "docs" / "ROADMAP.md").read_text()
    values: list[dict[str, Any]] = []
    for line in markdown.splitlines():
        if "search-history-checkpoint:" not in line:
            continue
        match = ROADMAP_CHECKPOINT.fullmatch(line.strip())
        if match is None:
            raise AuditError("roadmap has a malformed search-history checkpoint")
        values.append(
            {
                "required_prefix_rows": int(match["rows"]),
                "sha256": match["sha"],
            }
        )
    if not values:
        raise AuditError("roadmap must project search-history checkpoints")
    return values


def validate_registry(
    head_root: Path,
    base_root: Path,
    rows: list[list[str]],
    activated_at: datetime,
    audited_at: datetime,
) -> None:
    registry = load_object(head_root / "docs" / "SEARCH_QUERY_REGISTRY.json")
    if registry.get("registry_activated_at") != ACTIVATED_AT:
        raise AuditError("registry activation timestamp is immutable")
    surfaces = registry.get("surfaces")
    if not isinstance(surfaces, dict) or set(surfaces) != {
        GITHUB_SURFACE,
        "google_web",
    }:
        raise AuditError("registry must define exact GitHub and Google surfaces")
    if surfaces[GITHUB_SURFACE] != GITHUB_SURFACE_CONTRACT:
        raise AuditError("GitHub measurement surface contract was rewritten")

    queries = registry.get("queries")
    if not isinstance(queries, list):
        raise AuditError("registry queries must be a list")
    queries_by_id: dict[str, dict[str, Any]] = {}
    for query in queries:
        if not isinstance(query, dict) or not isinstance(query.get("id"), str):
            raise AuditError("registry query is malformed")
        if query["id"] in queries_by_id:
            raise AuditError("registry query IDs must be unique")
        queries_by_id[query["id"]] = query

    measurements = registry.get("measurements")
    if not isinstance(measurements, list):
        raise AuditError("registry measurements must be a list")
    base_registry_path = base_root / "docs" / "SEARCH_QUERY_REGISTRY.json"
    if base_registry_path.exists():
        base_measurements = load_object(base_registry_path).get("measurements")
        if not isinstance(base_measurements, list):
            raise AuditError("trusted base registry measurements must be a list")
        if measurements[: len(base_measurements)] != base_measurements:
            raise AuditError("registry must preserve trusted base measurements")
    projected: dict[tuple[str, str], dict[str, Any]] = {}
    snapshot_ids: set[str] = set()
    for measurement in measurements:
        if not isinstance(measurement, dict) or set(measurement) != MEASUREMENT_KEYS:
            raise AuditError("measurement has unexpected fields")
        snapshot_id = measurement["snapshot_id"]
        if (
            not isinstance(snapshot_id, str)
            or not snapshot_id
            or snapshot_id in snapshot_ids
        ):
            raise AuditError("measurement snapshot_id must be a unique string")
        snapshot_ids.add(snapshot_id)
        query_id = measurement["query_id"]
        if not isinstance(query_id, str):
            raise AuditError("measurement query_id must be a string")
        query = queries_by_id.get(query_id)
        if query is None or query.get("surface") != GITHUB_SURFACE:
            raise AuditError("measurement does not reference a GitHub query")
        observed = parse_timestamp(measurement["observed_at"], "measurement observed_at")
        if observed < activated_at:
            raise AuditError("measurement predates registry activation")
        if observed > audited_at:
            raise AuditError("measurement observed_at cannot be in the future")
        raw_query = query.get("raw_query")
        key = (measurement["observed_at"], raw_query)
        if not isinstance(raw_query, str) or key in projected:
            raise AuditError("measurement query projection is invalid")
        if measurement["surface"] != GITHUB_SURFACE:
            raise AuditError("measurement surface must be GitHub search")
        if measurement["provider"] != "github":
            raise AuditError("measurement provider must be github")
        if measurement["request_parameters"] != {
            "q": raw_query,
            "per_page": 50,
        }:
            raise AuditError("measurement request parameters were rewritten")
        if measurement["sort_contract"] != "default_best_match":
            raise AuditError("measurement sort contract was rewritten")
        if measurement["result_window"] != 50:
            raise AuditError("measurement result window was rewritten")
        returned_results = require_integer(
            measurement["returned_results"], "measurement returned_results"
        )
        api_total = require_integer(measurement["api_total"], "measurement api_total")
        if not 0 <= returned_results <= measurement["result_window"]:
            raise AuditError("measurement returned_results is outside its result window")
        if api_total < returned_results:
            raise AuditError("measurement api_total is smaller than returned_results")
        target_rank = measurement["target_rank"]
        if target_rank is not None:
            target_rank = require_integer(target_rank, "measurement target_rank")
            if not 1 <= target_rank <= returned_results:
                raise AuditError("measurement target_rank is outside returned results")
        if not isinstance(measurement["incomplete_results"], bool):
            raise AuditError("measurement incomplete_results must be boolean")
        corpus_digest = measurement["ordered_corpus_sha256"]
        if not isinstance(corpus_digest, str) or not SHA256.fullmatch(corpus_digest):
            raise AuditError("measurement corpus digest must be lowercase SHA-256")
        projected[key] = measurement

    history_keys: set[tuple[str, str]] = set()
    previous: datetime | None = None
    previous_ranks: dict[str, int | None] = {}
    for row in rows:
        observed = parse_timestamp(row[0], "history observed_at")
        if previous is not None and observed < previous:
            raise AuditError("history timestamps must be nondecreasing")
        if observed > audited_at:
            raise AuditError("history observed_at cannot be in the future")
        previous = observed
        raw_query = row[1]
        if len(raw_query) < 2 or raw_query[0] != "`" or raw_query[-1] != "`":
            raise AuditError("history query must preserve backticked raw text")
        raw_query = raw_query[1:-1]
        current_rank = parse_rank(row[2])
        expected_delta = rank_delta(
            previous_ranks.get(raw_query), current_rank, raw_query in previous_ranks
        )
        if row[3] != expected_delta:
            raise AuditError("history rank delta disagrees with the preceding observation")
        previous_ranks[raw_query] = current_rank
        key = (row[0], raw_query)
        if key in history_keys:
            raise AuditError("history observation keys must be unique")
        history_keys.add(key)
        if observed < activated_at:
            continue
        measurement = projected.get(key)
        if measurement is None:
            raise AuditError("registry-era history row lacks trusted replay metadata")
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
            raise AuditError("history row disagrees with replay metadata")
    registry_era_keys = {
        key
        for key in history_keys
        if parse_timestamp(key[0], "history observed_at") >= activated_at
    }
    if set(projected) != registry_era_keys:
        raise AuditError("registry-era measurement projection is incomplete")


def validate(
    head_root: Path,
    base_root: Path,
    audited_at: datetime | None = None,
) -> None:
    base_rows = history_rows(
        (base_root / "docs" / "SEARCH_VISIBILITY.md").read_text()
    )
    head_rows = history_rows(
        (head_root / "docs" / "SEARCH_VISIBILITY.md").read_text()
    )
    if head_rows[: len(base_rows)] != base_rows:
        raise AuditError("head history must preserve the trusted base prefix")
    values = checkpoints(head_root, head_rows)
    if roadmap_checkpoints(head_root) != values:
        raise AuditError("roadmap checkpoints do not match the bound ledger")
    activated_at = parse_timestamp(ACTIVATED_AT, "registry activated_at")
    if audited_at is None:
        audited_at = datetime.now(timezone.utc)
    validate_registry(head_root, base_root, head_rows, activated_at, audited_at)


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--head-root", type=Path, required=True)
    parser.add_argument("--base-root", type=Path, required=True)
    args = parser.parse_args()
    try:
        validate(args.head_root.resolve(), args.base_root.resolve())
    except (AuditError, OSError) as error:
        raise SystemExit(f"trusted search visibility audit failed: {error}") from error
    print("Trusted search visibility audit passed.")


if __name__ == "__main__":
    main()
