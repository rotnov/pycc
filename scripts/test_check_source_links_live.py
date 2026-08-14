#!/usr/bin/env python3
"""Hermetic tests for scripts/check_source_links_live.py (issue #202).

These tests use a local HTTP server fixture to exercise the live link
checker's classification, retry, redirect, timeout, and GET-fallback
logic without contacting any external network.

The tests cover the issue #202 validation expectations:
  - 200 → healthy
  - 404/410 → confirmed_missing
  - 403/429 → blocked_or_rate_limited
  - 500 → upstream_error
  - timeout → transport_unknown
  - redirect to different host → redirect_changed
  - HEAD failure with GET fallback → healthy
  - unbounded configuration is rejected
  - 403/429/timeout is NOT classified as confirmed_missing
  - 404/410 is NOT classified as transport_unknown
"""

from __future__ import annotations

import http.server
import json
import os
import socket
import sys
import threading
import time
import unittest
from pathlib import Path
from unittest.mock import patch

# Add scripts directory to path for imports
SCRIPTS_DIR = Path(__file__).parent
sys.path.insert(0, str(SCRIPTS_DIR))

import check_source_links_live as checker


class TestConfigValidation(unittest.TestCase):
    """Test that unbounded configurations are rejected (issue #202)."""

    def test_rejects_zero_connect_timeout(self):
        with self.assertRaises(checker.ConfigError):
            checker.validate_config(0, 15, 2, 3, 5)

    def test_rejects_unbounded_connect_timeout(self):
        with self.assertRaises(checker.ConfigError):
            checker.validate_config(120, 15, 2, 3, 5)

    def test_rejects_zero_read_timeout(self):
        with self.assertRaises(checker.ConfigError):
            checker.validate_config(10, 0, 2, 3, 5)

    def test_rejects_unbounded_read_timeout(self):
        with self.assertRaises(checker.ConfigError):
            checker.validate_config(10, 120, 2, 3, 5)

    def test_rejects_negative_retries(self):
        with self.assertRaises(checker.ConfigError):
            checker.validate_config(10, 15, -1, 3, 5)

    def test_rejects_unbounded_retries(self):
        with self.assertRaises(checker.ConfigError):
            checker.validate_config(10, 15, 10, 3, 5)

    def test_rejects_zero_concurrency(self):
        with self.assertRaises(checker.ConfigError):
            checker.validate_config(10, 15, 2, 0, 5)

    def test_rejects_unbounded_concurrency(self):
        with self.assertRaises(checker.ConfigError):
            checker.validate_config(10, 15, 2, 20, 5)

    def test_rejects_zero_redirects(self):
        with self.assertRaises(checker.ConfigError):
            checker.validate_config(10, 15, 2, 3, 0)

    def test_rejects_unbounded_redirects(self):
        with self.assertRaises(checker.ConfigError):
            checker.validate_config(10, 15, 2, 3, 20)

    def test_accepts_valid_config(self):
        # Should not raise
        checker.validate_config(10, 15, 2, 3, 5)


