---
name: ci-temporary-bypass
description: Use this skill when a required CI check is failing on a pull request for reasons that appear entirely unrelated to that pull request's own diff -- e.g. every open PR shows the same failure simultaneously. Verifies the failure is provably caused by external repository state (not the PR's own defect) through two independent adversarial checks, then temporarily relaxes exactly that one required check via a public, expiry-labeled, auditable incident, and restores it immediately afterward with a second independent verification. Never use it to work around a check that is failing because of the current PR's own content.
---

# ci-temporary-bypass (Alpha)

Temporarily relax exactly one required CI check that is provably stuck due
to external repository state, then restore it -- publicly, narrowly, and
verifiably. This supersedes D-024's "not delegated to routine tasks" and
D-054's "grants no reusable permission" for this one mechanism only, per a
decision recorded in `docs/decisions/D-125-session-driven-temporary-ci-check-relaxation.md`; every other principle in those
decisions (public incident, minimal scope, an expiry-labeled auditable
incident rather than a hard time bound, immediately restored, fully
auditable) still applies without exception.

Full design and rationale:
`docs/superpowers/specs/2026-08-02-ci-temporary-bypass-mechanism-design.md`.
The manual "Emergency path" in `docs/REPOSITORY_GOVERNANCE.md` still exists
separately for anything this skill does not cover (administrator-only,
broader-scope relaxations).

This skill may be invoked by an attended session on the owner's explicit
instruction, or unattended by the standing autopilot loop
(`issue-select`/`issue-implement`) when it independently encounters a
qualifying stuck check -- both are authorized, per the repository owner's
explicit 2026-08-02 decision recorded in the design doc.

## Scope boundary -- read before invoking anything

A required check qualifies **only** when all three hold, verified fresh,
never assumed:

1. Its exact failure text matches an **already-documented** failure class
   in `docs/decisions/README.md` or `docs/AGENT_RETROSPECTIVE.md`. A failure class
   seen for the first time can never go through this skill -- understand
   and document it in a separate session first, then this skill can be used
   for a later recurrence.
2. The same failure **reproduces fresh** (not a stale cached CI result --
   see `docs/sessions/`'s 2026-08-02 entries for why a `pull_request_target`
   check's last recorded result can be stale after the base branch moves)
   on another open pull request that has nothing to do with the one
   motivating this relaxation, **and** the reproduction genuinely isolates
   external state from the motivating PR's own content -- this is not
   satisfied by finding a second failing PR whose error text superficially
   matches. Actively construct and refute the alternative hypothesis "this
   candidate's own diff explains the failure" -- do not merely fail to
   think of it. Concrete near-miss this must reject: a PR proposing a
   genuinely new, not-yet-recognized manifest transition can fail
   `check_ci_permissions.rb`'s `validate_policy_successor_transition` with
   error text that pattern-matches an already-documented class, purely
   because *that PR's own content* introduces a digest or target the base
   checker does not yet recognize -- a correct, single-PR-fixable defect,
   not external state, even though the error text looks identical to a
   genuine cross-PR deadlock. Check whether the specific file(s) implicated
   in the failure are ones the motivating PR itself modifies; if so, the
   failure's cause cannot be external to that PR by construction, and this
   condition fails regardless of what a second PR shows.
3. The causal mechanism is read directly in the checker's own source (e.g.
   `scripts/check_ci_permissions.rb`), not inferred from the error text.

Never eligible, regardless of the above: `ci-gate` itself or any check
reflecting the PR's own build/test/coverage result; any check whose failure
cannot be traced to an unambiguous cause in the checker's source; any check
that fails only on the motivating PR and not on the independent comparison
PR (that is evidence of a real, PR-specific defect); any check whose
failure implicates a file the motivating PR itself modifies, even when a
second PR also currently fails the same check (two independently broken
PRs are not evidence of one shared external cause -- each must be
diagnosed on its own); any check when a `[ci-bypass]`-prefixed incident
issue is already open.

