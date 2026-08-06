---
id: D-020
title: "Pull-request CI is an untrusted execution boundary"
status: accepted
---

## D-020: Pull-request CI is an untrusted execution boundary

- Status: accepted
- Context: workflows may execute scripts and definitions controlled by a pull request, including same-repository branches. Workflow-level write permissions, OIDC, secrets, environments, or inherited state can therefore expose deployment and repository authority before review. A checker executed only from the pull-request revision cannot be its own trust anchor because that revision can change the checker too.
- Decision: every workflow declares an explicit workflow-level read/none permission baseline. Jobs that execute or consume untrusted pull-request state receive no elevated capability beyond a minimally scoped read-only `GITHUB_TOKEN`. Every privileged job, including one in a push-only or reusable workflow, is isolated behind the exact `push` plus `refs/heads/main` guard. The semantic policy checker provides normal CI feedback, while a separate read-only `pull_request_target` workflow runs the checker from the trusted base commit and downloads only head-revision workflow YAML as non-executable data. The checker also requires the trust-anchor workflow itself to match an independently reviewed SHA-256 allowlist.
- Alternatives: rely on repository permission defaults (rejected because they are mutable and implicit); trust an in-branch grep checker (rejected because YAML spellings bypass text matching and the PR can modify its own checker); grant workflow-level credentials and skip only the deploy step (rejected because credentials exist before step conditions contain their use).
- Consequences: after the one-time manually reviewed bootstrap PR introduces the trust anchor, workflow changes must pass the trusted `Workflow policy` check from their base revision. Cross-job artifacts, caches, outputs, reusable workflows, secrets, and environments remain explicit review boundaries even when the static checker accepts the YAML.

