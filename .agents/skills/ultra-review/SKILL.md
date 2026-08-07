---
name: ultra-review
description: Use this alpha project skill when the user wants a periodic, evidence-gated codebase review that files prioritized (`P1`/`P2`/`P3`), milestone-scoped GitHub issues for what it finds — "run an ultra review", "do a periodic code review and file issues", a standing recurring-review directive, or a scheduled/automated invocation with no issue named. Reads a GitHub-native checkpoint to review only the diff since the last run, dispatches the pinned D-068 deep-reviewer once, maps its `blocker`/`warning`/`note` findings to `P1`/`P2`/`P3`, deduplicates against already-filed `ultra-review`-labeled issues, and files the rest autonomously within a bounded evidence bar — without a human approving each payload. Does not implement anything itself and does not pick an issue to work (`issue-select`'s job) or plan one (`issue-to-plan`'s job).
---

# ultra-review (Alpha)

Resolve the current repository root. Before applying this skill, read
`.claude/skills/ultra-review/SKILL.md` from that repository completely and
follow it as the canonical workflow. If the file is missing, stop and report
the missing project instruction instead of substituting a cached copy.
