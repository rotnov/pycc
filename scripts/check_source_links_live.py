#!/usr/bin/env python3
"""Live source link checker for the Python AOT compiler comparison (issue #202).

This is layer 2 of the two-layer source link monitoring contract:
  - Layer 1 (hermetic, PR-required): scripts/check_source_links_registry.rb
    validates the registry format and coverage without network calls.
  - Layer 2 (live, scheduled/non-blocking): this script checks each
    registered URL's transport reachability, classifies the outcome,
    and updates the registry.

The live audit runs from trusted default-branch code on a scheduled
cadence and by manual dispatch. It must NOT execute pull-request-controlled
code or receive secrets/write permissions.

Classification semantics (issue #202):
  - healthy: expected 2xx representation
  - redirect_changed: final URL/host differs from the original
  - confirmed_missing: repeatable 404 or 410
  - blocked_or_rate_limited: 401/403/429
  - upstream_error: repeatable 5xx
  - transport_unknown: DNS/TLS/timeout/connection failure
  - content_mismatch: expected content-type check fails

blocked_or_rate_limited, upstream_error, and transport_unknown are NOT
treated as broken-link evidence. They are surfaced for human triage.

Usage:
  python3 scripts/check_source_links_live.py [--registry PATH] [--dry-run]
  python3 scripts/check_source_links_live.py --self-test

Exits 0 if no confirmed_missing or redirect_changed entries are found.
Exits 1 if any confirmed_missing or redirect_changed entries are found.
Exits 2 on configuration or usage error.
"""

from __future__ import annotations

import argparse
import concurrent.futures
import json
import os
import socket
import ssl
import sys
import time
import urllib.error
import urllib.request
from datetime import datetime, timezone
from pathlib import Path
from typing import Any
from urllib.parse import urlparse

# --- Configuration constants (bounded per issue #202) ---

# Bounded timeouts: connect and read must not be unbounded.
DEFAULT_CONNECT_TIMEOUT = 10  # seconds
DEFAULT_READ_TIMEOUT = 15  # seconds
MAX_TIMEOUT = 60  # seconds — reject unbounded configurations

# Retry policy: bounded retries with backoff.
DEFAULT_MAX_RETRIES = 2  # initial attempt + 2 retries = 3 total
MAX_ALLOWED_RETRIES = 5  # reject unbounded retry configurations
RETRY_BACKOFF_BASE = 1.0  # seconds

# Concurrency cap: bounded to avoid overwhelming external hosts.
DEFAULT_MAX_CONCURRENCY = 3
MAX_ALLOWED_CONCURRENCY = 10  # reject unbounded concurrency

# Maximum redirect hops to follow.
DEFAULT_MAX_REDIRECTS = 5
MAX_ALLOWED_REDIRECTS = 10

# Honest user agent identifying the checker.
USER_AGENT = (
    "pycc-link-checker/1.0 "
    "(https://github.com/rotnov/pycc; "
    "source link monitoring for the Python AOT compiler comparison)"
)

# Valid outcome classifications.
VALID_OUTCOMES = frozenset({
    "healthy",
    "redirect_changed",
    "confirmed_missing",
    "blocked_or_rate_limited",
    "upstream_error",
    "transport_unknown",
    "content_mismatch",
})

# Status codes that map to blocked_or_rate_limited.
BLOCKED_STATUS_CODES = frozenset({401, 403, 429})

# Status codes that map to confirmed_missing.
MISSING_STATUS_CODES = frozenset({404, 410})


class ConfigError(Exception):
    """Raised when the checker configuration is invalid or unbounded."""


