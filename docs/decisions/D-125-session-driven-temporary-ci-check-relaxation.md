---
id: D-125
title: "Session-driven temporary CI-check relaxation, narrowly superseding D-024/D-054"
status: accepted
---

## D-125: Session-driven temporary CI-check relaxation, narrowly superseding D-024/D-054

- Status: accepted
- Context: issue #109's D-112 `ci.yml` activation (PR #278) sat blocked on
  a maintainer-only emergency-bypass authorization for an extended period,
  during which every open pull request in the repository -- including two
  entirely unrelated ones (#279, #280) -- showed a failing required
  `audit` check for reasons that had nothing to do with their own diffs.
  `docs/REPOSITORY_GOVERNANCE.md`'s existing manual "Emergency path"
  (D-054/incident #125/PR #119, the only prior use) requires a human
  administrator to personally operate GitHub's UI/API every time, and
  D-054 explicitly states it "grants no reusable permission"; D-024
  states this authority "is not delegated to routine tasks." The
  repository owner decided, during a 2026-08-02 brainstorming session
  (`docs/superpowers/specs/2026-08-02-ci-temporary-bypass-mechanism-design.md`),
  to narrowly supersede that specific stance rather than continue
  absorbing this recurring cost manually.
- Decision: `scripts/manage_ci_bypass.py` and the
  `.claude/skills/ci-temporary-bypass/SKILL.md` /
  `.agents/skills/ci-temporary-bypass/SKILL.md` skill it backs may
  temporarily relax exactly one required status check, using whichever
  session's own authenticated `gh` access invokes them -- no new
  credential is provisioned or stored. Every use requires, in order: a
  fresh, isolated adversarial `Agent()` dispatch (never `advisor()`)
  independently confirming the failure matches an already-documented
  class, reproduces fresh on an unrelated open PR while genuinely
  isolating external state from the motivating PR's own content -- not
  satisfied by a superficial text match; a concurrent D-114/PR #291
  incident on this same repository surfaced a real near-miss the
  mechanism must reject (PR #290's own `validate_policy_successor_transition`
  failure, caused entirely by content #290 itself introduced, would have
  pattern-matched this mechanism's trigger class exactly as convincingly
  as a genuine cross-PR deadlock) -- and has an unambiguous cause read
  directly in the checker's own source; a public
  `[ci-bypass]`-prefixed incident issue created *before* the relaxation,
  containing the pre-relax snapshot, reason, evidence, and an explicit
  expiry; the scoped `PATCH .../protection/required_status_checks` call
  (never the whole-object `PUT .../protection`, which requires every
  field to be specified and risks silently resetting an omitted one);
  and, after the triggering work, a `PATCH` restore, a byte-exact
  readback verification, and a second independent adversarial `Agent()`
  dispatch confirming no other protection field drifted. The mechanism
  refuses to stack -- a new relaxation cannot begin while a `[ci-bypass]`
  incident is already open. The standing autopilot loop
  (`issue-select`/`issue-implement`) is explicitly authorized to invoke
  this mechanism unattended, without a live in-the-moment instruction --
  a deliberate choice the repository owner made after considering and
  rejecting the more conservative attended-only alternative.
- Alternatives: an independent GitHub Actions workflow timer with its own
  permanently-stored admin-scoped repository secret (rejected -- while
  it gives a hard time-bound guarantee closer to D-054's own shell exit
  trap, it requires a new, permanently-held, very-high-privilege
  credential to exist in the repository indefinitely, a standing risk
  the owner chose not to accept). Leaving the manual Emergency path as
  the only path (rejected -- does not address the actual, recurring cost
  that prompted this decision). A fully automated, unattended,
  credential-bearing execution path independent of any session's own
  `gh` access (rejected as out of scope for this decision; explicitly
  not what was built).
- Consequences: `AGENTS.md`'s "Protect main" section gains a new preflight
  rule -- deliberately a probabilistic fail-safe (some future session
  eventually notices and restores via `manage_ci_bypass.py status`), not
  a hard infrastructure time bound, in exchange for introducing no new
  standing secret. `docs/REPOSITORY_GOVERNANCE.md`'s existing manual
  Emergency path is unchanged and remains available for anything this
  narrower mechanism does not cover. `relax()`'s stacking guard
  (`find_open_bypass_issue`) is called once before any work starts and
  again immediately before the mutating `PATCH`, narrowing but not
  eliminating the window where two concurrent sessions could both pass
  the first check before either creates its incident; a residual race
  remains between the second check and the `PATCH` call itself, and
  `relax()` fails closed (aborts before mutating protection, leaving a
  manual-cleanup pointer) if it detects a different incident appeared in
  that window. Accepted, documented follow-ups from the pinned reviewer's
  adversarial passes on PR #303, all P2/P3 and none altering the
  mechanism's core safety properties: `find_open_bypass_issue` and
  `restore_to_baseline`'s stacking guards are deliberately left
  unauthenticated by issue author (unlike `status()`'s live-incident
  suppression check, which is authenticated) -- a forged public issue
  there only makes the tool refuse and escalate to a human, which is
  fail-closed and correct, but it also means an outsider can wedge both
  the mechanism and its own documented repair path (`restore
  --to-baseline`) simultaneously by opening one issue titled
  `[ci-bypass] ...` with a past Expiry; a future session should consider
  a documented human-acknowledgement path that cannot be locked out this
  way. `status()`'s cached `trusted_login` uses `None` as a "not yet
  looked up" sentinel, which could in principle collide with a
  genuinely-`None` authenticated login. `restore()`'s post-PATCH readback
  comparison still includes the four snapshot fields it never PATCHes
  (`enforce_admins`, `required_pull_request_reviews`,
  `required_conversation_resolution`, `allow_force_pushes`/`allow_deletions`),
  so a trusted incident body edited to flip one of them produces a false
  "DRIFT after restore" alarm on a restore that actually succeeded, rather
  than being excluded from that specific comparison. `--expiry-minutes`
  has no upper bound, so an arbitrarily distant expiry defeats the stale-
  incident detector for that long; a sanity cap would be cheap insurance.

