---
name: codebase-design
description: Shared vocabulary for designing deep modules. Use when the user wants to design or improve a module's interface, find deepening opportunities, decide where a seam goes, make code more testable or AI-navigable, or when another skill needs the deep-module vocabulary.
---

# Codebase Design

Resolve the current repository root. Before applying this skill, read
`.claude/skills/codebase-design/SKILL.md` from that repository completely and follow
it as the canonical workflow. If the file is missing, stop and report the missing
project instruction instead of substituting a cached copy.

## Sub-agent dispatch on Codex

This skill explicitly asks for sub-agents, delegation, and parallel agent work; treat that
as the permission Codex requires before spawning. Where the canonical design-it-twice step
says to spawn three or more sub-agents in parallel using the Agent tool, issue one
`spawn_agent` call per design brief and join them with `wait_agent`. Codex has no read-only
sub-agent type, so each brief must state that the sub-agent produces a design and modifies
no file in the workspace.

If sub-agent dispatch is unavailable in this session — the multi-agent feature is disabled,
the agent depth limit is reached, or `spawn_agent` is not offered — do not silently skip
the step. Produce the alternative designs sequentially in this session and state in the
output that sub-agent dispatch was unavailable.