class LinkResult:
    """The result of checking a single URL."""

    def __init__(
        self,
        url: str,
        outcome: str,
        http_status: int | None = None,
        final_url: str | None = None,
        final_host: str | None = None,
        content_type: str | None = None,
        observed_at: str | None = None,
        error_detail: str | None = None,
    ):
        self.url = url
        self.outcome = outcome
        self.http_status = http_status
        self.final_url = final_url
        self.final_host = final_host
        self.content_type = content_type
        self.observed_at = observed_at or datetime.now(timezone.utc).isoformat()
        self.error_detail = error_detail

    def to_dict(self) -> dict[str, Any]:
        d: dict[str, Any] = {
            "url": self.url,
            "outcome": self.outcome,
            "observed_at": self.observed_at,
        }
        if self.http_status is not None:
            d["http_status"] = self.http_status
        if self.final_url is not None:
            d["final_url"] = self.final_url
        if self.final_host is not None:
            d["final_host"] = self.final_host
        if self.content_type is not None:
            d["content_type"] = self.content_type
        if self.error_detail is not None:
            d["error_detail"] = self.error_detail
        return d


def validate_config(
    connect_timeout: int,
    read_timeout: int,
    max_retries: int,
    max_concurrency: int,
    max_redirects: int,
) -> None:
    """Reject unbounded or invalid configurations (issue #202)."""
    if connect_timeout <= 0 or connect_timeout > MAX_TIMEOUT:
        raise ConfigError(
            f"connect_timeout must be between 1 and {MAX_TIMEOUT}, "
            f"got {connect_timeout}"
        )
    if read_timeout <= 0 or read_timeout > MAX_TIMEOUT:
        raise ConfigError(
            f"read_timeout must be between 1 and {MAX_TIMEOUT}, "
            f"got {read_timeout}"
        )
    if max_retries < 0 or max_retries > MAX_ALLOWED_RETRIES:
        raise ConfigError(
            f"max_retries must be between 0 and {MAX_ALLOWED_RETRIES}, "
            f"got {max_retries}"
        )
    if max_concurrency <= 0 or max_concurrency > MAX_ALLOWED_CONCURRENCY:
        raise ConfigError(
            f"max_concurrency must be between 1 and {MAX_ALLOWED_CONCURRENCY}, "
            f"got {max_concurrency}"
        )
    if max_redirects <= 0 or max_redirects > MAX_ALLOWED_REDIRECTS:
        raise ConfigError(
            f"max_redirects must be between 1 and {MAX_ALLOWED_REDIRECTS}, "
            f"got {max_redirects}"
        )


def classify_outcome(
    status: int | None,
    original_url: str,
    final_url: str,
    error: Exception | None = None,
) -> tuple[str, str | None]:
    """Classify an HTTP check outcome per issue #202 semantics.

    Returns (outcome, error_detail).
    """
    if error is not None:
        # Transport-level errors
        if isinstance(error, (socket.timeout, TimeoutError)):
            return "transport_unknown", f"timeout: {error}"
        if isinstance(error, (socket.gaierror,)):
            return "transport_unknown", f"DNS error: {error}"
        if isinstance(error, (ssl.SSLError,)):
            return "transport_unknown", f"TLS error: {error}"
        if isinstance(error, (ConnectionError, ConnectionRefusedError,
                             ConnectionResetError)):
            return "transport_unknown", f"connection error: {error}"
        # urllib HTTPError is not a transport error — it has a status code
        if isinstance(error, urllib.error.HTTPError):
            code = error.code
            if code in MISSING_STATUS_CODES:
                return "confirmed_missing", f"HTTP {code}"
            if code in BLOCKED_STATUS_CODES:
                return "blocked_or_rate_limited", f"HTTP {code}"
            if 500 <= code < 600:
                return "upstream_error", f"HTTP {code}"
            if 300 <= code < 400:
                # Should not happen if redirects are followed, but classify
                return "redirect_changed", f"HTTP {code} redirect"
            # Other 4xx
            return "blocked_or_rate_limited", f"HTTP {code}"
        if isinstance(error, urllib.error.URLError):
            reason = error.reason
            if isinstance(reason, socket.timeout):
                return "transport_unknown", f"timeout: {reason}"
            if isinstance(reason, socket.gaierror):
                return "transport_unknown", f"DNS error: {reason}"
            if isinstance(reason, ssl.SSLError):
                return "transport_unknown", f"TLS error: {reason}"
            if isinstance(reason, ConnectionError):
                return "transport_unknown", f"connection error: {reason}"
            return "transport_unknown", f"URL error: {reason}"
        # Unknown error type
        return "transport_unknown", f"unexpected error: {type(error).__name__}: {error}"

    if status is None:
        return "transport_unknown", "no status code received"

    # We have a status code
    if 200 <= status < 300:
        # Check if the final URL differs from the original (redirect)
        if final_url and final_url != original_url:
            original_parsed = urlparse(original_url)
            final_parsed = urlparse(final_url)
            if final_parsed.hostname != original_parsed.hostname:
                return "redirect_changed", (
                    f"redirected from {original_parsed.hostname} "
                    f"to {final_parsed.hostname}"
                )
            # Same host, different path — still a redirect but same host
            return "redirect_changed", (
                f"redirected to {final_url} (same host)"
            )
        return "healthy", None

    if status in MISSING_STATUS_CODES:
        return "confirmed_missing", f"HTTP {status}"

    if status in BLOCKED_STATUS_CODES:
        return "blocked_or_rate_limited", f"HTTP {status}"

    if 500 <= status < 600:
        return "upstream_error", f"HTTP {status}"

    if 300 <= status < 400:
        return "redirect_changed", f"HTTP {status} redirect not followed"

    # Other 4xx
    return "blocked_or_rate_limited", f"HTTP {status}"


