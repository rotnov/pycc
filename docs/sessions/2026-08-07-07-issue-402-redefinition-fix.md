# 2026-08-07 checkpoint 07 — issue #402 fix implemented, PR #412 opened

## Status

PR [#412](https://github.com/rotnov/pycc/pull/412) opened against `origin/main @
e18dc63`, fixing GitHub issue [#402](https://github.com/rotnov/pycc/issues/402).
Not yet merged — CI is in flight. This continues the `issue-select` →
`issue-implement` autopilot loop from checkpoint
[04](2026-08-07-04-issue-356-merge-issue-402-correction.md), which corrected
#402's own false "second call is dead code" claim and narrowed it to the real
gap, and checkpoint [05](2026-08-07-05-issue-357-reconciled-and-merged.md),
which closed out the unrelated "356 then 357" sequencing (PR #357 merged,
its own session log landed as PR #406).

Note on numbering: this entry claims slot `07`, not `06`, because a
different concurrent local branch (`docs/session-log-2026-08-07-06`, not
pushed at the time this was written) had already claimed
`docs/sessions/2026-08-07-06-issue-401-status-page-freshness-merged.md` for
an unrelated checkpoint (issue #401 / PR #407). This repository's local
clone is shared by multiple concurrent autonomous actors (see the checked-in
worktree list under `.claude/worktrees/`, `.codex/worktrees/`,
`.windsurf/worktrees/`) committing under the same `rotnov` identity — the
same-day sequence number is a local-clone convention, not one this session
can reserve ahead of time, so a same-number collision on an unpushed branch
is expected and resolved by bumping to the next free slot, not by touching
the other branch.

## The bug and the fix

Root cause lived in `crates/pycc_types/src/lib.rs`. `check_incompatible_redefinitions`
skipped any function whose signature still contained `Ty::Infer`, deferring
to a second call site inside `check_and_resolve` that runs after solver
resolution. That post-resolution call could only ever catch redefinitions
whose *resolved arities* differ, because `infer_function_signatures_with_solver`
keys its resolved-signature map by function *name*, not by item — two
same-named, same-arity definitions always converge onto one shared resolved
signature before the post-resolution check runs, so it could never observe a
same-arity, `Ty::Infer`-involving mismatch. Two genuinely incompatible
redefinitions were silently accepted instead of being rejected with `T0021`.

Fix: delete the `Ty::Infer`-skip guard so the check does an unconditional,
full structural `PartialEq` comparison on `Ty` (Rust's derived `PartialEq`
already handles `Ty::Infer` positions correctly), then remove the now-dead
post-resolution recheck inside `check_and_resolve`. Three stale doc comments
that described the gap as open, plus one `docs/ROADMAP.md` caveat, were
updated in the same change.

## Process notes

- `issue-select`'s adversarial round (checkpoint 04/05 era) confirmed #402
  as the sole unblocked P1 with no collision against the two open PRs at
  that time. `issue-to-plan`, dispatched into a fresh `Agent` per D-142/D-143,
  ran 3 review rounds — the third built and ran the actual test fixtures
  against both the unfixed and fixed tree rather than reasoning about them,
  and found the issue's own suggested fixture text doesn't compile through
  the real CLI frontend (an unannotated top-level function needs the `_`
  leading-underscore "private" convention per D-038, or T0001 fires before
  the redefinition check is ever reached) — corrected before publishing.
- Implementation was dispatched into a second fresh `Agent`, working inside
  the same task worktree the D-021 preflight had already created
  (`.claude/worktrees/issue-402-redefinition-fix`). It reported one plan
  deviation: the plan's item (c) asked for a test duplicating item (a)'s own
  fixture through `check()` directly, which was already exactly what (a)
  tested — retargeted at `checked_function_signatures` instead, so each of
  the three call sites of `check_incompatible_redefinitions` (`check`,
  `checked_function_signatures`, `check_and_resolve`) gets independent
  coverage of the issue's fixture.
- `origin/main` advanced twice more while this task was in flight (PR #410's
  successor commits, then PR #411, a homepage-only fix) — both hops were
  measured as disjoint via `git diff --stat`, not assumed. The branch was
  rebased cleanly onto the final tip (`e18dc63`) immediately before the
  pinned-reviewer dispatch, so review and the opened PR both target the
  actual current tree.
- D-068/D-155 pinned `ievo:deep-reviewer`, dispatched fresh by the
  orchestrating session (not reused from the implementer's own internal
  self-review pass) against the post-rebase diff: **0 findings**. It
  independently re-derived the same-name-keyed-map root cause from source
  rather than taking the PR description's claim on faith.

## Gates (all local, pre-push)

`cargo llvm-cov --workspace --fail-under-lines 100 --fail-under-regions 100`
(100.00%/100.00%), `cargo clippy --workspace --all-targets -- -D warnings`
(clean), `cargo test --workspace` (0 failures), `cargo doc --workspace
--no-deps` (clean), `ruby scripts/check_roadmap_evidence.rb` +
`test_check_roadmap_evidence.rb` (pass).

## Where to resume

If this session ends before PR #412 merges: monitor its CI via
`scripts/ci-watch.sh` or `gh pr checks 412 --watch`, re-verify D-078 state
(origin/main unchanged, PR head unchanged, zero unresolved review threads)
immediately before merging, then `gh pr merge 412 --merge --delete-branch`.

Once #412 merges, the next planned step is **not** a fresh `issue-select`
pick: run the `ultra-review` skill (merged via PR #357) over recently-merged
pull requests to look for post-merge defects, per an explicit request from
this session's human principal, before re-entering `issue-select` for the
next autopilot task.
