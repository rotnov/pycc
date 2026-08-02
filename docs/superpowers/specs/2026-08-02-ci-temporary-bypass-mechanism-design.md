# Session-driven CI temporary-bypass mechanism — design

## Goal

Give this repository a designed, reusable way to temporarily relax exactly
one stuck required CI check when it is provably failing due to external
repository state (not due to the candidate PR's own diff), without
depending on a human administrator manually operating GitHub's UI/API
every time, while preserving every existing safety property of the manual
"Emergency path" already documented in `docs/REPOSITORY_GOVERNANCE.md`
(public incident, minimal scope, time-bounded, immediately restored,
fully auditable).

## Origin

This design exists because issue #109's D-112 `ci.yml` activation (PR
#278) sat blocked on a maintainer-only "emergency-bypass authorization"
for an extended period, during which every open pull request in the
repository — including two entirely unrelated ones opened this same
session (#279, #280) — showed a failing required `audit` check for
reasons that had nothing to do with their own diffs. The existing manual
Emergency path (`docs/REPOSITORY_GOVERNANCE.md`, one documented use to
date: D-054/incident #125/PR #119) is deliberately not a standing,
reusable permission — D-054 explicitly states "grants no reusable
permission," and D-024 states this authority "is not delegated to
routine tasks." This design is a deliberate, explicit, narrowly-scoped
supersession of that specific stance, decided by the repository owner
during a brainstorming session on 2026-08-02, not a reinterpretation of
the existing decisions.

## Non-negotiable safety properties (carried over from D-024/D-054)

- Every use is publicly auditable via a GitHub issue created before the
  relaxation takes effect.
- Exactly one required check is relaxed per use; every other protection
  control (up-to-date requirement, conversation resolution, force-push
  and deletion prohibitions, `enforce_admins`, any other required check)
  stays enforced throughout.
- The relaxation is restored, and independently verified restored, as
  soon as the triggering work completes.
- The mechanism cannot stack: a new relaxation cannot begin while a prior
  one's incident issue is still open.
- Any failure in verification, execution, or restoration fails closed —
  it never silently proceeds as though the risky step succeeded.

## Scope boundary — what qualifies

A required check qualifies for this mechanism only when **all three**
hold, verified fresh, not assumed:

1. Its exact failure text matches an **already-documented** failure
   class (in `docs/DECISIONS.md` or `docs/AGENT_RETROSPECTIVE.md`). The
   first occurrence of a new failure class can never go through this
   mechanism — it must be understood and documented in a separate
   session first.
2. The same failure **reproduces fresh** (not from a stale cached CI
   result — see the 2026-08-02 `docs/SESSION_LOG.md` entries for why
   staleness matters here) on another open pull request unrelated to the
   one motivating the relaxation.
3. The causal mechanism is read directly in the checker's own source
   (e.g. `scripts/check_ci_permissions.rb`), not inferred from the error
   text alone.

Hard exclusions, never eligible regardless of the above:

- `ci-gate` itself, or any check that reflects the actual build/test/
  coverage result of the PR's own diff.
- Any check where reading the checker's source does not produce an
  unambiguous causal explanation.
- Any check that fails only on the PR motivating the relaxation and not
  on the independent comparison PR — that is evidence of a real, PR-
  specific defect, not external state.
- Any situation where a `[ci-bypass]`-prefixed incident issue is already
  open.

## Components

### 1. Two independent adversarial gates

Both are fresh, isolated `Agent()` dispatches — never the `advisor()`
tool, which shares the calling session's own transcript and could
inherit its framing. Each gate is fail-closed: if it cannot be
dispatched or does not return a clear verdict, that counts as REFUTED /
DRIFT, never as a skipped step.

**Gate 1 (pre-relax, adversarial).** Given only raw evidence — the exact
failure text, the checker's source path, and the other PR to
independently reproduce against — the dispatched agent's explicit brief
is to try to prove the claim false: re-run the reproduction itself
rather than trusting the caller's report, and read the checker code
itself rather than trusting the caller's explanation. Verdict: CONFIRMED
(all three scope-boundary conditions independently verified) or REFUTED
(mechanism does not apply; stop and report through the normal path,
exactly as this session did for issue #243/PR #278 before this design
existed).

**Gate 2 (post-restore verification).** Given the pre-relax protection
snapshot and the post-restore readback, the dispatched agent's brief is
to compare them field by field and flag any drift beyond the one check
that was deliberately relaxed and restored. Verdict: MATCH (incident
issue can close) or DRIFT (release-blocking governance incident,
escalate immediately, issue stays open).

### 2. `scripts/manage_ci_bypass.py`

A testable, `gh`-backed lifecycle script, in the shape of
`scripts/manage_ievo_hooks.py` (capture state → mutate narrowly → verify
→ restore), not a from-scratch ad hoc sequence of raw API calls.

All GitHub interaction is via `gh api`, using whichever session's own
authenticated `gh` access invokes the script — no new credential is
provisioned or stored for this mechanism. Every mutating call uses:

```
gh api -X PATCH repos/OWNER/REPO/branches/main/protection/required_status_checks \
  -f strict=true -f 'contexts[]=...'
```

**Not** the top-level `PUT .../protection` (whole-object replace).
Empirically confirmed during this same session's work (see the
`docs/SESSION_LOG.md` 2026-08-02 "Issue #109 fully resolved" entry): the
scoped `required_status_checks` sub-resource takes `PATCH`, not `PUT` — a
concurrent session on this exact repository misdiagnosed a `PUT`-caused
404 on that path as a repository-level quirk (even testing and ruling
out a Rulesets migration) before finding the correct verb. The scoped
`PATCH` is deliberately preferred over the whole-object `PUT`: the
whole-object form requires every field to be specified explicitly and
is unnecessarily broad for a change that only ever touches the required-
checks list — an omitted field on that path resets that control, a real
risk this mechanism must not carry.

Three subcommands:

- **`status`** — reads current protection, compares against the
  documented baseline (`docs/REPOSITORY_GOVERNANCE.md`'s required-checks
  list), reports drift. No mutation. This is what the new AGENTS.md
  preflight rule (below) calls.
- **`relax --check <context> --reason <text> --evidence <path> [--expiry-minutes N]`**
  (default `N = 60`, matching D-054's "as short as possible while still
  covering one real merge+verify cycle" spirit; the flag exists because a
  single fixed constant cannot fit every future case, but every use must
  state its own chosen value explicitly in the incident issue rather than
  leaving it implicit) — refuses unless the named check is currently
  failing on the motivating PR, and refuses if a `[ci-bypass]`-prefixed
  issue is already open. Snapshots current `required_status_checks`,
  creates the public incident issue (title prefix `[ci-bypass]`, body:
  snapshot, reason, Gate 1's full verdict, evidence PR link, expiry
  timestamp = creation time + `--expiry-minutes`), then issues the scoped
  `PATCH` removing exactly the named check.
- **`restore --incident <issue-number>`** — reads the snapshot back out
  of the incident issue (authoritative; a local ephemeral state file is
  a faster-path cache only, never the sole source), reissues the scoped
  `PATCH` with the full original `contexts` list, reads back the result,
  and exits non-zero on any mismatch rather than silently accepting a
  partial restore. On match, appends the readback and Gate 2's verdict
  to the incident issue and closes it.

Testing: `gh` calls go through one `run_gh()` wrapper, mockable via
`unittest.mock.patch`, matching this repository's existing subprocess-
mocking convention (`scripts/test_manage_ievo_hooks.py`'s `run_manager`
pattern, inverted). Required coverage under D-014: happy path,
check-not-currently-failing refusal, already-open-incident refusal,
API failure at each step (fail-closed, no partial-success continuation),
restore-mismatch (DRIFT) path.

### 3. Fail-safe: an `AGENTS.md` rule, not a standing infrastructure timer

Considered and explicitly rejected: an independent GitHub Actions
workflow timer with its own permanently-stored admin-scoped repository
secret. Technically the strongest time-bound guarantee (closest analog
to D-054's shell exit trap), but it requires a new, permanently-held,
very-high-privilege credential to exist in the repository indefinitely —
a standing risk the repository owner explicitly chose not to accept in
exchange for a hard time bound.

Instead, `AGENTS.md`'s existing "Protect main" section — which already
states "A failed `main-history-audit` run is a release-blocking
governance incident. Open an issue, identify the bypass and actor, and
restore protection before further merges." — gains a directly analogous
rule: every session's D-021 preflight runs `manage_ci_bypass.py status`;
if protection is relaxed and there is no open `[ci-bypass]`-prefixed
issue tracking it (or that issue is open past its own recorded expiry
with no restore recorded), the current session must restore immediately
and treat it as a release-blocking governance incident before any other
work.

This is a probabilistic, not a hard, time bound — restoration depends on
some future session eventually running preflight, not a guaranteed
number of minutes. The repository owner explicitly accepted this
trade-off in exchange for not introducing a new standing high-privilege
secret.

## Documentation updates in the same change

- `docs/DECISIONS.md`: new decision, explicitly superseding D-024's
  "authority... not delegated to routine tasks" and D-054's "grants no
  reusable permission" for this one narrow, fully-gated mechanism only;
  everything else in D-024/D-054 remains unchanged. Alternatives section
  records the rejected independent-timer-plus-secret approach and the
  rejected do-nothing (manual-only) approach.
- `docs/REPOSITORY_GOVERNANCE.md`: new "Session-driven temporary bypass"
  section alongside the existing manual "Emergency path" (which remains,
  unchanged, for cases this mechanism does not cover).
- `AGENTS.md`: the new preflight fail-safe rule (above), plus this
  skill's entry in the Codex/Claude Code dual-discoverability list.
- `docs/SPEC.md`: link if it indexes governance documents.

## New skill

`.claude/skills/ci-temporary-bypass/SKILL.md` (with a thin
`.agents/skills/ci-temporary-bypass/SKILL.md` Codex entrypoint pointing
at it, matching the `issue-select`/`issue-implement` pattern) orchestrates
the end-to-end sequence: detect a candidate failure → verify scope
boundary → Gate 1 → `relax` → (triggering work proceeds) → `restore` →
Gate 2 → close. Not registered as an "alpha skill" with `evals.json` —
this is a deterministic procedure, not an LLM-response-evaluated one.

## Explicitly out of scope

- Rulesets-based branch protection (this repository confirmed, via
  `gh api repos/rotnov/pycc/rulesets`, that it uses classic Branch
  Protection, not GitHub Rulesets; this design does not attempt to
  support both).
- Any check outside the three-condition scope boundary above.
- A fully automated, credential-bearing, unattended (non-interactive-
  session) execution path — this mechanism only works when a session
  with the owner's own authenticated `gh` access is actively running it.

## Decision — standing autopilot authorization

Resolved explicitly by the repository owner (2026-08-02, same
brainstorming session): the standing autopilot loop
(`issue-select`/`issue-implement`, `docs/DECISIONS.md`'s own autopilot
directive) **is** authorized to invoke `ci-temporary-bypass` on its own
initiative, unattended, when it hits a qualifying stuck check — no
in-the-moment owner instruction is required for a given invocation. This
was a deliberate choice, not a default: the owner considered and rejected
the more conservative alternative (attended-only, autopilot always stops
and reports instead, matching its behavior today for issue #243/PR #278
before this mechanism existed).

This raises the real-world stakes of the two adversarial gates
(Components §1) and the `AGENTS.md` fail-safe rule (Components §3): with
no live human necessarily watching a given invocation, the gates are the
*only* check standing between a genuine external-state failure and an
agent incorrectly relaxing protection for a real, PR-specific defect, and
the fail-safe rule's "some future session eventually runs preflight" bound
may in practice mean a longer unattended window than in an attended
session. Both should be implemented and tested to the corresponding
higher standard — this is not a place to cut corners because "the owner
will probably notice" applies less than it does for a live session.
