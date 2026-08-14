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


def run_fixture_with_location(
    status: int,
    location: str,
    *,
    max_time: int = 5,
) -> tuple[subprocess.CompletedProcess[str], list[dict]]:
    """Run the notifier against a server that sends a redirect with Location."""
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
            try:
                self.send_response(status)
                self.send_header("Location", location)
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


def run_multi_attempt_fixture(
    attempt_responses: list[str],
    *,
    retry_count: int = 1,
    max_time: int = 5,
) -> tuple[subprocess.CompletedProcess[str], list[dict]]:
    """Run the notifier against a server that gives different responses per attempt.

    Each entry in attempt_responses is either an HTTP status string like
    "200" or "202", or the literal "close" to indicate the server reads
    the body and closes the connection without sending a response.
    """
    records: list[dict] = []
    attempt_idx = [0]

    class Handler(BaseHTTPRequestHandler):
        def do_POST(self) -> None:
            length = int(self.headers.get("Content-Length", "0"))
            body = self.rfile.read(length)
            idx = attempt_idx[0]
            attempt_idx[0] += 1
            records.append(
                {
                    "attempt": idx + 1,
                    "path": self.path,
                    "content_type": self.headers.get("Content-Type"),
                    "body": body,
                }
            )
            if idx < len(attempt_responses):
                response = attempt_responses[idx]
            else:
                response = "close"
            if response == "close":
                # Read the body then close without sending a response.
                # This simulates response loss after payload transmission.
                return
            try:
                self.send_response(int(response))
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
            INDEXNOW_RETRY_COUNT=str(retry_count),
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
            timeout=30,
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


def parse_result_line(stdout: str) -> dict[str, str]:
    """Extract the 'IndexNow result:' line from notifier stdout and parse it."""
    for line in stdout.strip().splitlines():
        if line.startswith("IndexNow result:"):
            fields_str = line[len("IndexNow result:") :].strip()
            fields: dict[str, str] = {}
            for token in fields_str.split():
                if "=" in token:
                    k, v = token.split("=", 1)
                    fields[k] = v
            return fields
    raise AssertionError(
        f"no 'IndexNow result:' line in notifier stdout:\n{stdout!r}"
    )


REQUIRED_RESULT_FIELDS = [
    "timestamp",
    "endpoint_host",
    "method",
    "url_count",
    "sitemap_sha256",
    "deployed_commit",
    "http_status",
    "accepted_class",
    "failure_class",
    "delivery_state",
    "attempt_count",
    "possible_duplicate_delivery",
    "retries",
    "curl_exit",
]