**Even once the named check is relaxed, that does not mean the pull request
is safe to merge.** Before merging, independently re-check the PR's overall
mergeability -- other blockers (an unresolved human or bot review thread,
being behind the base branch, a different failing check) are not addressed
by relaxing one named check, and merging through one of those anyway is
exactly the failure this skill's own incident-issue discipline exists to
prevent. If anything else blocks the merge, stop, restore immediately
(this skill's own Step 4, below), and report -- do not treat the relaxed
check as a green light for the whole PR.

## Workflow

### 1. Gate 1 -- pre-relax, adversarial

Dispatch a fresh, isolated `Agent()` -- never `advisor()`, which shares this
session's own transcript and could inherit its framing. Give it only raw
evidence: the exact failure text, the checker's source file path, the
motivating PR's own number and changed-file list, and the other PR number
to reproduce against. Its explicit brief: try to prove the claim false. It
must re-run the reproduction itself (not trust this session's report),
read the checker code itself (not trust this session's explanation), and
explicitly check whether the failure's implicated file(s) are ones the
*motivating* PR itself modifies -- if so, verdict REFUTED regardless of
what the other PR shows, per the Scope Boundary section's own worked
near-miss. If it cannot be dispatched, or returns anything other than an
unambiguous CONFIRMED verdict on all three scope-boundary conditions,
treat that as REFUTED -- fail closed, do not proceed.

On REFUTED: before accepting the verdict, independently verify any factual
claim about external state that drove the REFUTED decision (issue status,
file existence, command output). Re-run the specific command the subagent
cited and compare. If the subagent's claim does not hold, discard the
REFUTED verdict and re-evaluate the scope-boundary conditions yourself
inline. A fail-closed design that trusts fabricated evidence blocks
legitimate work as effectively as a real failure.

If the claim holds, stop, report through the normal path (exactly as this
repository did for issue #243/PR #278 before this skill existed). Do not
retry Gate 1 for the same claim.

### 2. Relax

On CONFIRMED, write Gate 1's full verdict text to a file, then:

```
python3 scripts/manage_ci_bypass.py relax \
  --check <check-name> --reason "<short reason>" \
  --evidence <path-to-gate-1-verdict> --pr <motivating-pr-number> \
  [--expiry-minutes N]
```

`--expiry-minutes` defaults to 60. State its own chosen value explicitly if
overridden -- the incident issue always shows the effective expiry either
way. This step refuses (exit 1, `CiBypassError`) if the named check is not
currently failing on the given PR, or if a `[ci-bypass]` incident is
already open -- both are stop conditions, not retryable in-place.

### 3. Do the triggering work

Proceed with whatever the relaxation was for (typically: merge the
motivating PR). Before merging, re-verify overall mergeability per the
Scope Boundary section's closing paragraph above -- stop and restore
immediately if anything else blocks it.

### 4. Restore

Immediately after the triggering work completes (successfully or not):

```
python3 scripts/manage_ci_bypass.py restore --incident <issue-number>
```

This reads the snapshot back from the incident issue (authoritative),
`PATCH`es protection back to it, reads back the result, and raises
`CiBypassError` on any mismatch rather than silently closing the incident.

If no tracking incident can be found for a drifted branch protection state
(this skill's own Stop conditions below, or AGENTS.md's D-021 preflight
escalation path, for exactly this case), use the baseline escalation path
instead of guessing an incident number:

```
python3 scripts/manage_ci_bypass.py restore --to-baseline
```

This creates its own `[ci-bypass]`-prefixed forced-restore issue (so the
action stays public and auditable even with nothing to restore from),
`PATCH`es protection directly to the documented baseline, and raises
`CiBypassError` on any readback mismatch exactly like `restore --incident`
does. `--incident` and `--to-baseline` are mutually exclusive.

`--to-baseline` can only repair drift in `required_status_checks`
(`strict`/`contexts`) -- the same narrow scope this whole mechanism is
authorized to mutate; drift in any of the other five protection fields
(`enforce_admins`, `required_pull_request_reviews`,
`required_conversation_resolution`, `allow_force_pushes`,
`allow_deletions`) requires manual administrator action via GitHub's branch
protection settings, and the incident it creates in that case is
deliberately not restorable via `restore --incident` -- close it by hand
once the out-of-scope field(s) are fixed.

### 5. Gate 2 -- post-restore verification

Dispatch a second fresh, isolated `Agent()`. Give it the pre-relax snapshot
and the post-restore readback (both already in `restore`'s own output and
the incident issue's closing comment). Its brief: compare them field by
field and flag any drift beyond the one check that was deliberately
relaxed and restored -- not just the required-checks list, every other
protection field too (`enforce_admins`, `required_pull_request_reviews`,
`required_conversation_resolution`, `allow_force_pushes`, `allow_deletions`).

MATCH: done, incident issue is already closed by `restore`.
DRIFT, or Gate 2 cannot be dispatched: treat as a release-blocking
governance incident -- do not let this pass silently. Reopen the incident
issue with the drift details and escalate; this is not a condition this
skill resolves on its own.

## Stop conditions

- Gate 1 returns REFUTED, or cannot be dispatched.
- `relax` raises `CiBypassError` for any reason.
- Anything other than the named check blocks the actual merge after
  `relax` succeeds (unresolved review thread, behind base, another failing
  check) -- restore immediately, do not merge anyway.
- `restore` raises `CiBypassError` (including DRIFT).
- Gate 2 returns DRIFT, or cannot be dispatched.
- `relax` fails after creating the incident issue but before the `PATCH`
  succeeds (e.g. the network call in between drops, or the TOCTOU re-check
  finds a different incident that appeared concurrently and aborts on
  purpose). Protection was never actually mutated in this case -- the
  mechanism correctly refused to stack or half-apply a relaxation -- but the
  stray incident issue leaves the mechanism wedged (a `[ci-bypass]` issue is
  now open, so a fresh `relax` will refuse to stack) until it is cleaned up.
  The safe remedy is `python3 scripts/manage_ci_bypass.py restore --incident
  <that-issue-number>`: it is safe to run even though no relaxation actually
  took effect, because it `PATCH`es back to the snapshot embedded in that
  same issue (the unchanged, pre-relax protection) and closes it.

Every stop condition above ends with restoring protection (if it was ever
relaxed) and reporting -- never with leaving protection relaxed and moving
on to something else.
