#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11"
# dependencies = []
# ///
"""Objective check. Exit 0 = the task was solved correctly.

The task asks the agent to create a PR with `gh pr create`. The correct
behaviour under the patched rule is to write the body to a temp file and
use `--body-file`. The control rule (without the patch) gives no such
instruction, so the agent may inline a heredoc in `--body`, which is the
failure mode.

This verify checks:
1. The agent used `--body-file` (the primary goal)
2. The agent did NOT use `--body "$(cat <<'EOF'` (the anti-pattern)
3. Workaround: the agent claims to use --body-file but actually inlines
"""
import sys
from pathlib import Path


def fail(msg: str) -> None:
    print(f"FAIL: {msg}")
    sys.exit(1)


response_path = Path(sys.argv[1]) if len(sys.argv) > 1 else Path("response.md")

if not response_path.exists():
    fail(f"no response file found at {response_path}")

response = response_path.read_text().lower()

# Check that the agent used --body-file
body_file_markers = [
    "--body-file",
    "body-file",
    "body file",
    "temp file",
    "temporary file",
    "/tmp/",
    "write.*body.*file",
]

has_body_file = any(m in response for m in body_file_markers)
if not has_body_file:
    fail(
        "response shows no evidence of using --body-file — the agent "
        "likely inlined the body in the --body argument"
    )

# Check that the agent did NOT use the heredoc anti-pattern
heredoc_markers = [
    "--body \"$(cat <<'eof'",
    "--body \"$(cat <<\"eof\"",
    "--body \"$(cat <<'eof",
    "<<'eof'",
    '<<"eof"',
]

has_heredoc = any(m in response for m in heredoc_markers)
if has_heredoc:
    fail(
        "response uses the heredoc anti-pattern in --body — the rule "
        "explicitly says to use --body-file instead"
    )

print("OK")
