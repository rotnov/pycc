#!/usr/bin/env python3
"""Stop hook: blocks a reflexive turn-end that dispatches an agent and then
does nothing but announce waiting for its notification, per AGENTS.md's
"Bound waits on dispatched background subagents" rule. Machine-local guard —
see .harden/incidents/stalled-subagent-indefinite-block/2026-08-24-d774821a.md
(that incident folder is machine-local scratch, not committed to this repo;
absent on a fresh checkout by design)."""
import json
import re
import sys

WAIT_PATTERN = re.compile(
    r"(жд[уёе]м?\b|waiting for (its|the) (report|notification|result)|"
    r"wait for the (notification|monitor)|продолжаю ждать)",
    re.IGNORECASE,
)
DISPATCH_TOOLS = {"Agent", "SendMessage", "Workflow"}
VERIFICATION_TOOLS = {
    "Bash", "Read", "Grep", "Glob", "WebFetch", "TaskStop", "Monitor",
    "ListAgents", "mcp__ccd_session_mgmt__list_events",
}


def is_tool_result_entry(entry):
    """True for a transcript entry that is a tool result relayed back as a
    role:"user" message, not a genuine new human turn. Real Claude Code
    transcripts record both a `toolUseResult` field and content blocks of
    type "tool_result" on these entries."""
    if "toolUseResult" in entry:
        return True
    content = entry.get("message", {}).get("content")
    if isinstance(content, list) and content:
        return all(
            isinstance(block, dict) and block.get("type") == "tool_result"
            for block in content
        )
    return False


def collect_current_turn(lines):
    """Walk the transcript backward, collecting every entry belonging to the
    turn that just ended. Stops only at a genuine prior human message — a
    tool-result entry (also recorded with role:"user") is part of the same
    turn and must not end the walk, since an assistant message containing a
    tool_use block is always followed by a fresh assistant message after the
    tool_result round-trip."""
    turn_entries = []
    for line in reversed(lines):
        line = line.strip()
        if not line:
            continue
        try:
            entry = json.loads(line)
        except json.JSONDecodeError:
            continue
        if not isinstance(entry, dict):
            continue
        message = entry.get("message")
        role = message.get("role") if isinstance(message, dict) else None
        if role == "user" and not entry.get("isMeta") and not is_tool_result_entry(entry):
            break
        turn_entries.append(entry)
    turn_entries.reverse()
    return turn_entries


def analyze_turn(turn_entries):
    """Returns (dispatched, verified_after_dispatch, final_text).

    `verified_after_dispatch` only counts a verification-shaped tool call
    that happens strictly after the last dispatch-shaped call in the turn —
    a Read/Grep/Bash call made while deciding whether to dispatch does not
    excuse skipping verification after the dispatch actually happens.
    """
    tool_calls = []
    final_text = ""
    for entry in turn_entries:
        message = entry.get("message")
        if not isinstance(message, dict) or message.get("role") != "assistant":
            continue
        content = message.get("content")
        if not isinstance(content, list):
            continue
        for block in content:
            if not isinstance(block, dict):
                continue
            block_type = block.get("type")
            if block_type == "tool_use":
                tool_calls.append(block.get("name", ""))
            elif block_type == "text":
                final_text = block.get("text", "")

    last_dispatch_pos = None
    for i, name in enumerate(tool_calls):
        if name in DISPATCH_TOOLS:
            last_dispatch_pos = i

    if last_dispatch_pos is None:
        return False, False, final_text

    verified_after = any(
        name in VERIFICATION_TOOLS for name in tool_calls[last_dispatch_pos + 1:]
    )
    return True, verified_after, final_text


def main():
    try:
        payload = json.load(sys.stdin)
    except (json.JSONDecodeError, ValueError):
        sys.exit(0)
    if not isinstance(payload, dict):
        sys.exit(0)
    if payload.get("stop_hook_active"):
        # Already re-entered once for this stop; never loop forever.
        sys.exit(0)

    transcript_path = payload.get("transcript_path")
    if not transcript_path:
        sys.exit(0)

    try:
        with open(transcript_path, "r", encoding="utf-8") as fh:
            lines = fh.readlines()
    except (OSError, UnicodeDecodeError):
        sys.exit(0)

    turn_entries = collect_current_turn(lines)
    dispatched, verified_after_dispatch, final_text = analyze_turn(turn_entries)

    if dispatched and not verified_after_dispatch and WAIT_PATTERN.search(final_text):
        print(json.dumps({
            "decision": "block",
            "reason": (
                "AGENTS.md's bound-waits rule: dispatching an agent and then "
                "ending your turn purely to 'wait for a notification' is not "
                "allowed, even for a single first-level dispatch. Before "
                "stopping, either (a) do other genuinely useful synchronous "
                "work right now (verify real state via Bash/gh/git, check "
                "another pending item), or (b) if there is truly nothing "
                "else to do, say so explicitly and explain why this specific "
                "wait is the correct terminal action for this turn instead "
                "of a reflexive default."
            ),
        }))
        sys.exit(0)

    sys.exit(0)


if __name__ == "__main__":
    main()