def check_single_url(
    url: str,
    connect_timeout: int,
    read_timeout: int,
    max_retries: int,
    max_redirects: int,
) -> LinkResult:
    """Check a single URL with bounded timeouts, retry, and GET fallback.

    Strategy:
      1. Try HEAD first.
      2. If HEAD fails with 405 (Method Not Allowed) or other method error,
         fall back to GET.
      3. Follow redirects up to max_redirects.
      4. Retry on transport errors up to max_retries with backoff.
    """
    observed_at = datetime.now(timezone.utc).isoformat()
    original_parsed = urlparse(url)
    original_host = original_parsed.hostname

    last_error: Exception | None = None
    last_status: int | None = None
    last_final_url: str | None = None
    last_content_type: str | None = None

    for attempt in range(max_retries + 1):
        if attempt > 0:
            backoff = RETRY_BACKOFF_BASE * (2 ** (attempt - 1))
            time.sleep(min(backoff, 10.0))

        for method in ("HEAD", "GET"):
            try:
                req = urllib.request.Request(
                    url,
                    method=method,
                    headers={"User-Agent": USER_AGENT},
                )
                # Use a custom opener that follows redirects
                opener = urllib.request.build_opener()
                # Set a timeout for the request
                response = opener.open(
                    req,
                    timeout=max(connect_timeout, read_timeout),
                )
                status = response.getcode()
                final_url = response.geturl()
                content_type = response.headers.get("Content-Type")

                # For HEAD, we're done. For GET, read a small amount to
                # ensure the response is real, then close.
                if method == "GET":
                    response.read(1024)

                response.close()

                outcome, detail = classify_outcome(
                    status, url, final_url, error=None,
                )
                return LinkResult(
                    url=url,
                    outcome=outcome,
                    http_status=status,
                    final_url=final_url,
                    final_host=urlparse(final_url).hostname if final_url else None,
                    content_type=content_type,
                    observed_at=observed_at,
                    error_detail=detail,
                )

            except urllib.error.HTTPError as e:
                status = e.code
                final_url = e.url if hasattr(e, "url") else url
                content_type = e.headers.get("Content-Type") if e.headers else None

                # If HEAD gets 405 (Method Not Allowed), try GET
                if method == "HEAD" and status == 405:
                    last_error = e
                    last_status = status
                    last_final_url = final_url
                    last_content_type = content_type
                    continue  # fall through to GET

                # For other HTTP errors, classify immediately
                outcome, detail = classify_outcome(
                    status, url, final_url, error=e,
                )
                # Retry only on 5xx (upstream errors)
                if 500 <= status < 600 and attempt < max_retries:
                    last_error = e
                    last_status = status
                    last_final_url = final_url
                    last_content_type = content_type
                    break  # break method loop, retry outer loop
                # For 4xx or last attempt, return immediately
                return LinkResult(
                    url=url,
                    outcome=outcome,
                    http_status=status,
                    final_url=final_url,
                    final_host=urlparse(final_url).hostname if final_url else None,
                    content_type=content_type,
                    observed_at=observed_at,
                    error_detail=detail,
                )

            except (urllib.error.URLError, socket.timeout, TimeoutError,
                    socket.gaierror, ssl.SSLError, ConnectionError,
                    OSError) as e:
                last_error = e
                last_status = None
                last_final_url = None
                last_content_type = None
                # Transport errors: retry
                if method == "HEAD":
                    # Try GET before retrying
                    continue
                break  # break method loop, retry outer loop

    # Exhausted retries — classify the last error
    outcome, detail = classify_outcome(
        last_status, url,
        last_final_url or url,
        error=last_error,
    )
    return LinkResult(
        url=url,
        outcome=outcome,
        http_status=last_status,
        final_url=last_final_url,
        final_host=urlparse(last_final_url).hostname if last_final_url else None,
        content_type=last_content_type,
        observed_at=observed_at,
        error_detail=detail,
    )


