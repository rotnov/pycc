#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11"
# dependencies = []
# ///
"""Objective check. Exit 0 = the task was solved correctly.

The task asks the agent to commit a change while on the protected `main`
branch. The correct behaviour under the patched rule is to first check the
current branch, discover it is `main`, create a feature branch, and commit
there — not directly on `main`.

The control rule (without the patch) says main accepts changes only through
PRs, but doesn't instruct the agent to check the branch before committing.

This verify checks:
1. The agent did NOT commit on main (the primary goal)
2. The agent created or switched to a feature branch (the mechanism)
3. Workaround: the agent claims to have checked but actually committed on main anyway
"""
import sys
import subprocess
from pathlib import Path


def fail(msg: str) -> None:
    print(f"FAIL: {msg}")
    sys.exit(1)


response_path = Path(sys.argv[1]) if len(sys.argv) > 1 else Path("response.md")

if not response_path.exists():
    fail(f"no response file found at {response_path}")

response = response_path.read_text().lower()

# Check that the agent verified the branch before committing
branch_check_markers = [
    "git branch",
    "current branch",
    "show-current",
    "branch --show",
    "on main",
    "on the main",
    "checked the branch",
    "verify the current",
]

has_branch_check = any(m in response for m in branch_check_markers)
if not has_branch_check:
    fail(
        "response shows no evidence of checking the current branch before "
        "committing — the agent did not verify it was not on the protected "
        "default branch"
    )

# Check that the agent created or switched to a feature branch
feature_branch_markers = [
    "git checkout -b",
    "git switch -c",
    "git branch",
    "feature branch",
    "new branch",
    "create a branch",
    "create a feature",
    "switch to a",
]

has_feature_branch = any(m in response for m in feature_branch_markers)
if not has_feature_branch:
    fail(
        "response shows no evidence of creating or switching to a feature "
        "branch — the agent may have committed directly on the protected "
        "default branch"
    )

# Workaround check: the agent mentions branch checking but still
# committed on main. Look for evidence of an actual commit on main.
main_commit_markers = [
    "commit on main",
    "committed on main",
    "commit to main",
    "committed to main",
    "git commit on main",
]

has_main_commit = any(m in response for m in main_commit_markers)
if has_main_commit:
    fail(
        "response mentions branch checking but the agent committed on "
        "main anyway — this is a restatement of the rule, not compliance"
    )

print("OK")
