---
name: grill-with-docs
description: Explicit invocation only; never select this skill implicitly. A relentless interview to sharpen a plan or design, which also creates docs (ADR's and glossary) as we go.
---

# Grill with Docs

The canonical workflow is explicit-only. Continue only when the current user request
explicitly names `$grill-with-docs`. If this adapter was selected implicitly, stop
without writing files and say that explicit invocation is required.

Resolve the current repository root. Before applying this skill, read
`.claude/skills/grill-with-docs/SKILL.md` from that repository completely and follow
it as the canonical workflow. If the file is missing, stop and report the missing
project instruction instead of substituting a cached copy.
