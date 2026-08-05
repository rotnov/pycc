---
name: process-error-postmortem
description: Use when the agent catches itself having made a process mistake (wasted meaningful time, produced a wrong intermediate result, violated a convention, used the wrong tool for the job) or the user points one out. Diagnose the root cause, identify which existing artifact (a skill's SKILL.md, AGENTS.md, an ADR, or the absence of one) failed to prevent it, and either patch that artifact directly or propose the patch — then record the entry in docs/AGENT_RETROSPECTIVE.md and, if the lesson is durable, promote it into the owning artifact per the existing AGENTS.md rule. Do not use for code bugs (those belong in issues and tests), ambiguous design calls (those belong in docs/DECISIONS.md), or routine debugging that self-corrected within the same turn with no lasting effect.
---

# Process-error postmortem

Resolve the current repository root. Before applying this skill, read
`.claude/skills/process-error-postmortem/SKILL.md` from that repository completely and
follow it as the canonical workflow. If the file is missing, stop and report the
missing project instruction instead of substituting a cached copy.
