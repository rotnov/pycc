#!/usr/bin/env python3
"""Audit pull-request search evidence with code from the trusted base revision."""

from __future__ import annotations

import argparse
from datetime import datetime, timezone
import hashlib
import html
import json
from pathlib import Path
import re
from typing import Any


GITHUB_SURFACE = "github_repository_search"
ACTIVATED_AT = "2026-07-30T14:14:24Z"
UTC_TIMESTAMP = re.compile(r"\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z\Z")
SHA256 = re.compile(r"[0-9a-f]{64}\Z")
GITHUB_QUALIFIER = re.compile(r"(?i)(?<![A-Za-z0-9_])([a-z][a-z0-9_-]*):")
DESCRIPTION_DIAGNOSTIC = re.compile(
    r"(?i)(?<![A-Za-z0-9_])in:description(?:\s|\Z)"
)
TOPIC_DIAGNOSTIC = re.compile(r"(?i)topic:[a-z0-9][a-z0-9-]*\Z")
ROADMAP_CHECKPOINT = re.compile(
    r"<!-- search-history-checkpoint: github_repository_search "
    r"(?P<rows>[1-9]\d*) (?P<sha>[0-9a-f]{64}) -->\Z"
)
FENCE_START = re.compile(r"(?:`{3,}|~{3,})(?:[^\r\n]*)\Z")
SETEXT_UNDERLINE = re.compile(r"(=+|-+)[ \t]*\Z")
RAW_HTML_BLOCK_START = re.compile(
    r"<(?:!--|/?[A-Za-z][A-Za-z0-9-]*(?=[ \t/>])|\?|![A-Z]|!\[CDATA\[)",
    re.IGNORECASE,
)
LIST_MARKER = re.compile(r"(?:[-+*]|\d+[.)])[ \t]+(.*)\Z")
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


def atx_heading(line: str) -> tuple[int, str] | None:
    match = re.fullmatch(r" {0,3}(#{1,6})(?:[ \t]+(.*)|[ \t]*)", line)
    if match is None:
        return None
    content = (match.group(2) or "").strip()
    content = re.sub(r"[ \t]+#+[ \t]*\Z", "", content).rstrip()
    return len(match.group(1)), content


def visible_block_content(line: str) -> str:
    """Remove Markdown container prefixes from one prospective block line."""
    content = line.strip()
    while content:
        if content.startswith(">"):
            content = content[1:].lstrip(" \t")
            continue
        marker = LIST_MARKER.fullmatch(content)
        if marker is not None:
            content = marker.group(1).lstrip(" \t")
            continue
        break
    return content.rstrip()


def markdown_headings(markdown: str) -> list[tuple[int, int, int, str]]:
    """Return heading start, content start, level, and title for visible blocks."""
    lines = markdown.splitlines()
    for line in lines:
        content = visible_block_content(line)
        if FENCE_START.fullmatch(content):
            raise AuditError("search visibility ledger cannot contain fenced blocks")
        if RAW_HTML_BLOCK_START.match(content):
            raise AuditError("search visibility ledger cannot contain raw HTML blocks")

    headings: list[tuple[int, int, int, str]] = []
    for index, line in enumerate(lines):
        content = visible_block_content(line)
        parsed = atx_heading(content)
        if parsed is not None:
            if any(character in parsed[1] for character in "[]<>"):
                raise AuditError(
                    "search visibility headings cannot contain inline links or HTML"
                )
            headings.append((index, index + 1, parsed[0], parsed[1]))
            continue
        if not content or index + 1 >= len(lines):
            continue
        underline = SETEXT_UNDERLINE.fullmatch(
            visible_block_content(lines[index + 1])
        )
        if underline is not None:
            if any(character in content for character in "[]<>"):
                raise AuditError(
                    "search visibility headings cannot contain inline links or HTML"
                )
            level = 1 if underline.group(1).startswith("=") else 2
            headings.append((index, index + 2, level, content))
    return headings


def rendered_heading_matches(candidate: str, expected: str) -> bool:
    """Match the canonical title after entity and inline-markup normalization."""
    rendered = html.unescape(candidate)
    rendered = rendered.translate(str.maketrans("", "", "*_~`"))
    candidate_words = re.findall(r"[A-Za-z0-9]+", rendered.casefold())
    expected_words = re.findall(r"[A-Za-z0-9]+", expected.casefold())
    return any(
        candidate_words[index : index + len(expected_words)] == expected_words
        for index in range(len(candidate_words) - len(expected_words) + 1)
    )


