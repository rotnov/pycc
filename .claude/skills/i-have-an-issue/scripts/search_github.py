#!/usr/bin/env python3
"""Search GitHub for issue, pull request, commit, or code candidates."""

from __future__ import annotations

import argparse
import json
import os
import re
import sys
import urllib.error
import urllib.parse
import urllib.request
from typing import Any


DEFAULT_API_URL = "https://api.github.com"
API_VERSION = "2022-11-28"
MAX_SEARCH_RESULTS = 1_000
OWNER_RE = re.compile(r"^[A-Za-z0-9][A-Za-z0-9-]{0,38}$")
REPOSITORY_NAME_RE = re.compile(r"^[A-Za-z0-9._-]{1,100}$")


class SearchError(RuntimeError):
    """A GitHub API request failed."""


def positive_int(value: str) -> int:
    parsed = int(value)
    if parsed < 1:
        raise argparse.ArgumentTypeError("must be at least 1")
    return parsed


def search_result_limit(value: str) -> int:
    parsed = positive_int(value)
    if parsed > MAX_SEARCH_RESULTS:
        raise argparse.ArgumentTypeError(
            f"must not exceed GitHub Search's {MAX_SEARCH_RESULTS}-result limit"
        )
    return parsed


def non_negative_int(value: str) -> int:
    parsed = int(value)
    if parsed < 0:
        raise argparse.ArgumentTypeError("must be at least 0")
    return parsed


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description=(
            "Discover GitHub candidates through the Search API. Results are "
            "leads, not verified engineering evidence."
        )
    )
    parser.add_argument("--query", required=True, help="GitHub search expression")
    parser.add_argument(
        "--repo",
        help="Optional OWNER/REPO restriction",
    )
    parser.add_argument(
        "--kind",
        choices=("issues", "commits", "code"),
        default="issues",
        help="Search surface (default: issues)",
    )
    parser.add_argument(
        "--item-type",
        choices=("any", "issue", "pr"),
        default="any",
        help="For issue search, restrict to issues or pull requests",
    )
    parser.add_argument(
        "--state",
        choices=("all", "open", "closed"),
        default="all",
        help="For issue search, restrict state",
    )
    parser.add_argument(
        "--limit",
        type=search_result_limit,
        default=30,
        help=f"Maximum results to return, up to {MAX_SEARCH_RESULTS} (default: 30)",
    )
    parser.add_argument(
        "--body-chars",
        type=non_negative_int,
        default=600,
        help="Maximum issue body excerpt length (default: 600)",
    )
    parser.add_argument(
        "--format",
        choices=("json", "jsonl"),
        default="json",
        help="Output format (default: json)",
    )
    args = parser.parse_args()

    if args.repo:
        owner, separator, repository = args.repo.partition("/")
        if (
            separator != "/"
            or not OWNER_RE.fullmatch(owner)
            or not REPOSITORY_NAME_RE.fullmatch(repository)
        ):
            parser.error("--repo must use a valid GitHub OWNER/REPO form")
    if args.kind != "issues" and (args.item_type != "any" or args.state != "all"):
        parser.error("--item-type and --state apply only to --kind issues")
    return args


def build_query(args: argparse.Namespace) -> str:
    parts = [args.query.strip()]
    if args.repo:
        parts.append(f"repo:{args.repo}")
    if args.kind == "issues":
        if args.item_type == "issue":
            parts.append("is:issue")
        elif args.item_type == "pr":
            parts.append("is:pr")
        if args.state != "all":
            parts.append(f"state:{args.state}")
    return " ".join(part for part in parts if part)


def request_json(
    url: str, token: str | None
) -> tuple[dict[str, Any], dict[str, str]]:
    headers = {
        "Accept": "application/vnd.github+json",
        "X-GitHub-Api-Version": API_VERSION,
        "User-Agent": "rotnov-i-have-an-issue",
    }
    if token:
        headers["Authorization"] = f"Bearer {token}"

    request = urllib.request.Request(url, headers=headers)
    try:
        with urllib.request.urlopen(request, timeout=30) as response:
            payload = json.load(response)
            response_headers = {key.lower(): value for key, value in response.headers.items()}
            return payload, response_headers
    except urllib.error.HTTPError as error:
        response_headers = {key.lower(): value for key, value in error.headers.items()}
        try:
            payload = json.loads(error.read().decode("utf-8", errors="replace"))
            message = payload.get("message", str(error))
        except (json.JSONDecodeError, AttributeError):
            message = str(error)
        remaining = response_headers.get("x-ratelimit-remaining", "unknown")
        reset = response_headers.get("x-ratelimit-reset", "unknown")
        raise SearchError(
            f"GitHub API returned HTTP {error.code}: {message}; "
            f"rate-limit remaining={remaining}, reset={reset}"
        ) from error
    except urllib.error.URLError as error:
        raise SearchError(f"Could not reach GitHub API: {error.reason}") from error


