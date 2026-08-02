#!/usr/bin/env python3
"""Session-driven CI temporary-bypass lifecycle management.

See docs/superpowers/specs/2026-08-02-ci-temporary-bypass-mechanism-design.md
and docs/REPOSITORY_GOVERNANCE.md's "Session-driven temporary bypass" section
for the full design and the safety properties this script must preserve.
"""

from __future__ import annotations

import argparse
import json
import subprocess
import sys

REPO = "rotnov/pycc"
BASELINE_CONTEXTS = ["audit", "ci-gate"]
INCIDENT_TITLE_PREFIX = "[ci-bypass]"
DEFAULT_EXPIRY_MINUTES = 60


class CiBypassError(Exception):
    """Raised for any fail-closed condition in this lifecycle."""


def run_gh(args: list[str], input_text: str | None = None) -> str:
    result = subprocess.run(
        ["gh"] + args, input=input_text, capture_output=True, text=True
    )
    if result.returncode != 0:
        raise CiBypassError(
            f"gh {' '.join(args)} failed (exit {result.returncode}): "
            f"{result.stderr.strip()}"
        )
    return result.stdout


def get_protection(repo: str = REPO) -> dict:
    output = run_gh(["api", f"repos/{repo}/branches/main/protection"])
    return json.loads(output)


def required_contexts(protection: dict) -> list[str]:
    return protection["required_status_checks"]["contexts"]


def status(repo: str = REPO) -> tuple[bool, str]:
    protection = get_protection(repo)
    current = sorted(required_contexts(protection))
    baseline = sorted(BASELINE_CONTEXTS)
    if current == baseline:
        return True, f"Branch protection matches baseline: {current}"
    return False, (
        f"Branch protection DRIFT: current required checks {current} "
        f"!= baseline {baseline}"
    )


def main(argv: list[str] | None = None) -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo", default=REPO)
    subparsers = parser.add_subparsers(dest="command", required=True)
    subparsers.add_parser("status")
    args = parser.parse_args(argv)

    try:
        if args.command == "status":
            ok, message = status(args.repo)
            print(message)
            sys.exit(0 if ok else 1)
    except CiBypassError as error:
        print(f"error: {error}", file=sys.stderr)
        sys.exit(1)


if __name__ == "__main__":
    main()