def assert_result_record(
    result: subprocess.CompletedProcess[str],
    *,
    expected_http_status: str,
    expected_accepted_class: str,
    expected_failure_class: str,
    expected_delivery_state: str | None = None,
) -> dict[str, str]:
    """Verify the result record is present and has all required fields."""
    fields = parse_result_line(result.stdout)
    for field in REQUIRED_RESULT_FIELDS:
        if field not in fields:
            raise AssertionError(
                f"result record missing field '{field}': {fields!r}"
            )
    if fields["http_status"] != expected_http_status:
        raise AssertionError(
            f"expected http_status={expected_http_status!r}, "
            f"got {fields['http_status']!r}"
        )
    if fields["accepted_class"] != expected_accepted_class:
        raise AssertionError(
            f"expected accepted_class={expected_accepted_class!r}, "
            f"got {fields['accepted_class']!r}"
        )
    if fields["failure_class"] != expected_failure_class:
        raise AssertionError(
            f"expected failure_class={expected_failure_class!r}, "
            f"got {fields['failure_class']!r}"
        )
    if expected_delivery_state is not None:
        if fields["delivery_state"] != expected_delivery_state:
            raise AssertionError(
                f"expected delivery_state={expected_delivery_state!r}, "
                f"got {fields['delivery_state']!r}"
            )
    if fields["method"] != "POST":
        raise AssertionError(f"expected method=POST, got {fields['method']!r}")
    if not fields["timestamp"]:
        raise AssertionError("timestamp is empty")
    if not fields["endpoint_host"]:
        raise AssertionError("endpoint_host is empty")
    if not fields["sitemap_sha256"]:
        raise AssertionError("sitemap_sha256 is empty")
    if not fields["url_count"] or fields["url_count"] == "0":
        raise AssertionError(f"url_count is invalid: {fields['url_count']!r}")
    if not fields["attempt_count"] or fields["attempt_count"] == "0":
        raise AssertionError(
            f"attempt_count is invalid: {fields['attempt_count']!r}"
        )
    if fields["possible_duplicate_delivery"] not in ("true", "false"):
        raise AssertionError(
            f"possible_duplicate_delivery must be true or false, "
            f"got {fields['possible_duplicate_delivery']!r}"
        )
    return fields


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
    # --- Success paths: 200 and 202 must produce distinct accepted_class ---
    success_200, records_200 = run_fixture(200)
    if success_200.returncode != 0:
        raise AssertionError(
            f"IndexNow notifier failed against HTTP 200:\n{success_200.stderr}"
        )
    assert_request(records_200)
    assert_result_record(
        success_200,
        expected_http_status="200",
        expected_accepted_class="submitted",
        expected_failure_class="none",
        expected_delivery_state="submitted",
    )

    success_202, records_202 = run_fixture(202)
    if success_202.returncode != 0:
        raise AssertionError(
            f"IndexNow notifier failed against HTTP 202:\n{success_202.stderr}"
        )
    assert_request(records_202)
    assert_result_record(
        success_202,
        expected_http_status="202",
        expected_accepted_class="key_validation_pending",
        expected_failure_class="none",
        expected_delivery_state="key_validation_pending",
    )

    # Negative control: 200 and 202 must NOT produce the same accepted_class.
    fields_200 = parse_result_line(success_200.stdout)
    fields_202 = parse_result_line(success_202.stdout)
    if fields_200["accepted_class"] == fields_202["accepted_class"]:
        raise AssertionError(
            "200 and 202 collapsed into the same accepted_class: "
            f"{fields_200['accepted_class']!r}"
        )

    # --- Failure paths: each must produce a sanitized failure_class ---
    failure_503, failure_503_records = run_fixture(503)
    if failure_503.returncode == 0:
        raise AssertionError("IndexNow notifier accepted an HTTP 503 response")
    assert_request(failure_503_records)
    assert_result_record(
        failure_503,
        expected_http_status="503",
        expected_accepted_class="failed",
        expected_failure_class="http_error_503",
        expected_delivery_state="http_rejected",
    )

    redirect, redirect_records = run_fixture(302)
    if redirect.returncode == 0:
        raise AssertionError("IndexNow notifier accepted a terminal HTTP 302")
    assert_request(redirect_records)
    assert_result_record(
        redirect,
        expected_http_status="302",
        expected_accepted_class="failed",
        expected_failure_class="redirect_rejected",
        expected_delivery_state="redirect_rejected",
    )

    # Timeout: curl exits non-zero with no HTTP response code.
    # The body is sent before the server delays, so this may be an
    # ambiguous state: delivery_unknown_after_payload_write (body
    # crossed the wire but no accepted response was received).
    timed_out, timeout_records = run_fixture(
        200,
        response_delay=2,
        max_time=1,
    )
    if timed_out.returncode == 0:
        raise AssertionError("IndexNow notifier ignored its maximum runtime")
    assert_request(timeout_records)
    timeout_fields = parse_result_line(timed_out.stdout)
    if timeout_fields["accepted_class"] != "failed":
        raise AssertionError(
            f"timeout accepted_class should be 'failed', "
            f"got {timeout_fields['accepted_class']!r}"
        )
    if timeout_fields["failure_class"] not in ("timeout", "transport_error"):
        raise AssertionError(
            f"timeout failure_class should be 'timeout' or 'transport_error', "
            f"got {timeout_fields['failure_class']!r}"
        )
    if timeout_fields["http_status"] not in ("none", ""):
        raise AssertionError(
            f"timeout http_status should be 'none' or empty, "
            f"got {timeout_fields['http_status']!r}"
        )
    # The body was sent before the timeout, so the delivery state must
    # reflect ambiguous response loss, not a clean pre-body failure.
    if timeout_fields["delivery_state"] not in (
        "delivery_unknown_after_payload_write",
        "failed_before_payload_write",
    ):
        raise AssertionError(
            f"timeout delivery_state should be "
            f"'delivery_unknown_after_payload_write' or "
            f"'failed_before_payload_write', "
            f"got {timeout_fields['delivery_state']!r}"
        )

    # --- Negative control: a successful 200 must emit a result record ---
    # (already verified above via assert_result_record on success_200)

    # --- Negative control: result record must include deployed_commit ---
    fields_200 = parse_result_line(success_200.stdout)
    if "deployed_commit" not in fields_200:
        raise AssertionError("result record missing deployed_commit field")
    # In tests, GITHUB_SHA is not set, so it defaults to "unknown".
    if fields_200["deployed_commit"] != "unknown":
        raise AssertionError(
            f"deployed_commit should default to 'unknown' in tests, "
            f"got {fields_200['deployed_commit']!r}"
        )

    # --- Negative control: GITHUB_SHA is reflected in deployed_commit ---
    records_sha: list[dict] = []

    class HandlerSha(BaseHTTPRequestHandler):
        def do_POST(self) -> None:
            length = int(self.headers.get("Content-Length", "0"))
            records_sha.append({"body": self.rfile.read(length)})
            self.send_response(200)
            self.end_headers()

        def log_message(self, *_a: object) -> None:
            pass

    server_sha = ThreadingHTTPServer(("127.0.0.1", 0), HandlerSha)
    thread_sha = threading.Thread(target=server_sha.serve_forever, daemon=True)
    thread_sha.start()
    try:
        env_sha = clean_env(
            INDEXNOW_ENDPOINT=f"http://127.0.0.1:{server_sha.server_port}/indexnow",
            INDEXNOW_RETRY_COUNT="0",
            INDEXNOW_CONNECT_TIMEOUT_SECONDS="2",
            INDEXNOW_MAX_TIME_SECONDS="5",
            GITHUB_SHA="abc123def456",
        )
        result_sha = subprocess.run(
            [str(NOTIFIER)],
            cwd=REPO_ROOT,
            env=env_sha,
            text=True,
            capture_output=True,
            check=False,
            timeout=10,
        )
    finally:
        server_sha.shutdown()
        server_sha.server_close()
        thread_sha.join(timeout=5)
    if result_sha.returncode != 0:
        raise AssertionError(
            f"notifier failed with GITHUB_SHA set:\n{result_sha.stderr}"
        )
    sha_fields = parse_result_line(result_sha.stdout)
    if sha_fields["deployed_commit"] != "abc123def456":
        raise AssertionError(
            f"deployed_commit should reflect GITHUB_SHA, "
            f"got {sha_fields['deployed_commit']!r}"
        )

    # --- GITHUB_STEP_SUMMARY: verify the summary file is written ---
    summary_dir = tempfile.mkdtemp(prefix="pycc-indexnow-summary-")
    summary_path = Path(summary_dir) / "summary.md"
    records_sum: list[dict] = []

    class HandlerSum(BaseHTTPRequestHandler):
        def do_POST(self) -> None:
            length = int(self.headers.get("Content-Length", "0"))
            records_sum.append({"body": self.rfile.read(length)})
            self.send_response(200)
            self.end_headers()

        def log_message(self, *_a: object) -> None:
            pass

    server_sum = ThreadingHTTPServer(("127.0.0.1", 0), HandlerSum)
    thread_sum = threading.Thread(target=server_sum.serve_forever, daemon=True)
    thread_sum.start()
    try:
        env_sum = clean_env(
            INDEXNOW_ENDPOINT=f"http://127.0.0.1:{server_sum.server_port}/indexnow",
            INDEXNOW_RETRY_COUNT="0",
            INDEXNOW_CONNECT_TIMEOUT_SECONDS="2",
            INDEXNOW_MAX_TIME_SECONDS="5",
            GITHUB_STEP_SUMMARY=str(summary_path),
        )
        result_sum = subprocess.run(
            [str(NOTIFIER)],
            cwd=REPO_ROOT,
            env=env_sum,
            text=True,
            capture_output=True,
            check=False,
            timeout=10,
        )
    finally:
        server_sum.shutdown()
        server_sum.server_close()
        thread_sum.join(timeout=5)
    if result_sum.returncode != 0:
        raise AssertionError(
            f"notifier failed with GITHUB_STEP_SUMMARY set:\n{result_sum.stderr}"
        )
    if not summary_path.exists():
        raise AssertionError("GITHUB_STEP_SUMMARY file was not created")
    summary_content = summary_path.read_text()
    if "IndexNow notification" not in summary_content:
        raise AssertionError(
            f"step summary missing header:\n{summary_content!r}"
        )
    for required in [
        "URL count",
        "Sitemap SHA-256",
        "Deployed commit",
        "HTTP status",
        "Delivery state",
        "Accepted class",
        "Failure class",
        "Attempt count",
        "Possible duplicate delivery",
    ]:
        if required not in summary_content:
            raise AssertionError(
                f"step summary missing '{required}':\n{summary_content!r}"
            )
    if "not" not in summary_content.lower() or "crawl" not in summary_content.lower():
        raise AssertionError(
            f"step summary missing receipt-vs-crawl disclaimer:\n{summary_content!r}"
        )
    import shutil
    shutil.rmtree(summary_dir)

    # --- Redirect handling: 3xx with Location must be fail-closed ---
    # The notifier must NOT follow redirects. A redirected request may
    # not carry the original JSON payload, so a final 200 from a
    # different origin would be a false delivery claim (issue #205).
    for redirect_status in (301, 302, 303, 307, 308):
        redir_result, redir_records = run_fixture_with_location(
            redirect_status,
            f"http://127.0.0.1:1/received",
        )
        if redir_result.returncode == 0:
            raise AssertionError(
                f"IndexNow notifier accepted HTTP {redirect_status} redirect"
            )
        assert_request(redir_records)
        redir_fields = assert_result_record(
            redir_result,
            expected_http_status=str(redirect_status),
            expected_accepted_class="failed",
            expected_failure_class="redirect_rejected",
            expected_delivery_state="redirect_rejected",
        )
        if redir_fields["possible_duplicate_delivery"] != "false":
            raise AssertionError(
                f"redirect should not set possible_duplicate_delivery: "
                f"{redir_fields!r}"
            )

    # --- Ambiguous retry: body sent + no response, then 200 ---
    # The first attempt sends the body but gets no response (connection
    # closed). The second attempt gets 200. The final result must be
    # classified as submitted_after_ambiguous_retry with
    # possible_duplicate_delivery=true, because the first attempt may
    # have been accepted before its response was lost.
    ambig_200, ambig_200_records = run_multi_attempt_fixture(
        ["close", "200"],
        retry_count=1,
    )
    if ambig_200.returncode != 0:
        raise AssertionError(
            f"notifier failed after ambiguous retry + 200:\n{ambig_200.stderr}"
        )
    if len(ambig_200_records) != 2:
        raise AssertionError(
            f"expected 2 requests (ambiguous + retry), "
            f"got {len(ambig_200_records)}"
        )
    # Both attempts must carry the full JSON payload.
    for rec in ambig_200_records:
        if not rec["body"]:
            raise AssertionError(
                f"attempt {rec['attempt']} sent empty body"
            )
        payload = json.loads(rec["body"])
        if "urlList" not in payload:
            raise AssertionError(
                f"attempt {rec['attempt']} body missing urlList"
            )
    ambig_200_fields = parse_result_line(ambig_200.stdout)
    if ambig_200_fields["delivery_state"] != "submitted_after_ambiguous_retry":
        raise AssertionError(
            f"expected delivery_state='submitted_after_ambiguous_retry', "
            f"got {ambig_200_fields['delivery_state']!r}"
        )
    if ambig_200_fields["accepted_class"] != "submitted_after_ambiguous_retry":
        raise AssertionError(
            f"expected accepted_class='submitted_after_ambiguous_retry', "
            f"got {ambig_200_fields['accepted_class']!r}"
        )
    if ambig_200_fields["possible_duplicate_delivery"] != "true":
        raise AssertionError(
            f"expected possible_duplicate_delivery='true', "
            f"got {ambig_200_fields['possible_duplicate_delivery']!r}"
        )
    if ambig_200_fields["attempt_count"] != "2":
        raise AssertionError(
            f"expected attempt_count='2', "
            f"got {ambig_200_fields['attempt_count']!r}"
        )
    if ambig_200_fields["http_status"] != "200":
        raise AssertionError(
            f"expected http_status='200', "
            f"got {ambig_200_fields['http_status']!r}"
        )

    # --- Ambiguous retry: body sent + no response, then 202 ---
    ambig_202, ambig_202_records = run_multi_attempt_fixture(
        ["close", "202"],
        retry_count=1,
    )
    if ambig_202.returncode != 0:
        raise AssertionError(
            f"notifier failed after ambiguous retry + 202:\n{ambig_202.stderr}"
        )
    ambig_202_fields = parse_result_line(ambig_202.stdout)
    if ambig_202_fields["delivery_state"] != (
        "key_validation_pending_after_ambiguous_retry"
    ):
        raise AssertionError(
            f"expected delivery_state="
            f"'key_validation_pending_after_ambiguous_retry', "
            f"got {ambig_202_fields['delivery_state']!r}"
        )
    if ambig_202_fields["possible_duplicate_delivery"] != "true":
        raise AssertionError(
            f"expected possible_duplicate_delivery='true', "
            f"got {ambig_202_fields['possible_duplicate_delivery']!r}"
        )

    # --- All attempts lose response after body sent ---
    # Every attempt sends the body but gets no response. The delivery
    # state must be delivery_unknown_after_payload_write, not a clean
    # failure. The notifier must exit non-zero.
    all_lost, all_lost_records = run_multi_attempt_fixture(
        ["close", "close"],
        retry_count=1,
    )
    if all_lost.returncode == 0:
        raise AssertionError(
            "notifier accepted all-response-loss as success"
        )
    if len(all_lost_records) != 2:
        raise AssertionError(
            f"expected 2 requests, got {len(all_lost_records)}"
        )
    all_lost_fields = parse_result_line(all_lost.stdout)
    if all_lost_fields["delivery_state"] != (
        "delivery_unknown_after_payload_write"
    ):
        raise AssertionError(
            f"expected delivery_state="
            f"'delivery_unknown_after_payload_write', "
            f"got {all_lost_fields['delivery_state']!r}"
        )
    if all_lost_fields["accepted_class"] != "failed":
        raise AssertionError(
            f"expected accepted_class='failed', "
            f"got {all_lost_fields['accepted_class']!r}"
        )

    # --- Direct 200 with no ambiguity must NOT set possible_duplicate ---
    direct_200_fields = parse_result_line(success_200.stdout)
    if direct_200_fields["possible_duplicate_delivery"] != "false":
        raise AssertionError(
            f"direct 200 should have possible_duplicate_delivery='false', "
            f"got {direct_200_fields['possible_duplicate_delivery']!r}"
        )
    if direct_200_fields["delivery_state"] != "submitted":
        raise AssertionError(
            f"direct 200 should have delivery_state='submitted', "
            f"got {direct_200_fields['delivery_state']!r}"
        )
    if direct_200_fields["attempt_count"] != "1":
        raise AssertionError(
            f"direct 200 should have attempt_count='1', "
            f"got {direct_200_fields['attempt_count']!r}"
        )

    # --- Direct 202 with no ambiguity must NOT set possible_duplicate ---
    direct_202_fields = parse_result_line(success_202.stdout)
    if direct_202_fields["possible_duplicate_delivery"] != "false":
        raise AssertionError(
            f"direct 202 should have possible_duplicate_delivery='false', "
            f"got {direct_202_fields['possible_duplicate_delivery']!r}"
        )
    if direct_202_fields["delivery_state"] != "key_validation_pending":
        raise AssertionError(
            f"direct 202 should have delivery_state='key_validation_pending', "
            f"got {direct_202_fields['delivery_state']!r}"
        )

    # --- Mutation control: 200 and 202 after ambiguity must be distinct ---
    if ambig_200_fields["delivery_state"] == ambig_202_fields["delivery_state"]:
        raise AssertionError(
            "200 and 202 after ambiguous retry collapsed into the same "
            f"delivery_state: {ambig_200_fields['delivery_state']!r}"
        )

    # --- Mutation control: ambiguous and direct must be distinct ---
    if ambig_200_fields["delivery_state"] == direct_200_fields["delivery_state"]:
        raise AssertionError(
            "ambiguous and direct 200 collapsed into the same "
            f"delivery_state: {ambig_200_fields['delivery_state']!r}"
        )

    # --- Environment validation ---
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

    # --- Sitemap validation ---
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
