---
id: D-024
title: "Protected main and audited emergency bypass"
status: accepted
---

## D-024: Protected main and audited emergency bypass

- Status: accepted (the "not delegated to routine tasks" clause in "Authority and scope" below is narrowly superseded by [D-125](#d-125-session-driven-temporary-ci-check-relaxation-narrowly-superseding-d-024d-054) for exactly that decision's own session-driven temporary-bypass mechanism — see that decision for the current scope; every other clause here remains in force unchanged)
- Context: commit `0ac9b1d` reached `main` while PR #3 remained open after a timed-out merge request, proving that commit messages and monitoring conventions do not create a review boundary. An AI-authored compiler needs the repository host—not agent intent—to enforce the PR/CI/review path. The repository currently has one maintainer, and GitHub does not count an author's approval of their own pull request.
- Decision: protect `main` for administrators and automation alike. Require an up-to-date pull request, the current `build-test-coverage` status, and resolved conversations, and disallow force pushes/deletion. Set required approving reviews to zero while there is only one maintainer; enable one independent approval, stale-review dismissal, and last-push approval when a second human maintainer is available. A push-triggered audit runs the pre-push checker (or an immutable reviewed bootstrap when that parent predates the checker) and requires every introduced commit to correlate with a merged-main PR whose merge commit arrived in the same push; a historical commit-to-PR association alone is insufficient. Because GitHub loads a push workflow from the pushed revision, the external repository monitor independently verifies the workflow content and expected run. New required checks are added to protection only after they have run successfully on `main`.
- Authority and scope: normal agent/bot credentials may open/update PRs but cannot bypass protection. Repository administrators retain the platform ability to edit protection only for the documented emergency procedure; that authority is not delegated to routine tasks.
- Privacy and failure behavior: the audit publishes only repository-native commit/PR identifiers. A direct or unassociated commit, changed audit workflow, missing expected run, or failed run requires a governance incident and blocks releases until history/protection are reconciled. CI/provider or external-monitor outages delay release rather than weakening requirements.
- Rollback: emergency relaxation is time-bounded, linked to an incident, records before/after settings, and restores protection immediately after recovery. Permanently weakening the rule requires a superseding decision and explicit review.

