---
name: improve-codebase-architecture
description: Explicit invocation only; never select this skill implicitly. Scan a codebase for deepening opportunities, present them as a visual HTML report, then grill through whichever one you pick.
---

# Improve Codebase Architecture

The canonical workflow is explicit-only. Continue only when the current user request
explicitly names `$improve-codebase-architecture`. If this adapter was selected
implicitly, stop without writing files and say that explicit invocation is required.

Resolve the current repository root. Before applying this skill, read
`.claude/skills/improve-codebase-architecture/SKILL.md` from that repository
completely and follow it as the canonical workflow. If the file is missing, stop and
report the missing project instruction instead of substituting a cached copy.

## Sub-agent dispatch on Codex

This skill explicitly asks for sub-agents, delegation, and parallel agent work; treat that
as the permission Codex requires before spawning. Where the canonical workflow says to use
the Agent tool with `subagent_type=Explore`, use `spawn_agent` and join with `wait_agent`.
Codex has no read-only sub-agent type, so the spawn brief must impose that discipline
itself: instruct the sub-agent to read and report only and to modify no file in the
workspace. When the canonical workflow routes into the codebase-design skill's
design-it-twice step, issue one `spawn_agent` call per design brief and `wait_agent` on all
of them.

If sub-agent dispatch is unavailable in this session — the multi-agent feature is disabled,
the agent depth limit is reached, or `spawn_agent` is not offered — do not silently skip
the step. Carry out the same exploration inline in this session and state in the output that
sub-agent dispatch was unavailable and the step ran inline.