class TestClassification(unittest.TestCase):
    """Test the classify_outcome function directly."""

    def test_200_healthy(self):
        outcome, detail = checker.classify_outcome(
            200, "https://example.com/", "https://example.com/",
        )
        self.assertEqual(outcome, "healthy")
        self.assertIsNone(detail)

    def test_201_healthy(self):
        outcome, _ = checker.classify_outcome(
            201, "https://example.com/", "https://example.com/",
        )
        self.assertEqual(outcome, "healthy")

    def test_204_healthy(self):
        outcome, _ = checker.classify_outcome(
            204, "https://example.com/", "https://example.com/",
        )
        self.assertEqual(outcome, "healthy")

    def test_404_confirmed_missing(self):
        outcome, detail = checker.classify_outcome(
            404, "https://example.com/missing", "https://example.com/missing",
        )
        self.assertEqual(outcome, "confirmed_missing")
        self.assertIn("404", detail)

    def test_410_confirmed_missing(self):
        outcome, _ = checker.classify_outcome(
            410, "https://example.com/gone", "https://example.com/gone",
        )
        self.assertEqual(outcome, "confirmed_missing")

    def test_403_blocked_not_missing(self):
        """403 must NOT be classified as confirmed_missing (issue #202)."""
        outcome, _ = checker.classify_outcome(
            403, "https://example.com/", "https://example.com/",
        )
        self.assertEqual(outcome, "blocked_or_rate_limited")
        self.assertNotEqual(outcome, "confirmed_missing")

    def test_429_blocked_not_missing(self):
        """429 must NOT be classified as confirmed_missing (issue #202)."""
        outcome, _ = checker.classify_outcome(
            429, "https://example.com/", "https://example.com/",
        )
        self.assertEqual(outcome, "blocked_or_rate_limited")

    def test_401_blocked(self):
        outcome, _ = checker.classify_outcome(
            401, "https://example.com/", "https://example.com/",
        )
        self.assertEqual(outcome, "blocked_or_rate_limited")

    def test_500_upstream_error(self):
        outcome, _ = checker.classify_outcome(
            500, "https://example.com/", "https://example.com/",
        )
        self.assertEqual(outcome, "upstream_error")

    def test_503_upstream_error(self):
        outcome, _ = checker.classify_outcome(
            503, "https://example.com/", "https://example.com/",
        )
        self.assertEqual(outcome, "upstream_error")

    def test_timeout_transport_unknown(self):
        """Timeout must NOT be classified as confirmed_missing (issue #202)."""
        error = socket.timeout("timed out")
        outcome, detail = checker.classify_outcome(
            None, "https://example.com/", "https://example.com/", error=error,
        )
        self.assertEqual(outcome, "transport_unknown")
        self.assertNotEqual(outcome, "confirmed_missing")

    def test_dns_error_transport_unknown(self):
        error = socket.gaierror("DNS lookup failed")
        outcome, _ = checker.classify_outcome(
            None, "https://example.com/", "https://example.com/", error=error,
        )
        self.assertEqual(outcome, "transport_unknown")

    def test_connection_error_transport_unknown(self):
        error = ConnectionRefusedError("Connection refused")
        outcome, _ = checker.classify_outcome(
            None, "https://example.com/", "https://example.com/", error=error,
        )
        self.assertEqual(outcome, "transport_unknown")

    def test_redirect_different_host(self):
        """Redirect to a different host must be redirect_changed, not healthy."""
        outcome, detail = checker.classify_outcome(
            200,
            "https://old.example.com/page",
            "https://new.example.com/page",
        )
        self.assertEqual(outcome, "redirect_changed")
        self.assertIn("old.example.com", detail)
        self.assertIn("new.example.com", detail)

    def test_redirect_same_host(self):
        """Redirect to same host but different path is still redirect_changed."""
        outcome, _ = checker.classify_outcome(
            200,
            "https://example.com/old",
            "https://example.com/new",
        )
        self.assertEqual(outcome, "redirect_changed")

    def test_404_not_transport_unknown(self):
        """404 must NOT be classified as transport_unknown (issue #202)."""
        outcome, _ = checker.classify_outcome(
            404, "https://example.com/missing", "https://example.com/missing",
        )
        self.assertEqual(outcome, "confirmed_missing")
        self.assertNotEqual(outcome, "transport_unknown")


class LocalHTTPServer:
    """A local HTTP server fixture for hermetic testing."""

    def __init__(self, handler_class):
        self.handler_class = handler_class
        self.server: http.server.HTTPServer | None = None
        self.thread: threading.Thread | None = None
        self.port: int = 0

    def __enter__(self):
        # Bind to port 0 to get a free port
        self.server = http.server.HTTPServer(
            ("127.0.0.1", 0), self.handler_class,
        )
        self.port = self.server.server_address[1]
        self.thread = threading.Thread(
            target=self.server.serve_forever,
            daemon=True,
        )
        self.thread.start()
        return self

    def __exit__(self, *args):
        if self.server:
            self.server.shutdown()
            self.server.server_close()
        if self.thread:
            self.thread.join(timeout=5)

    @property
    def base_url(self) -> str:
        return f"http://127.0.0.1:{self.port}"


class HealthyHandler(http.server.BaseHTTPRequestHandler):
    """Returns 200 for all requests."""

    def log_message(self, format, *args):
        pass  # suppress log output

    def do_HEAD(self):
        self.send_response(200)
        self.send_header("Content-Type", "text/html")
        self.end_headers()

    def do_GET(self):
        self.send_response(200)
        self.send_header("Content-Type", "text/html")
        self.end_headers()
        self.wfile.write(b"<html><body>OK</body></html>")