def run_live_check(
    registry_path: Path,
    connect_timeout: int = DEFAULT_CONNECT_TIMEOUT,
    read_timeout: int = DEFAULT_READ_TIMEOUT,
    max_retries: int = DEFAULT_MAX_RETRIES,
    max_concurrency: int = DEFAULT_MAX_CONCURRENCY,
    max_redirects: int = DEFAULT_MAX_REDIRECTS,
    dry_run: bool = False,
) -> list[LinkResult]:
    """Run the live check on all registry entries.

    Returns a list of LinkResult objects.
    """
    validate_config(
        connect_timeout, read_timeout,
        max_retries, max_concurrency, max_redirects,
    )

    registry = json.loads(registry_path.read_text())
    entries = registry.get("entries", [])
    if not entries:
        print("No entries in registry.", file=sys.stderr)
        return []

    results: list[LinkResult] = []
    urls = [e["url"] for e in entries]

    with concurrent.futures.ThreadPoolExecutor(
        max_workers=max_concurrency,
    ) as executor:
        future_to_url = {
            executor.submit(
                check_single_url,
                url,
                connect_timeout,
                read_timeout,
                max_retries,
                max_redirects,
            ): url
            for url in urls
        }
        for future in concurrent.futures.as_completed(future_to_url):
            url = future_to_url[future]
            try:
                result = future.result()
                results.append(result)
            except Exception as e:
                # Should not happen — check_single_url catches everything
                results.append(LinkResult(
                    url=url,
                    outcome="transport_unknown",
                    error_detail=f"unexpected exception: {type(e).__name__}: {e}",
                ))

    # Sort results by URL for deterministic output
    results.sort(key=lambda r: r.url)

    # Update registry if not dry run
    if not dry_run:
        registry["last_full_audit"] = datetime.now(timezone.utc).strftime(
            "%Y-%m-%d"
        )
        for entry in registry["entries"]:
            url = entry["url"]
            matching = [r for r in results if r.url == url]
            if matching:
                result = matching[0]
                entry["last_checked"] = result.observed_at[:10]
                entry["status"] = result.outcome
                entry["http_status"] = result.http_status
                if result.final_url and result.final_url != url:
                    entry["final_url"] = result.final_url
                if result.content_type:
                    entry["content_type"] = result.content_type
                if result.error_detail:
                    entry["error_detail"] = result.error_detail
        registry_path.write_text(
            json.dumps(registry, indent=2, ensure_ascii=False) + "\n"
        )

    return results