def section(markdown: str, heading: str) -> str:
    lines = markdown.splitlines()
    headings = markdown_headings(markdown)
    matches = [
        item
        for item in headings
        if item[2] == 2 and rendered_heading_matches(item[3], heading)
    ]
    if len(matches) != 1:
        raise AuditError(f"expected exactly one level-2 {heading!r} section")
    start = matches[0][1]
    end = len(lines)
    for heading_start, _, level, _ in headings:
        if heading_start >= start and level == 2:
            end = heading_start
            break
    return "\n".join(lines[start:end])


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
    table_ended = False
    for line in section(markdown, "GitHub repository search history").splitlines():
        if not line.startswith("|"):
            if line.lstrip().startswith("|") or line.count("|") >= 5:
                raise AuditError(
                    "GitHub history table lines must use an unindented leading pipe"
                )
            if saw_header and not saw_delimiter:
                raise AuditError("GitHub history header must be followed by its delimiter")
            if saw_delimiter:
                table_ended = True
            continue
        if table_ended:
            raise AuditError("GitHub history table cannot resume after an interruption")
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
    checkpoint_version = require_integer(
        document["checkpoint_version"], "checkpoint_version"
    )
    if checkpoint_version != 1:
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
    trusted_prefix_rows: int,
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
    require_integer(
        surfaces[GITHUB_SURFACE]["result_window"],
        "GitHub surface result_window",
    )

    queries = registry.get("queries")
    if not isinstance(queries, list):
        raise AuditError("registry queries must be a list")
    queries_by_id: dict[str, dict[str, Any]] = {}
    for query in queries:
        if not isinstance(query, dict) or not isinstance(query.get("id"), str):
            raise AuditError("registry query is malformed")
        raw_query = query.get("raw_query")
        if query.get("surface") == GITHUB_SURFACE:
            if not isinstance(raw_query, str) or not raw_query:
                raise AuditError("GitHub query text must be a nonempty string")
            qualifiers = [
                match.group(1).lower()
                for match in GITHUB_QUALIFIER.finditer(raw_query)
            ]
            intent_class = query.get("intent_class")
            description_diagnostic = (
                intent_class == "metadata_diagnostic"
                and qualifiers == ["in"]
                and DESCRIPTION_DIAGNOSTIC.search(raw_query) is not None
            )
            topic_diagnostic = (
                intent_class == "topic_diagnostic"
                and qualifiers == ["topic"]
                and TOPIC_DIAGNOSTIC.fullmatch(raw_query) is not None
            )
            if qualifiers and not (description_diagnostic or topic_diagnostic):
                raise AuditError("GitHub query violates the reviewed qualifier policy")
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
        request_parameters = measurement["request_parameters"]
        if not isinstance(request_parameters, dict) or set(request_parameters) != {
            "q",
            "per_page",
        }:
            raise AuditError("measurement request parameters have unexpected fields")
        per_page = require_integer(
            request_parameters["per_page"], "measurement request per_page"
        )
        if request_parameters["q"] != raw_query or per_page != 50:
            raise AuditError("measurement request parameters were rewritten")
        if measurement["sort_contract"] != "default_best_match":
            raise AuditError("measurement sort contract was rewritten")
        result_window = require_integer(
            measurement["result_window"], "measurement result_window"
        )
        if result_window != 50:
            raise AuditError("measurement result window was rewritten")
        returned_results = require_integer(
            measurement["returned_results"], "measurement returned_results"
        )
        api_total = require_integer(measurement["api_total"], "measurement api_total")
        if not 0 <= returned_results <= result_window:
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
        if measurement["incomplete_results"]:
            raise AuditError("incomplete search responses cannot produce rank evidence")
        if returned_results != min(api_total, result_window):
            raise AuditError("complete measurement returned an incomplete result window")
        corpus_digest = measurement["ordered_corpus_sha256"]
        if not isinstance(corpus_digest, str) or not SHA256.fullmatch(corpus_digest):
            raise AuditError("measurement corpus digest must be lowercase SHA-256")
        projected[key] = measurement

    history_keys: set[tuple[str, str]] = set()
    previous: datetime | None = None
    previous_ranks: dict[str, int | None] = {}
    replay_keys: set[tuple[str, str]] = set()
    for row_index, row in enumerate(rows):
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
        requires_replay = row_index >= trusted_prefix_rows or observed >= activated_at
        if not requires_replay:
            continue
        replay_keys.add(key)
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
    if set(projected) != replay_keys:
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
    base_checkpoint_path = (
        base_root / "docs" / "SEARCH_VISIBILITY_CHECKPOINTS.json"
    )
    if base_checkpoint_path.exists():
        base_values = checkpoints(base_root, base_rows)
        if values[: len(base_values)] != base_values:
            raise AuditError("checkpoints must preserve the trusted base prefix")
    if roadmap_checkpoints(head_root) != values:
        raise AuditError("roadmap checkpoints do not match the bound ledger")
    activated_at = parse_timestamp(ACTIVATED_AT, "registry activated_at")
    if audited_at is None:
        audited_at = datetime.now(timezone.utc)
    validate_registry(
        head_root,
        base_root,
        head_rows,
        len(base_rows),
        activated_at,
        audited_at,
    )


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