class NotFoundHandler(http.server.BaseHTTPRequestHandler):
    """Returns 404 for all requests."""

    def log_message(self, format, *args):
        pass

    def do_HEAD(self):
        self.send_response(404)
        self.end_headers()

    def do_GET(self):
        self.send_response(404)
        self.end_headers()


class GoneHandler(http.server.BaseHTTPRequestHandler):
    """Returns 410 for all requests."""

    def log_message(self, format, *args):
        pass

    def do_HEAD(self):
        self.send_response(410)
        self.end_headers()

    def do_GET(self):
        self.send_response(410)
        self.end_headers()


class ForbiddenHandler(http.server.BaseHTTPRequestHandler):
    """Returns 403 for all requests."""

    def log_message(self, format, *args):
        pass

    def do_HEAD(self):
        self.send_response(403)
        self.end_headers()

    def do_GET(self):
        self.send_response(403)
        self.end_headers()


class RateLimitedHandler(http.server.BaseHTTPRequestHandler):
    """Returns 429 for all requests."""

    def log_message(self, format, *args):
        pass

    def do_HEAD(self):
        self.send_response(429)
        self.end_headers()

    def do_GET(self):
        self.send_response(429)
        self.end_headers()


class ServerErrorHandler(http.server.BaseHTTPRequestHandler):
    """Returns 500 for all requests."""

    def log_message(self, format, *args):
        pass

    def do_HEAD(self):
        self.send_response(500)
        self.end_headers()

    def do_GET(self):
        self.send_response(500)
        self.end_headers()


class HeadMethodNotAllowedHandler(http.server.BaseHTTPRequestHandler):
    """Returns 405 for HEAD, 200 for GET — tests GET fallback."""

    def log_message(self, format, *args):
        pass

    def do_HEAD(self):
        self.send_response(405)
        self.send_header("Allow", "GET")
        self.end_headers()

    def do_GET(self):
        self.send_response(200)
        self.send_header("Content-Type", "text/html")
        self.end_headers()
        self.wfile.write(b"<html><body>OK</body></html>")


class RedirectHandler(http.server.BaseHTTPRequestHandler):
    """Redirects to a different host (simulated via a different path)."""

    def log_message(self, format, *args):
        pass

    def do_HEAD(self):
        self.send_response(302)
        # Redirect to a different port (simulating different host)
        # We'll use a second server for the redirect target
        target = getattr(self, "_redirect_target", "http://127.0.0.1:1/redirected")
        self.send_header("Location", target)
        self.end_headers()

    def do_GET(self):
        self.do_HEAD()


class SlowHandler(http.server.BaseHTTPRequestHandler):
    """Hangs indefinitely to trigger timeout."""

    def log_message(self, format, *args):
        pass

    def do_HEAD(self):
        time.sleep(30)  # longer than any test timeout
        self.send_response(200)
        self.end_headers()

    def do_GET(self):
        self.do_HEAD()


