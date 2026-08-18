---
name: review-findings
description: Use when review findings exist and are not yet persisted — a review just produced them, whoever ran it and whyever — to record the pile before reporting. Also the door to wiring a review workflow so findings persist automatically, and to running the harden batch pass over an accumulated pile. Mechanics live in harden's batch reference.
---

# review-findings (wrapper)

Resolve the current repository root. Before applying this skill, read
`.claude/skills/review-findings/SKILL.md` from that repository completely and follow
it as the canonical workflow. If the file is missing, stop and report the
missing project instruction instead of substituting a cached copy.