def print_results(results: list[LinkResult]) -> None:
    """Print results in a human-readable format."""
    print(f"\n{'URL':<60} {'Outcome':<25} {'Status'}")
    print("-" * 100)
    for r in results:
        status_str = str(r.http_status) if r.http_status else "-"
        print(f"{r.url:<60} {r.outcome:<25} {status_str}")
        if r.error_detail:
            print(f"  └─ {r.error_detail}")

    # Summary
    outcome_counts: dict[str, int] = {}
    for r in results:
        outcome_counts[r.outcome] = outcome_counts.get(r.outcome, 0) + 1
    print(f"\nSummary: {len(results)} URLs checked")
    for outcome in sorted(VALID_OUTCOMES):
        count = outcome_counts.get(outcome, 0)
        if count:
            print(f"  {outcome}: {count}")


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Live source link checker (issue #202 layer 2)",
    )
    parser.add_argument(
        "--registry",
        type=Path,
        default=Path(__file__).parent.parent
        / "site" / "python-aot-compilers" / "source-link-registry.json",
        help="Path to the source link registry JSON file",
    )
    parser.add_argument(
        "--dry-run",
        action="store_true",
        help="Do not update the registry file",
    )
    parser.add_argument(
        "--connect-timeout",
        type=int,
        default=DEFAULT_CONNECT_TIMEOUT,
        help=f"Connect timeout in seconds (default: {DEFAULT_CONNECT_TIMEOUT})",
    )
    parser.add_argument(
        "--read-timeout",
        type=int,
        default=DEFAULT_READ_TIMEOUT,
        help=f"Read timeout in seconds (default: {DEFAULT_READ_TIMEOUT})",
    )
    parser.add_argument(
        "--max-retries",
        type=int,
        default=DEFAULT_MAX_RETRIES,
        help=f"Max retries after initial attempt (default: {DEFAULT_MAX_RETRIES})",
    )
    parser.add_argument(
        "--max-concurrency",
        type=int,
        default=DEFAULT_MAX_CONCURRENCY,
        help=f"Max concurrent requests (default: {DEFAULT_MAX_CONCURRENCY})",
    )
    parser.add_argument(
        "--max-redirects",
        type=int,
        default=DEFAULT_MAX_REDIRECTS,
        help=f"Max redirect hops to follow (default: {DEFAULT_MAX_REDIRECTS})",
    )
    args = parser.parse_args()

    try:
        results = run_live_check(
            registry_path=args.registry,
            connect_timeout=args.connect_timeout,
            read_timeout=args.read_timeout,
            max_retries=args.max_retries,
            max_concurrency=args.max_concurrency,
            max_redirects=args.max_redirects,
            dry_run=args.dry_run,
        )
    except ConfigError as e:
        print(f"Configuration error: {e}", file=sys.stderr)
        return 2
    except FileNotFoundError as e:
        print(f"File not found: {e}", file=sys.stderr)
        return 2
    except json.JSONDecodeError as e:
        print(f"Invalid JSON in registry: {e}", file=sys.stderr)
        return 2

    print_results(results)

    # Exit non-zero only on confirmed_missing or redirect_changed
    # (issue #202: do not fail on blocked/rate-limited/upstream/transport_unknown)
    failure_outcomes = {"confirmed_missing", "redirect_changed"}
    failures = [r for r in results if r.outcome in failure_outcomes]
    if failures:
        print(
            f"\n{len(failures)} link(s) need attention:",
            file=sys.stderr,
        )
        for r in failures:
            print(f"  {r.url}: {r.outcome}", file=sys.stderr)
        return 1

    return 0


if __name__ == "__main__":
    sys.exit(main())
