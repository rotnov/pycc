# Incident: stalled-subagent-indefinite-block

**Date:** 2026-08-14
**Topic:** stalled-subagent-indefinite-block
**Verdict:** shipped (manual verify)

## Symptom

While processing the SEO issue list, a background subagent dispatched for issue #195 (agent_id `d0018c9f`) stalled. The main session called `read_subagent` with `block=true, timeout=600` over 50 times sequentially, each returning "still running", without ever abandoning the wait or moving on to other pending work. The session burned several hours blocking on a single stalled agent while issues #198, #200, #202, #204, #208, and #209 remained unstarted.

## Gap type

**absence** — no existing rule covered subagent wait bounds or stalled-agent abandonment. The "Autonomous agent operation" section discussed dispatching subagents for context conservation but said nothing about waiting on their completion.

## Termination point

`AGENTS.md` → "Autonomous agent operation" section.

## Artefact

New bullet rule added at line 16 of `AGENTS.md`:

> Bound waits on dispatched background subagents. After three consecutive blocking polls that return "still running" without progress, abandon the wait, kill the stalled agent, and either dispatch a replacement or continue with other pending work. Do not block indefinitely on a single stalled subagent while other tasks remain pending.

## Fixture

Not applicable — this is a process/behaviour rule, not a static gate. The rule is verified by inspecting future agent behavior: a session that blocks beyond three consecutive "still running" responses violates the rule.

## Verify

`verify: manual` — the rule is a behavioral bound on agent decision-making, not a command with a binary exit code. Compliance is checked by observing that future sessions do not exceed three consecutive blocking polls on a single stalled subagent.

## Sweep result

N/A — the rule governs agent behavior, not repository file content.

## Diff

Added one bullet to `AGENTS.md` line 16, immediately after the existing subagent dispatch guidance in the "Autonomous agent operation" section. No existing rules removed or modified.
