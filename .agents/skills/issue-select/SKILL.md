---
name: issue-select
description: Use this alpha project skill when the user wants the next GitHub issue chosen autonomously for end-to-end implementation — "pick the next issue", "what should we take next", a standing autopilot directive over the tracker, or an issue-implement run with no issue named. Inventory the full open list against the refreshed default branch and open pull requests, exclude issues whose execution would need maintainer-only authority or decisions, verify the top candidate's premise still reproduces, challenge the pick with an independent adversarial advisor instead of asking the user, and hand the selected issue to issue-implement with a written justification.
---

# issue-select (Alpha)

Resolve the current repository root. Before applying this skill, read
`.claude/skills/issue-select/SKILL.md` from that repository completely and
follow it as the canonical workflow. If the file is missing, stop and report
the missing project instruction instead of substituting a cached copy.

Two of that workflow's gates are arithmetic and easy to skip by accident, so
they are named here as well: step 2 enforces D-192's ceiling of 20 open
non-milestone issues, and step 5 enforces D-192's 4:1 quota — at most one
non-milestone merge in every five. Both are counted from the repository's
actual state with the commands the canonical file specifies, never estimated,
and a candidate declined by either is reported with the count that declined it.
