---
name: autopilot-async-monitoring
description: Use when driving the pycc autonomous-delivery autopilot loop (writing-plans -> subagent-driven-development chains, PR/CI monitoring, background Agent orchestration) and deciding how to wait on async state such as a pull request, a CI run, or a dispatched background agent. Covers checking real state before waiting, monitoring only currently-active work, and never letting a dispatched orchestrator "stop and wait" for its own sub-dispatch.
---

# autopilot-async-monitoring

Resolve the current repository root. Before applying this skill, read
`.claude/skills/autopilot-async-monitoring/SKILL.md` from that repository
completely and follow it as the canonical workflow. If the file is missing,
stop and report the missing project instruction instead of substituting a
cached copy.
