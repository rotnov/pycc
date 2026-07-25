#!/usr/bin/env python3
"""Exercise the real, non-dry-run IndexNow HTTP path hermetically."""

from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
from pathlib import Path
import os
import subprocess
import tempfile
import threading
import time
import xml.etree.ElementTree as ET


REPO_ROOT = Path(__file__).resolve().parent.parent
NOTIFIER = REPO_ROOT / "scripts" / "notify-indexnow.sh"
SITEMAP = REPO_ROOT / "site" / "sitemap.xml"
KEY = "3361fe03d0f44ab7cdbb1a3ce1461821"
CANONICAL = "https://rotnov.github.io/pycc/"
NAMESPACE = {"s": "http://www.sitemaps.org/schemas/sitemap/0.9"}


def clean_env(**overrides: str) -> dict[str, str]:
    env = {
        "PATH": os.environ.get("PATH", os.defpath),
        "LC_ALL": "C",
        "NO_PROXY": "127.0.0.1,localhost",
        "no_proxy": "127.0.0.1,localhost",
    }
    env.update(overrides)
    return env


def expected_urls() -> list[str]:
    root = ET.parse(SITEMAP).getroot()
    return [
        (node.text or "").strip()
        for node in root.findall("s:url/s:loc", NAMESPACE)
    ]


def run_fixture(
    status: int,
    *,
    response_delay: float = 0,
    max_time: int = 5,
) -> tuple[subprocess.CompletedProcess[str], list[dict]]:
    records: list[dict] = []

    class Handler(BaseHTTPRequestHandler):
        def do_POST(self) -> None:
            length = int(self.headers.get("Content-Length", "0"))
            records.append(
                {
                    "path": self.path,
                    "content_type": self.headers.get("Content-Type"),
                    "body": self.rfile.read(length),
                }
            )
            time.sleep(response_delay)
            try:
                self.send_response(status)
                self.end_headers()
            except BrokenPipeError:
                pass

        def log_message(self, _format: str, *_args: object) -> None:
            pass

    server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        env = clean_env(
            INDEXNOW_ENDPOINT=(
                f"http://127.0.0.1:{server.server_port}/indexnow"
            ),
            INDEXNOW_RETRY_COUNT="0",
            INDEXNOW_CONNECT_TIMEOUT_SECONDS="2",
            INDEXNOW_MAX_TIME_SECONDS=str(max_time),
        )
        result = subprocess.run(
            [str(NOTIFIER)],
            cwd=REPO_ROOT,
            env=env,
            text=True,
            capture_output=True,
            check=False,
            timeout=10,
        )
    finally:
        server.shutdown()
        server.server_close()
        thread.join(timeout=5)
    return result, records


def assert_request(records: list[dict]) -> None:
    if len(records) != 1:
        raise AssertionError(f"expected one IndexNow request, got {len(records)}")
    request = records[0]
    if request["path"] != "/indexnow":
        raise AssertionError(f"unexpected IndexNow path: {request['path']}")
    if request["content_type"] != "application/json; charset=utf-8":
        raise AssertionError(
            f"unexpected IndexNow content type: {request['content_type']}"
        )
    payload = json.loads(request["body"])
    expected = {
        "host": "rotnov.github.io",
        "key": KEY,
        "keyLocation": f"{CANONICAL}{KEY}.txt",
        "urlList": expected_urls(),
    }
    if payload != expected:
        raise AssertionError(
            "IndexNow payload does not match the canonical sitemap set:\n"
            f"expected {expected!r}\nactual   {payload!r}"
        )


def assert_sitemap_xml_rejected(sitemap: str, expected_error: str) -> None:
    with tempfile.TemporaryDirectory(prefix="pycc-indexnow-") as directory:
        path = Path(directory) / "sitemap.xml"
        path.write_text(sitemap)
        env = clean_env(
            INDEXNOW_DRY_RUN="1",
            INDEXNOW_SITEMAP=str(path),
        )
        result = subprocess.run(
            [str(NOTIFIER)],
            cwd=REPO_ROOT,
            env=env,
            text=True,
            capture_output=True,
            check=False,
            timeout=10,
        )
    if result.returncode == 0:
        raise AssertionError("IndexNow notifier accepted an invalid sitemap")
    if expected_error not in result.stderr:
        raise AssertionError(
            f"missing expected notifier error {expected_error!r}: "
            f"{result.stderr!r}"
        )


