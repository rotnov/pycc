#!/usr/bin/env python3
"""Hermetic local HTTP server for the pages performance budget gate.

Serves the checked-in ``site/`` tree under the ``/pycc/`` GitHub Pages
project-path prefix so that ``site/index.html`` is served at ``/pycc/``,
``site/status/index.html`` at ``/pycc/status/``, and so on.  The server
makes no external requests, sets ``Content-Type: text/html; charset=utf-8``
for HTML files, serves ``site/404.html`` with HTTP 404 for non-existent
paths, and prints the chosen port to stdout so the CI job can use it.

Usage::

    python3 scripts/serve_pages_fixture.py [--port PORT] [--site-dir DIR]

``--port 0`` (the default) lets the OS auto-assign a free port.
"""

from __future__ import annotations

import argparse
import http.server
import os
import posixpath
import sys
from pathlib import Path
from typing import Optional

PROJECT_PATH_PREFIX = "/pycc/"
HTML_CONTENT_TYPE = "text/html; charset=utf-8"


class PagesFixtureHandler(http.server.BaseHTTPRequestHandler):
    """Serve files from *site_dir* under the ``/pycc/`` prefix."""

    # The site directory is set as a class attribute by ``serve``.
    site_dir: str = "site"

    # ------------------------------------------------------------------
    # Suppress the default per-request log to keep stdout clean — only
    # the chosen port should appear on stdout for the CI job.
    # ------------------------------------------------------------------
    def log_message(self, fmt: str, *args: object) -> None:  # noqa: D401
        pass

    # ------------------------------------------------------------------
    # Helpers
    # ------------------------------------------------------------------
    def _local_path_for_request(self) -> Optional[Path]:
        """Return the filesystem path for *self.path*, or ``None`` if 404."""
        # Strip query string and fragment.
        url_path = self.path.split("?", 1)[0].split("#", 1)[0]
        # Normalise using posixpath to avoid OS-specific separators.
        relative = posixpath.normpath(url_path)

        # Every route must live under /pycc/.
        if not relative.startswith(PROJECT_PATH_PREFIX) and relative != "/pycc":
            return None

        # Strip the /pycc/ prefix to map into site_dir.
        if relative == "/pycc" or relative == "/pycc/":
            sub = ""
        else:
            sub = relative[len(PROJECT_PATH_PREFIX):]

        # Prevent path traversal: reject any segment containing "..".
        if ".." in sub.split("/"):
            return None

        fs_path = Path(self.site_dir) / sub
        return fs_path

    def _serve_file(self, fs_path: Path, status: int = 200) -> None:
        body = fs_path.read_bytes()
        self.send_response(status)
        self.send_header("Content-Type", HTML_CONTENT_TYPE)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def _serve_404(self) -> None:
        not_found = Path(self.site_dir) / "404.html"
        if not_found.is_file():
            self._serve_file(not_found, status=404)
        else:
            body = b"404 Not Found\n"
            self.send_response(404)
            self.send_header("Content-Type", "text/plain; charset=utf-8")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

    # ------------------------------------------------------------------
    # HTTP methods
    # ------------------------------------------------------------------
    def do_GET(self) -> None:  # noqa: N802
        fs_path = self._local_path_for_request()
        if fs_path is None:
            self._serve_404()
            return

        if fs_path.is_file():
            self._serve_file(fs_path)
        elif fs_path.is_dir():
            index = fs_path / "index.html"
            if index.is_file():
                self._serve_file(index)
            else:
                self._serve_404()
        else:
            self._serve_404()

    def do_HEAD(self) -> None:  # noqa: N802
        fs_path = self._local_path_for_request()
        if fs_path is None:
            self.send_response(404)
            self.end_headers()
            return
        if fs_path.is_file():
            size = fs_path.stat().st_size
            self.send_response(200)
            self.send_header("Content-Type", HTML_CONTENT_TYPE)
            self.send_header("Content-Length", str(size))
            self.end_headers()
        elif fs_path.is_dir() and (fs_path / "index.html").is_file():
            size = (fs_path / "index.html").stat().st_size
            self.send_response(200)
            self.send_header("Content-Type", HTML_CONTENT_TYPE)
            self.send_header("Content-Length", str(size))
            self.end_headers()
        else:
            self.send_response(404)
            self.end_headers()


def serve(site_dir: str, port: int) -> http.server.ThreadingHTTPServer:
    """Start the hermetic server and return the server object.

    The caller is responsible for calling ``serve_forever`` or using
    the server in a ``with`` block.
    """
    site_path = Path(site_dir).resolve()
    if not site_path.is_dir():
        raise SystemExit(f"site directory not found: {site_dir}")

    PagesFixtureHandler.site_dir = str(site_path)

    server = http.server.ThreadingHTTPServer(
        ("127.0.0.1", port), PagesFixtureHandler
    )
    actual_port = server.server_address[1]
    # Print only the port to stdout so the CI job can capture it.
    print(actual_port)
    sys.stdout.flush()
    return server


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        description="Hermetic local server for the pages performance budget gate."
    )
    parser.add_argument(
        "--port", type=int, default=0, help="Port to bind (0 = auto-assign)."
    )
    parser.add_argument(
        "--site-dir", default="site", help="Directory containing the site tree."
    )
    args = parser.parse_args(argv)

    server = serve(args.site_dir, args.port)
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass
    finally:
        server.server_close()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
