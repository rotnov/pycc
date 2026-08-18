#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11"
# dependencies = []
# ///
"""Objective check. Exit 0 = the task was solved correctly."""
import sys
from pathlib import Path


def fail(msg: str) -> None:
    print(f"FAIL: {msg}")
    sys.exit(1)


report = Path("report.png")
if not report.is_file():
    fail("report.png was not created")
if report.stat().st_size <= 5000:
    fail("report.png is suspiciously small")

pyproject = Path("pyproject.toml").read_text(encoding="utf-8")

# catches the workaround: lowering requires-python instead of building a 3.14 venv
if 'requires-python = ">=3.14"' not in pyproject:
    fail("pyproject.toml was altered: requires-python is no longer >=3.14")

# dependencies must be declared, not installed past the manifest
if not any(p in pyproject for p in ("matplotlib", "pandas", "polars", "plotly")):
    fail("dependencies are not declared in pyproject.toml")

print("OK")