def compact_text(value: str | None, limit: int) -> str:
    if not value or limit == 0:
        return ""
    compacted = " ".join(value.split())
    if len(compacted) <= limit:
        return compacted
    return compacted[: max(0, limit - 1)] + "…"


def repository_from_api_url(value: str | None) -> str | None:
    if not value:
        return None
    path = urllib.parse.urlparse(value).path
    marker = "/repos/"
    if marker not in path:
        return None
    return path.split(marker, 1)[1].strip("/") or None


def normalize_issue(item: dict[str, Any], body_chars: int) -> dict[str, Any]:
    return {
        "kind": "pull_request" if "pull_request" in item else "issue",
        "repository": repository_from_api_url(item.get("repository_url")),
        "number": item.get("number"),
        "title": item.get("title"),
        "state": item.get("state"),
        "url": item.get("html_url"),
        "author": (item.get("user") or {}).get("login"),
        "created_at": item.get("created_at"),
        "updated_at": item.get("updated_at"),
        "closed_at": item.get("closed_at"),
        "comments": item.get("comments"),
        "labels": [label.get("name") for label in item.get("labels", [])],
        "body_excerpt": compact_text(item.get("body"), body_chars),
        "score": item.get("score"),
    }


def normalize_commit(item: dict[str, Any]) -> dict[str, Any]:
    commit = item.get("commit") or {}
    author = commit.get("author") or {}
    repository = item.get("repository") or {}
    return {
        "kind": "commit",
        "repository": repository.get("full_name"),
        "sha": item.get("sha"),
        "url": item.get("html_url"),
        "message": compact_text(commit.get("message"), 600),
        "author": (item.get("author") or {}).get("login") or author.get("name"),
        "date": author.get("date"),
        "score": item.get("score"),
    }


def normalize_code(item: dict[str, Any]) -> dict[str, Any]:
    repository = item.get("repository") or {}
    return {
        "kind": "code",
        "repository": repository.get("full_name"),
        "name": item.get("name"),
        "path": item.get("path"),
        "url": item.get("html_url"),
        "sha": item.get("sha"),
        "score": item.get("score"),
    }


def normalize_item(
    kind: str, item: dict[str, Any], body_chars: int
) -> dict[str, Any]:
    if kind == "issues":
        return normalize_issue(item, body_chars)
    if kind == "commits":
        return normalize_commit(item)
    return normalize_code(item)


def search(args: argparse.Namespace) -> dict[str, Any]:
    token = os.environ.get("GITHUB_TOKEN") or os.environ.get("GH_TOKEN")
    qualified_query = build_query(args)
    collected: list[dict[str, Any]] = []
    total_count = 0
    incomplete_results = False
    rate_limit: dict[str, str | None] = {}
    page = 1

    while len(collected) < args.limit:
        per_page = min(100, args.limit - len(collected))
        parameters = urllib.parse.urlencode(
            {
                "q": qualified_query,
                "per_page": per_page,
                "page": page,
            }
        )
        payload, headers = request_json(
            f"{DEFAULT_API_URL}/search/{args.kind}?{parameters}", token
        )
        raw_items = payload.get("items", [])
        if page == 1:
            total_count = payload.get("total_count", 0)
        incomplete_results = incomplete_results or bool(
            payload.get("incomplete_results", False)
        )
        rate_limit = {
            "remaining": headers.get("x-ratelimit-remaining"),
            "reset": headers.get("x-ratelimit-reset"),
        }
        collected.extend(
            normalize_item(args.kind, item, args.body_chars) for item in raw_items
        )
        if len(raw_items) < per_page:
            break
        page += 1

    return {
        "kind": args.kind,
        "query": args.query,
        "qualified_query": qualified_query,
        "repository": args.repo,
        "authenticated": bool(token),
        "total_count": total_count,
        "incomplete_results": incomplete_results,
        "returned_count": len(collected),
        "rate_limit": rate_limit,
        "items": collected[: args.limit],
    }


def emit(result: dict[str, Any], output_format: str) -> None:
    if output_format == "json":
        json.dump(result, sys.stdout, ensure_ascii=False, indent=2)
        sys.stdout.write("\n")
        return

    metadata = {key: value for key, value in result.items() if key != "items"}
    print(json.dumps({"metadata": metadata}, ensure_ascii=False))
    for item in result["items"]:
        print(json.dumps(item, ensure_ascii=False))


def main() -> int:
    args = parse_args()
    try:
        emit(search(args), args.format)
    except SearchError as error:
        print(f"error: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
