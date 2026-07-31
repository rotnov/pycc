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
import unicodedata
from typing import Any


GITHUB_SURFACE = "github_repository_search"
GOOGLE_SURFACE = "google_web"
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
RAW_HTML_START = re.compile(
    r"(?:</?[A-Za-z][A-Za-z0-9-]*(?=[ \t\n\f/>])|<\?|<![A-Z]|<!\[CDATA\[)",
    re.IGNORECASE,
)
LIST_MARKER = re.compile(r"(?:[-+*]|\d+[.)])[ \t]+(.*)\Z")
SIMPLE_TERM = re.compile(r"[A-Za-z0-9]+\Z")
BOOLEAN_TERMS = {"AND", "NOT", "OR"}
LIFECYCLES = {"active", "diagnostic", "retired"}
KPI_ROLES = {"product_acquisition", "diagnostic", "excluded"}
INTENT_KPI_ROLES = {
    "product_category": "product_acquisition",
    "category_version": "product_acquisition",
    "task_output": "product_acquisition",
    "brand": "diagnostic",
    "metadata_diagnostic": "diagnostic",
    "topic_diagnostic": "diagnostic",
    "competitive_category": "diagnostic",
    "authorship_narrative": "excluded",
}
SEMANTIC_IDENTITY_VERSIONS = {
    GITHUB_SURFACE: "github-repository-search-v1",
    GOOGLE_SURFACE: "google-web-query-v1",
}
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
GOOGLE_SURFACE_CONTRACT = {
    "provider": "google",
    "transport": "search_console_performance",
    "sort": "provider_aggregated",
    "result_window": None,
    "qualifier_policy": (
        "Use processed owner-facing query data; do not substitute site: "
        "searches or URL Inspection."
    ),
}
REGISTRY_KEYS = {
    "registry_version",
    "registry_activated_at",
    "semantic_identity_versions",
    "surfaces",
    "measurements",
    "queries",
}
QUERY_KEYS = {
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
BOOTSTRAP_CHECKPOINTS = [
    {
        "required_prefix_rows": 108,
        "sha256": "e1e44e137edce9300e75648e898b41dd3b8e25f13e06ba5264b8ee61b0fad433",
    },
    {
        "required_prefix_rows": 130,
        "sha256": "3ebf1ad5457aef04840be6ce397bb4e03415ffdac04edcab3e8cde3a5a76bef5",
    },
]
BOOTSTRAP_FILE_SHA256 = {
    "SEARCH_QUERY_REGISTRY.json": (
        "6f14805935905fcfc73b5ec2bb7f047cef5c5d11e6ff574bef3618cf82fedf77"
    ),
    "SEARCH_VISIBILITY.md": (
        "df8a7dad6b867876a5e95a272195fc0c6411c7258e8f1038cdb8edeb17f97d3f"
    ),
    "SEARCH_VISIBILITY_CHECKPOINTS.json": (
        "c55b4a4f1a11025bdde26825bfe762fc243d62997edc2f72ab5725f80ded943b"
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


def commonmark_lines(markdown: str) -> list[str]:
    """Split only the CR, LF, and CRLF line endings CommonMark recognizes."""
    return markdown.replace("\r\n", "\n").replace("\r", "\n").split("\n")


def atx_heading(line: str) -> tuple[int, str] | None:
    match = re.fullmatch(r" {0,3}(#{1,6})(?:[ \t]+(.*)|[ \t]*)", line)
    if match is None:
        return None
    content = (match.group(2) or "").strip(" \t")
    content = re.sub(r"[ \t]+#+[ \t]*\Z", "", content).rstrip(" \t")
    return len(match.group(1)), content


def visible_block_content(line: str) -> str:
    """Remove Markdown container prefixes from one prospective block line."""
    content = line.strip(" \t")
    while content:
        if content.startswith(">"):
            content = content[1:].lstrip(" \t")
            continue
        marker = LIST_MARKER.fullmatch(content)
        if marker is not None:
            content = marker.group(1).lstrip(" \t")
            continue
        break
    return content.rstrip(" \t")


def has_indented_code_prefix(line: str) -> bool:
    """Detect code indentation after the supported block containers."""
    content = line
    while True:
        quote = re.match(r" {0,3}>[ \t]?", content)
        if quote is not None:
            content = content[quote.end() :]
            continue
        list_item = re.match(r" {0,3}(?:[-+*]|\d+[.)])([ \t]+)", content)
        if list_item is not None:
            whitespace = list_item.group(1)
            if "\t" in whitespace or len(whitespace) >= 5:
                return True
            content = content[list_item.end() :]
            continue
        return content.startswith("\t") or content.startswith("    ")


def top_level_source(lines: list[str], start: int, end: int) -> bool:
    """Return whether every source line is outside containers at column zero."""
    return all(
        line == line.lstrip(" \t")
        and visible_block_content(line) == line.strip(" \t")
        for line in lines[start:end]
    )


def markdown_headings(markdown: str) -> list[tuple[int, int, int, str, bool]]:
    """Return heading start, content start, level, and title for visible blocks."""
    if "<!--" in markdown or "-->" in markdown:
        raise AuditError("search visibility ledger cannot contain HTML comments")
    if RAW_HTML_START.search(markdown):
        raise AuditError(
            "search visibility ledger cannot contain HTML tags or raw HTML constructs"
        )
    lines = commonmark_lines(markdown)
    for line in lines:
        content = visible_block_content(line)
        if has_indented_code_prefix(line) and (
            atx_heading(content) is not None
            or SETEXT_UNDERLINE.fullmatch(content) is not None
        ):
            raise AuditError(
                "search visibility headings cannot use indented code blocks"
            )
        if SETEXT_UNDERLINE.fullmatch(content) is not None:
            raise AuditError(
                "search visibility ledger cannot contain Setext underlines"
            )
        if FENCE_START.fullmatch(content):
            raise AuditError("search visibility ledger cannot contain fenced blocks")
        if content == "$$":
            raise AuditError(
                "search visibility ledger cannot contain display-math blocks"
            )

    headings: list[tuple[int, int, int, str, bool]] = []
    for index, line in enumerate(lines):
        content = visible_block_content(line)
        parsed = atx_heading(content)
        if parsed is not None:
            if any(character in parsed[1] for character in "[]<>"):
                raise AuditError(
                    "search visibility headings cannot contain inline links or HTML"
                )
            headings.append(
                (
                    index,
                    index + 1,
                    parsed[0],
                    parsed[1],
                    top_level_source(lines, index, index + 1),
                )
            )
            continue
    return headings


def rendered_heading_words(candidate: str) -> list[str]:
    """Return visible heading words after entity and markup normalization."""
    rendered = html.unescape(candidate)
    if any(character in rendered for character in "*_~`"):
        raise AuditError(
            "search visibility headings cannot contain inline markup markers"
        )
    if any(
        unicodedata.category(character) in {"Cc", "Cf", "Mn", "Me"}
        for character in rendered
    ):
        raise AuditError(
            "search visibility headings cannot contain invisible Unicode formatting"
        )
    if not rendered.isascii():
        raise AuditError(
            "search visibility headings must use ASCII text to prevent confusables"
        )
    return re.findall(r"[A-Za-z0-9]+", rendered.casefold())


def rendered_heading_matches(candidate: str, expected: str) -> bool:
    """Find the canonical words anywhere to reject rendered lookalikes."""
    candidate_words = rendered_heading_words(candidate)
    expected_words = rendered_heading_words(expected)
    return any(
        candidate_words[index : index + len(expected_words)] == expected_words
        for index in range(len(candidate_words) - len(expected_words) + 1)
    )


def section(markdown: str, heading: str) -> str:
    lines = commonmark_lines(markdown)
    headings = markdown_headings(markdown)
    matches = [
        item
        for item in headings
        if item[2] == 2 and rendered_heading_matches(item[3], heading)
    ]
    if len(matches) != 1:
        raise AuditError(f"expected exactly one level-2 {heading!r} section")
    if matches[0][3] != heading:
        raise AuditError(f"canonical {heading!r} title must match exactly")
    canonical_line = lines[matches[0][0]]
    if (
        canonical_line != canonical_line.lstrip(" \t")
        or visible_block_content(canonical_line) != canonical_line.strip(" \t")
    ):
        raise AuditError(f"canonical {heading!r} heading must be top-level")
    start = matches[0][1]
    end = len(lines)
    for heading_start, _, level, _, top_level in headings:
        if heading_start >= start and level <= 2 and top_level:
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


def ascii_lower(value: str) -> str:
    return value.translate(
        str.maketrans("ABCDEFGHIJKLMNOPQRSTUVWXYZ", "abcdefghijklmnopqrstuvwxyz")
    )


def semantic_identity(surface: str, raw_query: str, version: str) -> str:
    if not raw_query or raw_query != raw_query.strip():
        raise AuditError("query text must be nonempty and have no edge whitespace")
    if surface == GITHUB_SURFACE:
        terms = raw_query.split()
        if terms and all(
            SIMPLE_TERM.fullmatch(term) and term not in BOOLEAN_TERMS for term in terms
        ):
            normalized = " ".join(sorted(ascii_lower(term) for term in terms))
            return f"{version}:bag:{normalized}"
        normalized = " ".join(ascii_lower(raw_query).split())
        return f"{version}:syntax:{normalized}"
    if surface == GOOGLE_SURFACE:
        return f"{version}:raw:{raw_query}"
    raise AuditError(f"unsupported query surface: {surface!r}")


def parse_rank(value: str) -> int | None:
    if value == ">50":
        return None
    if not re.fullmatch(r"[1-9]\d*", value):
        raise AuditError("history rank must be a positive integer or >50")
    return int(value)


def history_raw_query(value: str) -> str:
    if len(value) < 2 or value[0] != "`" or value[-1] != "`":
        raise AuditError("history query must preserve backticked raw text")
    raw_query = value[1:-1]
    if "`" in raw_query:
        raise AuditError("history raw query cannot contain embedded backticks")
    return raw_query


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
    header_boundary = True
    for line in commonmark_lines(
        section(markdown, "GitHub repository search history")
    ):
        if not line.startswith("|"):
            if line.lstrip(" \t").startswith("|") or line.count("|") >= 5:
                raise AuditError(
                    "GitHub history table lines must use an unindented leading pipe"
                )
            if not saw_header:
                header_boundary = re.fullmatch(r"[ \t]*", line) is not None
                continue
            if saw_header and not saw_delimiter:
                raise AuditError("GitHub history header must be followed by its delimiter")
            if saw_delimiter:
                table_ended = True
            continue
        if table_ended:
            raise AuditError("GitHub history table cannot resume after an interruption")
        stripped = line.strip(" \t")
        if (
            not stripped.endswith("|")
            or stripped.startswith("||")
            or stripped.endswith("||")
        ):
            raise AuditError(
                "GitHub history table lines must use exactly one leading and "
                "trailing pipe"
            )
        cells = [cell.strip(" \t") for cell in stripped[1:-1].split("|")]
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
            if not header_boundary:
                raise AuditError(
                    "GitHub history header must follow a valid block boundary"
                )
            saw_header = True
            continue
        if len(cells) == 6 and all(re.fullmatch(r":?-+:?", cell) for cell in cells):
            if not all(re.fullmatch(r":?-{3,}:?", cell) for cell in cells):
                raise AuditError(
                    "GitHub history delimiter cells require at least three hyphens"
                )
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


def unique_json_object(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    value: dict[str, Any] = {}
    for key, item in pairs:
        if key in value:
            raise AuditError(f"JSON object contains duplicate key {key!r}")
        value[key] = item
    return value


def load_object(path: Path) -> dict[str, Any]:
    try:
        value = json.loads(path.read_text(), object_pairs_hook=unique_json_object)
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
    for line in commonmark_lines(markdown):
        if "search-history-checkpoint:" not in line:
            continue
        match = ROADMAP_CHECKPOINT.fullmatch(line.strip(" \t"))
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
    if set(registry) != REGISTRY_KEYS:
        raise AuditError("registry has unexpected fields")
    if require_integer(registry["registry_version"], "registry_version") != 1:
        raise AuditError("registry_version must be 1")
    if registry.get("registry_activated_at") != ACTIVATED_AT:
        raise AuditError("registry activation timestamp is immutable")
    versions = registry.get("semantic_identity_versions")
    if versions != SEMANTIC_IDENTITY_VERSIONS:
        raise AuditError("registry semantic identity versions were rewritten")
    surfaces = registry.get("surfaces")
    if not isinstance(surfaces, dict) or set(surfaces) != {
        GITHUB_SURFACE,
        GOOGLE_SURFACE,
    }:
        raise AuditError("registry must define exact GitHub and Google surfaces")
    if surfaces[GITHUB_SURFACE] != GITHUB_SURFACE_CONTRACT:
        raise AuditError("GitHub measurement surface contract was rewritten")
    if surfaces[GOOGLE_SURFACE] != GOOGLE_SURFACE_CONTRACT:
        raise AuditError("Google measurement surface contract was rewritten")
    require_integer(
        surfaces[GITHUB_SURFACE]["result_window"],
        "GitHub surface result_window",
    )

    queries = registry.get("queries")
    if not isinstance(queries, list) or not queries:
        raise AuditError("registry queries must be a nonempty list")
    queries_by_id: dict[str, dict[str, Any]] = {}
    queries_by_key: dict[tuple[str, str], dict[str, Any]] = {}
    product_identities: set[tuple[str, str]] = set()
    history_last_observed: dict[str, datetime] = {}
    for row in rows:
        raw_query = history_raw_query(row[1])
        observed = parse_timestamp(row[0], "history observed_at")
        previous = history_last_observed.get(raw_query)
        if previous is None or observed > previous:
            history_last_observed[raw_query] = observed
    for query in queries:
        if not isinstance(query, dict) or set(query) != QUERY_KEYS:
            raise AuditError("registry query has unexpected fields")
        for field in (
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
            if not isinstance(query[field], str) or not query[field].strip():
                raise AuditError(f"registry query {field} must be a nonempty string")
        query_id = query["id"]
        surface = query["surface"]
        raw_query = query["raw_query"]
        if surface == GITHUB_SURFACE and "`" in raw_query:
            raise AuditError("GitHub registry raw_query cannot contain backticks")
        if surface == GITHUB_SURFACE and "|" in raw_query:
            raise AuditError("GitHub registry raw_query cannot contain pipes")
        if surface == GITHUB_SURFACE and len(raw_query.splitlines()) != 1:
            raise AuditError(
                "GitHub registry raw_query cannot contain line separators"
            )
        if surface == GITHUB_SURFACE and (
            "<!--" in raw_query
            or "-->" in raw_query
            or RAW_HTML_START.search(raw_query) is not None
        ):
            raise AuditError(
                "GitHub registry raw_query cannot contain HTML tag or comment syntax, "
                "including raw constructs"
            )
        if surface == GITHUB_SURFACE and any(
            unicodedata.category(character) in {"Cc", "Cf"}
            for character in raw_query
        ):
            raise AuditError(
                "GitHub registry raw_query cannot contain control characters "
                "or Unicode formatting characters"
            )
        if surface not in SEMANTIC_IDENTITY_VERSIONS:
            raise AuditError("registry query has an unsupported surface")
        version = SEMANTIC_IDENTITY_VERSIONS[surface]
        if query["semantic_identity_version"] != version:
            raise AuditError("registry query semantic identity version was rewritten")
        if query["semantic_identity"] != semantic_identity(surface, raw_query, version):
            raise AuditError("registry query semantic identity was rewritten")
        if query["lifecycle"] not in LIFECYCLES:
            raise AuditError("registry query lifecycle is invalid")
        if query["kpi_role"] not in KPI_ROLES:
            raise AuditError("registry query KPI role is invalid")
        expected_kpi_role = INTENT_KPI_ROLES.get(query["intent_class"])
        if expected_kpi_role is None or query["kpi_role"] != expected_kpi_role:
            raise AuditError("registry query intent/KPI role combination is invalid")
        query_activated = parse_timestamp(
            query["activated_at"], f"query {query_id} activated_at"
        )
        if query_activated > audited_at:
            raise AuditError("registry query activation cannot be in the future")
        retired_at = query["retired_at"]
        if query["lifecycle"] == "retired":
            query_retired = parse_timestamp(
                retired_at, f"query {query_id} retired_at"
            )
            if query_retired < query_activated:
                raise AuditError("registry query retires before activation")
            if query_retired > audited_at:
                raise AuditError("registry query retirement cannot be in the future")
            last_history = (
                history_last_observed.get(raw_query)
                if surface == GITHUB_SURFACE
                else None
            )
            if last_history is not None and query_retired <= last_history:
                raise AuditError(
                    "registry query retirement must follow its final history observation"
                )
        elif retired_at is not None:
            raise AuditError("non-retired registry query cannot have retired_at")
        if surface == GITHUB_SURFACE:
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
        if query_id in queries_by_id:
            raise AuditError("registry query IDs must be unique")
        query_key = (surface, raw_query)
        if query_key in queries_by_key:
            raise AuditError("registry exact queries must be unique per surface")
        if query["kpi_role"] == "product_acquisition":
            if query["lifecycle"] not in {"active", "retired"}:
                raise AuditError(
                    "product-acquisition query must be active or retired"
                )
            if query["alias_of"] is not None:
                raise AuditError("product-acquisition query cannot be an alias")
            if query["lifecycle"] == "active":
                identity_key = (surface, query["semantic_identity"])
                if identity_key in product_identities:
                    raise AuditError("product semantic identity is double-counted")
                product_identities.add(identity_key)
        queries_by_id[query_id] = query
        queries_by_key[query_key] = query

    for query in queries:
        alias_of = query["alias_of"]
        if alias_of is None:
            continue
        if not isinstance(alias_of, str) or alias_of not in queries_by_id:
            raise AuditError("registry query has an unknown alias target")
        target = queries_by_id[alias_of]
        if (
            query["kpi_role"] != "diagnostic"
            or query["surface"] != target["surface"]
            or query["semantic_identity"] != target["semantic_identity"]
        ):
            raise AuditError("registry query alias contract is invalid")

    retired_authorship = queries_by_key.get((GITHUB_SURFACE, "AI-native compiler"))
    if retired_authorship is None or (
        retired_authorship["intent_class"] != "authorship_narrative"
        or retired_authorship["lifecycle"] != "retired"
        or retired_authorship["kpi_role"] != "excluded"
    ):
        raise AuditError("AI-native compiler must remain a retired authorship query")

    measurements = registry.get("measurements")
    if not isinstance(measurements, list):
        raise AuditError("registry measurements must be a list")
    base_registry_path = base_root / "docs" / "SEARCH_QUERY_REGISTRY.json"
    if base_registry_path.exists():
        base_registry = load_object(base_registry_path)
        base_queries = base_registry.get("queries")
        if not isinstance(base_queries, list):
            raise AuditError("trusted base registry queries must be a list")
        if len(queries) < len(base_queries):
            raise AuditError("registry must preserve trusted base queries")
        immutable_query_fields = QUERY_KEYS - {"lifecycle", "retired_at"}
        for base_query, query in zip(base_queries, queries):
            if query == base_query:
                continue
            preserves_identity = (
                isinstance(base_query, dict)
                and all(
                    query.get(field) == base_query.get(field)
                    for field in immutable_query_fields
                )
            )
            retires_once = (
                isinstance(base_query, dict)
                and base_query.get("lifecycle") in {"active", "diagnostic"}
                and base_query.get("retired_at") is None
                and query.get("lifecycle") == "retired"
                and query.get("retired_at") is not None
            )
            if not preserves_identity or not retires_once:
                raise AuditError("registry must preserve trusted base queries")
        base_measurements = base_registry.get("measurements")
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
        query_activated = parse_timestamp(
            query["activated_at"], f"query {query_id} activated_at"
        )
        if observed < query_activated:
            raise AuditError("measurement predates query activation")
        if query["retired_at"] is not None and observed >= parse_timestamp(
            query["retired_at"], f"query {query_id} retired_at"
        ):
            raise AuditError("measurement is outside the query lifecycle")
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
        raw_query = history_raw_query(row[1])
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
    base_registry_path = base_root / "docs" / "SEARCH_QUERY_REGISTRY.json"
    trusted_prefix_rows = len(base_rows)
    if not base_registry_path.exists():
        if values != BOOTSTRAP_CHECKPOINTS:
            raise AuditError("initial registry must use the exact reviewed bootstrap")
        for filename, expected_digest in BOOTSTRAP_FILE_SHA256.items():
            payload = head_root.joinpath("docs", filename).read_bytes()
            if hashlib.sha256(payload).hexdigest() != expected_digest:
                raise AuditError("initial registry must use the exact reviewed bootstrap")
        trusted_prefix_rows = len(head_rows)
    activated_at = parse_timestamp(ACTIVATED_AT, "registry activated_at")
    if audited_at is None:
        audited_at = datetime.now(timezone.utc)
    validate_registry(
        head_root,
        base_root,
        head_rows,
        trusted_prefix_rows,
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
