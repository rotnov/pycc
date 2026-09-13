# 2026-09-12-01 — #1021 empty-container element-type inference

## Status

Pull request [#1035](https://github.com/rotnov/pycc/pull/1035) is open against
`main`. The branch was opened from `1ede16ff72bfdc09dc0c469136ba72b5e6dce9cb`
and later merged the default branch's own tip
`f043eb06326f544304d2cef6e38f7495c9d4c12b` (PR #1042, `build --ext`) into
itself, so that commit is the current merge base -- resolve it again before
merging rather than trusting either SHA here, since `strict: true` requires the
branch be up to date at the merge instant. It carries `Fixes #1021`, and the
`closingIssuesReferences` GraphQL query reports `totalCount: 1` naming only
#1021 -- re-run after the last `gh pr edit`. The branch carries the pass itself,
one commit per review round, the retrospective entries below, and this snapshot,
so the authoritative head is whatever `gh pr view 1035` reports once this file
lands; count the commits there rather than here.

Every review thread on the pull request was opened by the
`chatgpt-codex-connector` bot, and every thread opened to date is resolved --
each answered with a reply citing the commit that fixed it. Deliberately no
count is written here: this figure was corrected twice as further rounds
arrived, so read the current one from
`reviewThreads(first:50){totalCount nodes{isResolved}}` instead of trusting a
number in this file. `required_conversation_resolution` is on, so that state is
a merge precondition, alongside the required `audit` and `ci-gate` checks. Those
checks re-run on every push; this snapshot's own commit moves the head, so the
run that gates the merge is the one started after it, not any earlier green
run.

The full local gate set ran green from a single-writer baseline against the
branch's current head, after that default-branch merge. The commits between
the last executable-Rust change and the head carry documentation only: `docs/` files, plus module-level `//!` comments in
`crates/pycc_types/src/empty_container.rs`, which add no instrumentable line
and so leave the changed-line coverage denominator untouched -- re-run the
gate rather than inferring that from this sentence. The coverage diff was
regenerated in the same
invocation that ran the gate: clippy (warnings denied), `cargo test --workspace`,
`cargo llvm-cov`, the `scripts/` unittest suite, both agent validators,
`scripts/check-site.sh`, roadmap evidence, CI permissions, the decisions index
`--check`, decision immutability, and site-pin merge currency all exit 0.
Changed-line coverage is 424 of 424 (100.00%); workspace total is 37700/37734
(99.91%), reported and not enforced. Those figures move with every further
round; re-run the gate rather than citing them.

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
   Its retrospective entry generalizes that one to the class it belongs to --
   a gate's verdict is a property of the code *and* the machine state the gate
   observed -- and names the other two occurrences of the same pattern on this
   branch: a coverage diff generated before the branch's last commits, and a
   wall-clock-sensitive suite run concurrently with `cargo llvm-cov`.

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
