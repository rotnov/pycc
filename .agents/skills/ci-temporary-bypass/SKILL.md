---
name: ci-temporary-bypass
description: Use this skill when a required CI check is failing on a pull request for reasons that appear entirely unrelated to that pull request's own diff -- e.g. every open PR shows the same failure simultaneously. Verifies the failure is provably caused by external repository state (not the PR's own defect) through two independent adversarial checks, then temporarily relaxes exactly that one required check via a public, expiry-labeled, auditable incident, and restores it immediately afterward with a second independent verification. Never use it to work around a check that is failing because of the current PR's own content.
---

# ci-temporary-bypass (Alpha)

Resolve the current repository root. Before applying this skill, read
`.claude/skills/ci-temporary-bypass/SKILL.md` from that repository
completely and follow it as the canonical workflow. If the file is
missing, stop and report the missing project instruction instead of
substituting a cached copy.
