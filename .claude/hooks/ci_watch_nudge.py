#!/usr/bin/env python3
"""PreToolUse hook: nudge ad-hoc `gh pr` polling toward ci-watch.sh.

Local-only (wired from gitignored .claude/settings.local.json per D-023).
Fires on Bash calls that look like manual PR/CI polling (`gh pr view`,
`gh pr checks`, `gh pr list` used repeatedly to watch state) and reminds the
agent that .claude/skills/gha-watch-ci-pr/scripts/ci-watch.sh exists for
exactly this, run via `Monitor`. Does not block the call -- a single status
check is legitimate; this is a reminder against building an ad-hoc poll loop
in its place.
"""
import json
import re
import sys

PATTERN = re.compile(r"\bgh\s+pr\s+(view|checks|list)\b")


def main() -> int:
    try:
        payload = json.load(sys.stdin)
    except (json.JSONDecodeError, ValueError):
        return 0

    if payload.get("tool_name") != "Bash":
        return 0

    command = payload.get("tool_input", {}).get("command", "")
    if not PATTERN.search(command):
        return 0

    reminder = (
        "Reminder: for repeated PR/CI status polling, prefer "
        ".claude/skills/gha-watch-ci-pr/scripts/ci-watch.sh via Monitor "
        "instead of hand-rolled gh pr view/checks loops -- it exits on "
        "terminal state (MERGED/CLOSED/CONFLICTS/STALE/CHECK FAILED/BLOCKED/"
        "READY) so the session isn't left polling by hand."
    )
    # A non-blocking PreToolUse hook's stderr is not fed back to the model
    # on a successful (exit 0) run -- only a blocking hook's stderr is.
    # Deliver the reminder through the documented advisory channel instead:
    # stdout JSON with hookSpecificOutput.additionalContext.
    json.dump(
        {
            "hookSpecificOutput": {
                "hookEventName": "PreToolUse",
                "additionalContext": reminder,
            }
        },
        sys.stdout,
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
