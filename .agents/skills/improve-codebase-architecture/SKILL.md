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
