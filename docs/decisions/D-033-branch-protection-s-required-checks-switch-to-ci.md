---
id: D-033
title: "Branch protection's required-checks switch to `ci-gate` is live"
status: accepted
---

## D-033: Branch protection's required-checks switch to `ci-gate` is live

- Status: accepted
- Context: D-032 added `ci-gate` and proposed switching branch protection's required check from `build-test-coverage` to `ci-gate` once `ci-gate` existed on `main`, specifically to avoid blocking other pull requests mid-flight against a `main` tree without that job. D-024 separately documents protected `main`'s controls and names the required check as `build-test-coverage`, as it stood when that entry was written. A follow-up attempt to note the switch by editing D-024's `Decision` text directly was caught and reverted during review (flagged twice, independently) as violating this log's own rule against editing an accepted decision -- this entry exists specifically to record the fact without touching either D-024 or D-032's original text.
- Decision: after PR #19 (which added `ci-gate`) merged to `main`, branch protection's required status checks were updated via the GitHub API to `["ci-gate", "audit"]`, replacing the direct `build-test-coverage` requirement. `ci-gate` (D-032) transitively requires `build-test-coverage`'s success, so this is not a weakening of coverage enforcement -- it now also enforces the whole Tier-1 matrix through the same required-check slot. `docs/TESTING.md`, `docs/ROADMAP.md`, and `docs/REPOSITORY_GOVERNANCE.md` describe this current state directly; D-024's and D-032's own historical text is left exactly as originally written.
- Alternatives: edit D-024's or D-032's text directly to reflect the current state (rejected -- explicitly against this log's own edit rule, and the specific thing review caught when it was tried).
- Consequences: a reader relying on D-024 alone for the current required-check name needs this entry's context; future required-check changes should follow the same pattern (a new entry recording the fact, not an edit to the entry that first proposed it) rather than repeating this correction.

