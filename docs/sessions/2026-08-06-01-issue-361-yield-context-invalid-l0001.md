# 2026-08-06-01: Issue #361 (context-invalid `yield`/`yield from`) implemented, PR open

## Status

Implementation complete, locally gated green, D-068 pinned review clean (2 rounds), pull
request about to open against `origin/main` @ `f9ac131`. Not yet merged.

## What happened

Second iteration of the standing v0.3 autopilot loop (`/next-milestone` → `issue-select` →
`issue-implement`), continuing directly after issue #141 (PR #362) merged. This is #141's own
named follow-up: `yield`/`yield from` used outside a function body were still misclassified as
`C0001` ("valid Python, not implemented yet") instead of the context-invalid `L0001` #141
already established for `break`/`continue`/`async for`.

1. **Selection**: re-inventoried the v0.3 pool after #141 merged. #359/#354 stayed blocked
   (Part 2 of #118/#22, waiting on #360/#358). #142 stayed deprioritized (touches
   `crates/pycc_types/src/lib.rs`, contested by both #360 and #358). #361 — filed by #141's own
   implementer as its scoped-out follow-up — was the clean pick: same priority tier (P2), no
   open-PR collision, premise re-verified empirically (both `yield 1` and
   `yield from [...]` at module scope still produced `C0001` on the post-#141 tree).
2. **issue-to-plan** (dispatched, 3 adversarial rounds): corrected the issue's own framing —
   D-148's "independently parameterized" claim understated the actual blast radius (`lower_expr`
   had zero context parameters and 39 call sites needing a threaded `in_function: bool`, vs.
   `in_loop`'s 11). Investigated the `pycc_types`-vs-`pycc_hir` question on its own merits
   (not by analogy to #141) and confirmed `pycc_hir`-only is still correct: `yield` has no HIR
   representation at all today, so `pycc_types` can't see it. An oracle probe against CPython
   3.14.6 found comprehension-internal `yield` positions need a *third*, scope-independent
   classification — deliberately deferred, preserved via literal `true` at five call sites
   instead of the threaded value.
3. **Implementation** (dispatched): extracted `crates/pycc_hir/src/expr.rs` (mirroring #141's
   `stmt.rs`), threaded `in_function: bool`, added the two new `Expr::Yield`/`Expr::YieldFrom`
   match-guard arms, four new CLI fixtures, new ADR `D-149`. Caught and self-corrected a near-miss
   mid-task (an accidental blanket `cargo fmt --all` reformatting unrelated files — reverted via
   its own advisor consult before committing; final diff is scoped to intended files only, so no
   retrospective-log entry per `AGENT_RETROSPECTIVE.md`'s own "self-corrected within the same
   turn, no lasting effect" exclusion).
4. **Local gates, all green**: `cargo test -p pycc_hir` (212 passed, +6), `cargo test --test
   diagnostics_test` (56 passed, +4), clippy clean, **100.00%** lines/regions coverage
   (including the new `expr.rs`), `cargo doc` clean, roadmap-evidence check passed.
5. **D-068 pinned review** (2 rounds): round 1 found one real but low-severity doc-drift
   finding (the shared `context_invalid()` helper's doc comment still described itself as
   statement-context-only, omitting the new expression-context callers) plus one informational
   note (missing session-log checkpoint, expected at PR-open time, not a defect). Fixed the doc
   comment in a follow-up commit; round 2 confirmed clean.

## Known follow-ups (not blockers for this PR)

- The comprehension-internal `yield` classification is deliberately left at today's
  unconditional `C0001` rather than CPython's real (scope-independent) rule — untracked by any
  issue yet, per the plan's own explicit deferral.
- Same open-PR landscape as #141's own entry: #360/#357 still independently contest ADR
  `D-147` on their own branches (unmerged); this PR's `D-149` was re-verified free of both at
  implementation and pre-PR time.

## Where to resume

If this session ends before the PR merges: task branch `fix/issue-361-yield-context-invalid`
in worktree `.claude/worktrees/issue-361-yield-context`, ahead of `origin/main` (`f9ac131`),
working tree clean, not yet pushed. Push it, open the PR (`Fixes #361`), and resume at
`issue-implement`'s own step 7 (monitor) / step 8 (merge). The standing autopilot directive for
the v0.3 loop continues after this issue merges — re-enter `issue-select` step 1 with a fresh
baseline; do not stop to ask before picking the next issue (see this session's own corrected
behavior after an earlier lapse, recorded as an evolution candidate this same session).
