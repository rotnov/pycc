# 2026-09-12-01 — #1021 empty-container element-type inference

## Status

Pull request [#1035](https://github.com/rotnov/pycc/pull/1035) is open against
`main` at `1ede16ff72bfdc09dc0c469136ba72b5e6dce9cb`, which is both the branch's
merge base and the remote default-branch tip re-resolved immediately before this
file was committed. It carries `Fixes #1021`, and the
`closingIssuesReferences` GraphQL query reports `totalCount: 1` naming only
#1021 -- re-run after the last `gh pr edit`. Seventeen commits: the pass itself,
the review-round fixes, the retrospective entries below, and this snapshot, so
the authoritative head is whatever `gh pr view 1035` reports once this file
lands.

Eight review threads exist, every one opened by the `chatgpt-codex-connector`
bot across seven rounds, and all eight are resolved -- each answered with a reply
citing the commit that fixed it. `required_conversation_resolution` is on, so
that state is a merge precondition, alongside the required `audit` and `ci-gate`
checks. Those checks re-run on every push; this snapshot's own commit moves the
head, so the run that gates the merge is the one started after it, not any
earlier green run.

The full local gate set ran green from a single-writer baseline against
`636a5719`, the last commit that changes Rust sources -- this snapshot is the
only commit after it, and it touches only `docs/`, which the changed-line
coverage gate does not instrument. The coverage diff was regenerated in the same
invocation that ran the gate: clippy (warnings denied), `cargo test --workspace`,
`cargo llvm-cov`, the `scripts/` unittest suite, both agent validators,
`scripts/check-site.sh`, roadmap evidence, CI permissions, the decisions index
`--check`, decision immutability, and site-pin merge currency all exit 0.
Changed-line coverage is 359 of 359 (100.00%); workspace total is 37635/37669
(99.91%), reported and not enforced.

One local flake is worth naming so a future run does not mistake it for a
regression: `test_check_corpus_compile_rate.MaxSecondsTests.test_a_budget_that_expires_before_timing_still_records_the_match`
failed once when the `scripts/` suite ran concurrently with
`cargo llvm-cov --workspace`, and passed in isolation and on a second full run
of the suite. The test asserts on a wall-clock compile budget, so it is
load-sensitive; the file is owned by #1024/#1030 on `main` and is untouched by
this branch.

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
