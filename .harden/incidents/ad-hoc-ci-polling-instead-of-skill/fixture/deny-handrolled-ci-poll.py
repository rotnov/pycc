#!/usr/bin/env python3
"""Deny hand-rolled CI polling loops in Bash tool calls.

Installed by /harden (incident topic: ad-hoc-ci-polling-instead-of-skill,
2026-08-29 entry, recurrence #2). A session waiting on CI must use the
repository's ready-made watcher (in pycc:
`.claude/skills/gha-watch-ci-pr/scripts/ci-watch.sh`) run through the
Monitor tool -- never an ad-hoc while/until+sleep poll loop inside a
(background) Bash call, whose silent death wakes nobody (observed cost:
a 13-hour stall on a failed CI run).

Machine-local artefact: wired from ~/.claude/settings.json (PreToolUse,
matcher "Bash"); deliberately not part of any repository (pycc D-023
keeps machine-local hook wiring out of shared settings).

Exit 0 = allow; exit 2 = block, stderr is fed back to the model.
"""

import json
import re
import sys


def main() -> int:
    try:
        data = json.load(sys.stdin)
    except Exception:
        return 0  # never break tool calls on unreadable input
    if data.get("tool_name") != "Bash":
        return 0
    cmd = (data.get("tool_input") or {}).get("command") or ""
    if "ci-watch.sh" in cmd:
        return 0  # the sanctioned watcher itself (or its tests)
    polls_ci = re.search(r"\bgh\s+pr\s+(checks|view)\b", cmd) or re.search(
        r"\bgh\s+(run|api)\s+\S*(watch|runs|check-runs|status)\b", cmd
    )
    loops = re.search(r"\b(while|until)\b", cmd)
    sleeps = re.search(r"\bsleep\b", cmd)
    if polls_ci and loops and sleeps:
        sys.stderr.write(
            "Blocked: hand-rolled CI polling loop. Use the ready-made watcher "
            "through the Monitor tool instead -- in pycc: "
            "`.claude/skills/gha-watch-ci-pr/scripts/ci-watch.sh <repo> <pr-number> ...` "
            "(see the autopilot-async-monitoring skill). If the watcher misbehaves, "
            "fix the watcher script in its own change; do not re-implement its loop "
            "inline. (harden incident: background-child-stop-and-wait)"
        )
        return 2
    return 0


if __name__ == "__main__":
    sys.exit(main())
