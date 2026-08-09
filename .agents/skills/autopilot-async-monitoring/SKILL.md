---
name: autopilot-async-monitoring
description: Use when driving the pycc autonomous-delivery autopilot loop (writing-plans -> subagent-driven-development chains, PR/CI monitoring, background Agent orchestration) and deciding how to wait on async state such as a pull request, a CI run, or any dispatched `Agent` -- a one-off spike/build/benchmark task just as much as a nested sub-dispatch inside a pipeline. Covers checking real state before waiting, monitoring only currently-active work, and never ending a turn to "wait for a notification" while a dispatched agent, or a background child it started, is still live.
---

# autopilot-async-monitoring

Resolve the current repository root. Before applying this skill, read
`.claude/skills/autopilot-async-monitoring/SKILL.md` from that repository
completely and follow it as the canonical workflow. If the file is missing,
stop and report the missing project instruction instead of substituting a
cached copy.