class TestLiveCheckWithFixture(unittest.TestCase):
    """Test the live checker against a local HTTP fixture."""

    def test_healthy_url(self):
        with LocalHTTPServer(HealthyHandler) as server:
            url = f"{server.base_url}/"
            result = checker.check_single_url(
                url, connect_timeout=5, read_timeout=5,
                max_retries=0, max_redirects=5,
            )
            self.assertEqual(result.outcome, "healthy")
            self.assertEqual(result.http_status, 200)

    def test_404_confirmed_missing(self):
        with LocalHTTPServer(NotFoundHandler) as server:
            url = f"{server.base_url}/missing"
            result = checker.check_single_url(
                url, connect_timeout=5, read_timeout=5,
                max_retries=0, max_redirects=5,
            )
            self.assertEqual(result.outcome, "confirmed_missing")
            self.assertEqual(result.http_status, 404)

    def test_410_confirmed_missing(self):
        with LocalHTTPServer(GoneHandler) as server:
            url = f"{server.base_url}/gone"
            result = checker.check_single_url(
                url, connect_timeout=5, read_timeout=5,
                max_retries=0, max_redirects=5,
            )
            self.assertEqual(result.outcome, "confirmed_missing")
            self.assertEqual(result.http_status, 410)

    def test_403_blocked_not_missing(self):
        """403 must be blocked_or_rate_limited, not confirmed_missing."""
        with LocalHTTPServer(ForbiddenHandler) as server:
            url = f"{server.base_url}/forbidden"
            result = checker.check_single_url(
                url, connect_timeout=5, read_timeout=5,
                max_retries=0, max_redirects=5,
            )
            self.assertEqual(result.outcome, "blocked_or_rate_limited")
            self.assertEqual(result.http_status, 403)

    def test_429_blocked_not_missing(self):
        """429 must be blocked_or_rate_limited, not confirmed_missing."""
        with LocalHTTPServer(RateLimitedHandler) as server:
            url = f"{server.base_url}/rate-limited"
            result = checker.check_single_url(
                url, connect_timeout=5, read_timeout=5,
                max_retries=0, max_redirects=5,
            )
            self.assertEqual(result.outcome, "blocked_or_rate_limited")
            self.assertEqual(result.http_status, 429)

    def test_500_upstream_error(self):
        with LocalHTTPServer(ServerErrorHandler) as server:
            url = f"{server.base_url}/error"
            result = checker.check_single_url(
                url, connect_timeout=5, read_timeout=5,
                max_retries=0, max_redirects=5,
            )
            self.assertEqual(result.outcome, "upstream_error")
            self.assertEqual(result.http_status, 500)

    def test_head_fallback_to_get(self):
        """HEAD 405 must fall back to GET and return healthy (issue #202)."""
        with LocalHTTPServer(HeadMethodNotAllowedHandler) as server:
            url = f"{server.base_url}/method"
            result = checker.check_single_url(
                url, connect_timeout=5, read_timeout=5,
                max_retries=0, max_redirects=5,
            )
            self.assertEqual(result.outcome, "healthy")
            self.assertEqual(result.http_status, 200)

    def test_timeout_transport_unknown(self):
        """Timeout must be transport_unknown, not confirmed_missing."""
        with LocalHTTPServer(SlowHandler) as server:
            url = f"{server.base_url}/slow"
            result = checker.check_single_url(
                url, connect_timeout=1, read_timeout=1,
                max_retries=0, max_redirects=5,
            )
            self.assertEqual(result.outcome, "transport_unknown")
            self.assertNotEqual(result.outcome, "confirmed_missing")

    def test_redirect_to_different_host(self):
        """Redirect to a different host must be redirect_changed."""
        # Start two servers: one redirects, one is the target
        with LocalHTTPServer(HealthyHandler) as target_server:
            with LocalHTTPServer(RedirectHandler) as redirect_server:
                # Set the redirect target on the handler class
                RedirectHandler._redirect_target = (
                    f"{target_server.base_url}/redirected"
                )
                url = f"{redirect_server.base_url}/redirect"
                result = checker.check_single_url(
                    url, connect_timeout=5, read_timeout=5,
                    max_retries=0, max_redirects=5,
                )
                # The redirect goes to a different port (different "host")
                # urllib follows the redirect, so we get 200 from the target
                # but the final URL differs from the original
                self.assertEqual(result.outcome, "redirect_changed")
                self.assertNotEqual(result.final_url, url)


class TestRunLiveCheck(unittest.TestCase):
    """Test the run_live_check function with a temporary registry."""

    def test_dry_run_does_not_modify_registry(self):
        import tempfile
        with tempfile.TemporaryDirectory() as tmpdir:
            registry_path = Path(tmpdir) / "registry.json"
            registry = {
                "schema": "pycc-source-link-registry/v1",
                "last_full_audit": "2026-01-01",
                "entries": [],
            }
            registry_path.write_text(json.dumps(registry))
            results = checker.run_live_check(
                registry_path, dry_run=True,
            )
            self.assertEqual(results, [])
            # Registry should not be modified
            data = json.loads(registry_path.read_text())
            self.assertEqual(data["last_full_audit"], "2026-01-01")

    def test_run_with_healthy_server(self):
        import tempfile
        with LocalHTTPServer(HealthyHandler) as server:
            with tempfile.TemporaryDirectory() as tmpdir:
                registry_path = Path(tmpdir) / "registry.json"
                url = f"{server.base_url}/"
                registry = {
                    "schema": "pycc-source-link-registry/v1",
                    "last_full_audit": "2026-01-01",
                    "entries": [
                        {
                            "url": url,
                            "source_page": "test.html",
                            "original_href": url,
                            "classification": "external",
                            "fragment": None,
                            "role": "external-factual-source",
                            "last_checked": "2026-01-01",
                            "status": "ok",
                            "http_status": 200,
                        }
                    ],
                }
                registry_path.write_text(json.dumps(registry))
                results = checker.run_live_check(
                    registry_path,
                    connect_timeout=5, read_timeout=5,
                    max_retries=0, max_concurrency=1, max_redirects=5,
                )
                self.assertEqual(len(results), 1)
                self.assertEqual(results[0].outcome, "healthy")
                # Registry should be updated
                data = json.loads(registry_path.read_text())
                self.assertEqual(data["entries"][0]["status"], "healthy")

    def test_unbounded_config_rejected(self):
        import tempfile
        with tempfile.TemporaryDirectory() as tmpdir:
            registry_path = Path(tmpdir) / "registry.json"
            registry_path.write_text('{"entries": []}')
            with self.assertRaises(checker.ConfigError):
                checker.run_live_check(
                    registry_path,
                    connect_timeout=0,
                    read_timeout=5,
                    max_retries=2,
                    max_concurrency=3,
                    max_redirects=5,
                )


