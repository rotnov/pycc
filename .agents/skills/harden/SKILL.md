---
name: harden
description: Use when a failure has just happened — the user points out a mistake, a check fails, or you realize an assumption was wrong — and it should not happen again. Traces the failure to the artefact that owns it, picks an artefact type by how the failure is detectable, builds it, and proves it with the arena before it ships.
---

# harden (wrapper)

Resolve the current repository root. Before applying this skill, read
`.claude/skills/harden/SKILL.md` from that repository completely and follow
it as the canonical workflow. If the file is missing, stop and report the
missing project instruction instead of substituting a cached copy.
