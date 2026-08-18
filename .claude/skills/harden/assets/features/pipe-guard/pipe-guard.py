#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11"
# dependencies = []
# ///
"""Blocks a status-bearing command whose exit code a pipe is about to swallow.

A shell pipeline's status is the status of its LAST command, so
`git commit … | tail -1` reports tail's 0 whatever the commit did. Measured
three times in one day on the same operator: two failed commits and one
failed docker build, each read as success — the third one hours after the
journal entry describing the mechanism was written. A reminder does not
survive contact with habit; this hook fires instead of memory.

PreToolUse hook for Bash. Exit 2 blocks the call and the stderr text reaches
the model with the fix: run it bare, or opt in with `set -o pipefail`, or
read `PIPESTATUS`.
"""
import json
import re
import sys

# Commands whose exit status IS their verdict. Deliberately narrow: piping
# `git log` or `ls` is fine, the guard is for commands someone runs to learn
# whether something worked.
STATUS_RE = re.compile(
    r"\b(git\s+(commit|push|merge|rebase|cherry-pick)"
    r"|docker\s+(build|push)"
    r"|pytest|pre-commit|mypy|tox"
    r"|ruff\s+(check|format)"
    r"|npm\s+(test|ci)|yarn\s+test"
    r"|cargo\s+(build|test|check)"
    r"|go\s+(build|test|vet)"
    r"|make)\b")
OPT_OUT_RE = re.compile(r"set\s+-o\s+pipefail|pipestatus", re.IGNORECASE)


def strip_quotes(command: str) -> str:
    """Quoted text is data, not structure — `git commit -m "a | b"` has no pipe."""
    return re.sub(r"'[^']*'|\"[^\"]*\"", "''", command)


def violates(command: str) -> str | None:
    """The first status-bearing command a pipe would mask, or None.

    Statements are split on `&&`, `||`, `;` and newlines; within each, only
    the segment BEFORE the first `|` is checked — `git commit && git log |
    head` pipes the log, not the commit.
    """
    if OPT_OUT_RE.search(command):
        return None
    bare = strip_quotes(command)
    for statement in re.split(r"&&|\|\||;|\n", bare):
        if "|" not in statement:
            continue
        head = statement.split("|", 1)[0]
        if m := STATUS_RE.search(head):
            return m.group(0)
    return None


def main() -> int:
    try:
        payload = json.load(sys.stdin)
    except ValueError:
        return 0  # an unreadable payload must not block unrelated work
    if payload.get("tool_name") != "Bash":
        return 0
    command = str((payload.get("tool_input") or {}).get("command") or "")
    masked = violates(command)
    if not masked:
        return 0
    print(
        f"pipe-guard: `{masked}` is piped, so its exit code would be replaced "
        "by the last command in the pipeline — this exact mask hid two failed "
        "commits and a failed build in one day. Run it bare and filter its "
        "saved output afterwards, or prefix `set -o pipefail`, or read "
        "PIPESTATUS explicitly.",
        file=sys.stderr)
    return 2


if __name__ == "__main__":
    sys.exit(main())