def assert_sitemap_rejected(urls: list[str], expected_error: str) -> None:
    entries = "".join(f"<url><loc>{url}</loc></url>" for url in urls)
    sitemap = (
        '<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">'
        f"{entries}</urlset>"
    )
    assert_sitemap_xml_rejected(sitemap, expected_error)


def assert_environment_rejected(
    variable: str,
    value: str,
    expected_error: str,
) -> None:
    result = subprocess.run(
        [str(NOTIFIER)],
        cwd=REPO_ROOT,
        env=clean_env(INDEXNOW_DRY_RUN="1", **{variable: value}),
        text=True,
        capture_output=True,
        check=False,
        timeout=10,
    )
    if result.returncode == 0:
        raise AssertionError(
            f"IndexNow notifier accepted {variable}={value!r}"
        )
    if expected_error not in result.stderr:
        raise AssertionError(
            f"missing expected notifier error {expected_error!r}: "
            f"{result.stderr!r}"
        )


def main() -> None:
    for status in (200, 202):
        success, success_records = run_fixture(status)
        if success.returncode != 0:
            raise AssertionError(
                f"IndexNow notifier failed against HTTP {status}:\n"
                f"{success.stderr}"
            )
        assert_request(success_records)

    failure, failure_records = run_fixture(503)
    if failure.returncode == 0:
        raise AssertionError("IndexNow notifier accepted an HTTP 503 response")
    assert_request(failure_records)

    redirect, redirect_records = run_fixture(302)
    if redirect.returncode == 0:
        raise AssertionError("IndexNow notifier accepted a terminal HTTP 302")
    assert_request(redirect_records)

    timed_out, timeout_records = run_fixture(
        200,
        response_delay=2,
        max_time=1,
    )
    if timed_out.returncode == 0:
        raise AssertionError("IndexNow notifier ignored its maximum runtime")
    assert_request(timeout_records)

    assert_environment_rejected(
        "INDEXNOW_RETRY_COUNT",
        "-1",
        "must be a non-negative integer",
    )
    assert_environment_rejected(
        "INDEXNOW_CONNECT_TIMEOUT_SECONDS",
        "0",
        "must be a positive integer",
    )
    assert_environment_rejected(
        "INDEXNOW_MAX_TIME_SECONDS",
        "invalid",
        "must be a positive integer",
    )

    assert_sitemap_rejected([], "contains no canonical URLs")
    assert_sitemap_xml_rejected(
        '<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">'
        "<url>"
        f"<loc>{CANONICAL}</loc>"
        f"<loc>{CANONICAL}unexpected/</loc>"
        "</url>"
        "</urlset>",
        "must contain exactly one loc",
    )
    assert_sitemap_rejected([CANONICAL, CANONICAL], "contains duplicate")
    assert_sitemap_rejected(
        ["https://rotnov.github.io/not-pycc/"],
        "outside the verified project path",
    )
    assert_sitemap_rejected(
        [f"{CANONICAL}status/?preview=1"],
        "outside the verified project path",
    )
    assert_sitemap_rejected(
        [f"{CANONICAL}status/#details"],
        "outside the verified project path",
    )
    assert_sitemap_rejected(
        [f"{CANONICAL}../not-pycc/"],
        "outside the verified project path",
    )
    assert_sitemap_rejected(
        [f"{CANONICAL}%2e%2e/not-pycc/"],
        "outside the verified project path",
    )
    assert_sitemap_rejected(
        [f"{CANONICAL}%252e%252e/not-pycc/"],
        "outside the verified project path",
    )
    assert_sitemap_rejected(
        [f"{CANONICAL}status/%5cnot-pycc/"],
        "outside the verified project path",
    )
    print("IndexNow notifier HTTP tests passed.")


if __name__ == "__main__":
    main()
