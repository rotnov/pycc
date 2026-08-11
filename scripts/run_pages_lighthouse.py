#!/usr/bin/env python3
"""Lighthouse runner with a pinned, documented profile for issue #229.

Runs Lighthouse 12.8.2 against each canonical page served by the hermetic
local server (``serve_pages_fixture.py``).  The runner is deterministic and
hermetic: it makes no requests to the deployed site or any third-party
origin.  Five replicates per page are collected, all are preserved, and the
checker (``check_pages_performance_budget.rb``) selects the median and
enforces thresholds.  The runner does not retry-until-green.

Pinned profile
--------------
* **Lighthouse version**: 12.8.2 (matching #199's baseline)
* **Chrome/Chromium**: ``--headless --no-sandbox --disable-gpu``;
  GitHub-hosted ``ubuntu-latest`` has Chrome pre-installed.
* **Viewport**: mobile 412 × 823 CSS px, DPR 1.75
* **CPU/network throttling**: Lighthouse mobile preset
* **Cache state**: cold (clean profile each run)
* **Invocation flags**: ``--only-categories=performance --form-factor=mobile``
* **Replicate strategy**: 5 replicates per page, preserve all, select
  median, fail closed on missing/partial, no retry-until-green.
* **No requests** to the deployed site or third-party origins.

Usage::

    python3 scripts/run_pages_lighthouse.py \\
        --manifest tests/fixtures/pages-performance-manifest.json \\
        --output-dir /tmp/lighthouse-results \\
        --site-dir site

The runner starts ``serve_pages_fixture.py`` internally, runs Lighthouse
for each page × replicate, and writes the LHR JSON output to
``<output-dir>/<page-id>/replicate-<N>.json``.
"""

from __future__ import annotations

import argparse
import json
import os
import shutil
import subprocess
import sys
import time
from pathlib import Path
from typing import Any

LIGHTHOUSE_VERSION = "12.8.2"
REPLICATE_COUNT = 5
CHROME_FLAGS = "--headless --no-sandbox --disable-gpu"
LIGHTHOUSE_FLAGS = [
    "--only-categories=performance",
    "--form-factor=mobile",
    "--screenEmulation.mobile=true",
    "--screenEmulation.width=412",
    "--screenEmulation.height=823",
    "--screenEmulation.deviceScaleFactor=1.75",
    "--throttling-method=simulate",
    "--output=json",
    "--quiet",
]


def load_manifest(manifest_path: str) -> dict[str, Any]:
    path = Path(manifest_path)
    if not path.is_file():
        raise SystemExit(f"manifest not found: {manifest_path}")
    return json.loads(path.read_text())


def start_server(site_dir: str) -> tuple[subprocess.Popen, int]:
    """Start the hermetic server and return (process, port)."""
    proc = subprocess.Popen(
        [
            sys.executable,
            str(Path(__file__).parent / "serve_pages_fixture.py"),
            "--port", "0",
            "--site-dir", site_dir,
        ],
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
    )
    port_line = proc.stdout.readline().strip()
    if not port_line or not port_line.isdigit():
        stderr = proc.stderr.read()
        proc.terminate()
        proc.wait()
        raise SystemExit(
            f"could not determine server port (stdout={port_line!r}, stderr={stderr!r})"
        )
    return proc, int(port_line)


def run_lighthouse(
    url: str,
    output_path: Path,
    chrome_path: str,
) -> int:
    """Run one Lighthouse replicate and write the LHR JSON to *output_path*.

    Returns the subprocess exit code.
    """
    cmd = [
        "npx", "--yes", f"lighthouse@{LIGHTHOUSE_VERSION}",
        url,
        *LIGHTHOUSE_FLAGS,
        f"--chrome-flags={CHROME_FLAGS}",
        f"--chrome-path={chrome_path}",
        f"--output-path={output_path}",
    ]
    result = subprocess.run(cmd, capture_output=True, text=True)
    return result.returncode


def find_chrome() -> str:
    """Locate a Chrome/Chromium binary on the current system."""
    candidates = [
        os.environ.get("CHROME_PATH", ""),
        "/usr/bin/google-chrome",
        "/usr/bin/google-chrome-stable",
        "/usr/bin/chromium",
        "/usr/bin/chromium-browser",
        "/snap/bin/chromium",
        "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
        "/Applications/Chromium.app/Contents/MacOS/Chromium",
    ]
    for candidate in candidates:
        if candidate and Path(candidate).exists():
            return candidate
    # Fall back to letting Lighthouse find it.
    return ""


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        description="Lighthouse runner with a pinned profile for the pages performance budget gate."
    )
    parser.add_argument(
        "--manifest",
        default="tests/fixtures/pages-performance-manifest.json",
        help="Path to the page-identity manifest JSON.",
    )
    parser.add_argument(
        "--output-dir",
        required=True,
        help="Directory to write LHR JSON results.",
    )
    parser.add_argument(
        "--site-dir",
        default="site",
        help="Directory containing the site tree.",
    )
    parser.add_argument(
        "--chrome-path",
        default="",
        help="Path to Chrome/Chromium binary (auto-detected if omitted).",
    )
    args = parser.parse_args(argv)

    manifest = load_manifest(args.manifest)
    output_root = Path(args.output_dir)
    output_root.mkdir(parents=True, exist_ok=True)

    chrome_path = args.chrome_path or find_chrome()

    server_proc, port = start_server(args.site_dir)
    base_url = f"http://127.0.0.1:{port}"

    try:
        for page in manifest["canonical_pages"]:
            page_id = page["id"]
            local_route = page["local_route"]
            url = f"{base_url}{local_route.lstrip('/')}"
            # Ensure trailing slash for directory-index routes.
            if not url.endswith("/"):
                url += "/"

            page_dir = output_root / page_id
            page_dir.mkdir(parents=True, exist_ok=True)

            for replicate in range(1, REPLICATE_COUNT + 1):
                output_path = page_dir / f"replicate-{replicate}.json"
                print(
                    f"Running Lighthouse for {page_id} "
                    f"replicate {replicate}/{REPLICATE_COUNT}..."
                )
                sys.stdout.flush()
                rc = run_lighthouse(url, output_path, chrome_path)
                if rc != 0:
                    print(
                        f"FAIL: Lighthouse exited {rc} for {page_id} "
                        f"replicate {replicate}",
                        file=sys.stderr,
                    )
                    return 1
                if not output_path.is_file():
                    print(
                        f"FAIL: Lighthouse output not written for {page_id} "
                        f"replicate {replicate}",
                        file=sys.stderr,
                    )
                    return 1

        print("OK: all Lighthouse replicates collected")
        return 0
    finally:
        server_proc.terminate()
        try:
            server_proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            server_proc.kill()
            server_proc.wait()


if __name__ == "__main__":
    raise SystemExit(main())