class TestMainExitCode(unittest.TestCase):
    """Test that main() returns correct exit codes."""

    def test_exit_0_on_healthy(self):
        import tempfile
        with LocalHTTPServer(HealthyHandler) as server:
            with tempfile.TemporaryDirectory() as tmpdir:
                registry_path = Path(tmpdir) / "registry.json"
                url = f"{server.base_url}/"
                registry = {
                    "schema": "pycc-source-link-registry/v1",
                    "last_full_audit": "2026-01-01",
                    "entries": [
                        {
                            "url": url,
                            "source_page": "test.html",
                            "original_href": url,
                            "classification": "external",
                            "fragment": None,
                            "role": "external-factual-source",
                            "last_checked": "2026-01-01",
                            "status": "ok",
                            "http_status": 200,
                        }
                    ],
                }
                registry_path.write_text(json.dumps(registry))
                with patch("sys.argv", ["check_source_links_live.py",
                                        "--registry", str(registry_path),
                                        "--connect-timeout", "5",
                                        "--read-timeout", "5",
                                        "--max-retries", "0",
                                        "--max-concurrency", "1"]):
                    code = checker.main()
                self.assertEqual(code, 0)

    def test_exit_1_on_confirmed_missing(self):
        import tempfile
        with LocalHTTPServer(NotFoundHandler) as server:
            with tempfile.TemporaryDirectory() as tmpdir:
                registry_path = Path(tmpdir) / "registry.json"
                url = f"{server.base_url}/missing"
                registry = {
                    "schema": "pycc-source-link-registry/v1",
                    "last_full_audit": "2026-01-01",
                    "entries": [
                        {
                            "url": url,
                            "source_page": "test.html",
                            "original_href": url,
                            "classification": "external",
                            "fragment": None,
                            "role": "external-factual-source",
                            "last_checked": "2026-01-01",
                            "status": "ok",
                            "http_status": 200,
                        }
                    ],
                }
                registry_path.write_text(json.dumps(registry))
                with patch("sys.argv", ["check_source_links_live.py",
                                        "--registry", str(registry_path),
                                        "--connect-timeout", "5",
                                        "--read-timeout", "5",
                                        "--max-retries", "0",
                                        "--max-concurrency", "1"]):
                    code = checker.main()
                self.assertEqual(code, 1)

    def test_exit_0_on_blocked_not_missing(self):
        """403 should NOT cause a non-zero exit (issue #202)."""
        import tempfile
        with LocalHTTPServer(ForbiddenHandler) as server:
            with tempfile.TemporaryDirectory() as tmpdir:
                registry_path = Path(tmpdir) / "registry.json"
                url = f"{server.base_url}/forbidden"
                registry = {
                    "schema": "pycc-source-link-registry/v1",
                    "last_full_audit": "2026-01-01",
                    "entries": [
                        {
                            "url": url,
                            "source_page": "test.html",
                            "original_href": url,
                            "classification": "external",
                            "fragment": None,
                            "role": "external-factual-source",
                            "last_checked": "2026-01-01",
                            "status": "ok",
                            "http_status": 200,
                        }
                    ],
                }
                registry_path.write_text(json.dumps(registry))
                with patch("sys.argv", ["check_source_links_live.py",
                                        "--registry", str(registry_path),
                                        "--connect-timeout", "5",
                                        "--read-timeout", "5",
                                        "--max-retries", "0",
                                        "--max-concurrency", "1"]):
                    code = checker.main()
                self.assertEqual(code, 0)


if __name__ == "__main__":
    unittest.main()
