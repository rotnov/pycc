---
name: gha-watch-ci-pr
description: Use when driving the pycc autonomous-delivery autopilot loop and deciding how to wait on async state — a pull request, a CI run, or any dispatched `Agent`. Provides `ci-watch.sh` for PR/CI polling and rules for serialization, session identification, and dispatched-agent lifecycles.
---

# gha-watch-ci-pr

Resolve the current repository root. Before applying this skill, read
`.claude/skills/gha-watch-ci-pr/SKILL.md` from that repository completely and
follow it as the canonical workflow. If the file is missing, stop and report
the missing project instruction instead of substituting a cached copy.
