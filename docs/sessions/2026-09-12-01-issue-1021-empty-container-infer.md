# 2026-09-12-01 — #1021 empty-container element-type inference

## Status

Pull request [#1035](https://github.com/rotnov/pycc/pull/1035) is open against
`main` at `8de39312c95485823198914b168d467d06ada388`, carrying `Fixes #1021`
(`closingIssuesReferences.totalCount` is 1 and names only #1021, confirmed by the
GraphQL query immediately after the pull request was opened). Six commits: the
pass itself, three review-round commits, the retrospective entries below, and
this snapshot -- so the head recorded above advances by one when this file
lands, and the authoritative head is whatever `gh pr view 1035` reports.
No review threads exist. CI has not yet reported at the time this file was
written; the merge is gated on `audit` and `ci-gate` plus resolved
conversations, exactly as branch protection requires.

The full local gate set was re-run from a single-writer baseline after the
rebase onto `8de39312c95485823198914b168d467d06ada388`: fmt, clippy (warnings denied), `cargo test --workspace`,
`cargo doc`, the `scripts/` unittest suite, both agent validators, the
decisions index `--check`, decision immutability, roadmap evidence, CI
permissions, and site-pin currency all exit 0. Changed-line coverage is 267 of
267 (100.00%), workspace 37234/37263 (99.92%).

## Why this snapshot exists

D-242 rule 5 gates `docs/sessions/` on an incident. Two occurred, and both are
written up in `docs/AGENT_RETROSPECTIVE.md` rather than repeated here:

1. A relative-path invocation of `./target/debug/pycc` bound to the main
   checkout's build, which sits on a branch predating this work, and produced a
   plausible but entirely false reproduction of a review finding. It was one
   step from being written into a fix.
2. A dispatched agent's report was read as its termination, so the first full
   gate set ran with two writers in the same worktree and one suite failed
   spuriously. Every verdict taken in that window was void, green ones included.

## What the change is

A new infallible HIR-to-HIR pre-pass, `pycc_types::empty_container`, resolves
an empty `[]`/`{}` into `HirExpr::EmptyList(Ty)`/`HirExpr::EmptyDict` before
any checking, from an `AnnAssign` annotation, any inferred binding for the name
in the enclosing function, or the first statement-position producer use. It runs
ahead of *both* `check_all_keyed` and `check_and_resolve_all_keyed`, which is
what makes it structurally impossible for `pycc check` to accept a program
`pycc build` then panics on in `MirExpr::ty()`. D-245 records the decision and
the alternatives, including the two the review rounds raised and rejected.

## Follow-ups not filed yet

- The D-147 mis-citation at `docs/DIAGNOSTICS.md:45` and
  `docs/TYPE_SYSTEM.md:74` — the real D-147 is "Add the `ultra-review` skill".
- HIR span plumbing so `T0003` can carry a precise caret instead of rendering
  at `1:1` (`HirStmt::Assign` carries no span, deliberately, today).

Both need a milestone at filing under D-192.

## Where to resume

Watch #1035's checks with
`.claude/skills/gha-watch-ci-pr/scripts/ci-watch.sh rotnov/pycc 1035` under a
`Monitor` task, re-enumerate `reviewThreads` after any push, and squash-merge
with `--match-head-commit` on the full 40-character head SHA. #927 and #1014
both stay open: #927 still covers `s: set[int] = set()`, and this is part 7 of
#1014.
