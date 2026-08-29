# Session handoff — issue #638: release a bigint temporary on the exception edge

**Date:** 2026-08-29
**Branch:** `feat/issue-638-bigint-exception-release` (5 commits ahead of `origin/main` at `9164a806`)
**Status:** implementation complete, three D-068 review rounds done (two produced real fixes, third clean), ready to open the pull request.

## What this delivers

Adds a `pending_int_releases` codegen-time stack (`crates/pycc_codegen/src/bigint_rc.rs`,
`crates/pycc_codegen/src/exception.rs`'s `guard_statement_effects`) so that when an owning bigint
temporary is abandoned mid-construction because a later sibling sub-expression raises, its birth
reference is released on the exception-unwinding edge instead of leaking. This closes D-181's
residual item 2, recorded as [D-208](../decisions/D-208-release-a-bigint-temporary-on-the-exception-unwinding-edge.md).

Six call sites in `crates/pycc_codegen/src/lib.rs` are now protected: the `range()` preheater
(two push sites), `BinOp`, `Compare`, call-argument transfer, and — added in this session, as the
sixth site — `MirExpr::TupleLiteral`'s element-evaluation loop.

## Commits on this branch (oldest first)

1. `7bd75265` "Materialize out-of-range `int` literals through the runtime (#148)" — pre-existing, unrelated to #638, already on `origin/main` before this branch diverged (listed here only because `git log` on this branch shows it; not part of this PR's diff).
2. The #638 implementation commit(s) establishing the original 4-5 protected sites and the `pending_int_releases` mechanism.
3. `051bb4cc` — round-1 review fixes: replaced a fragile string-split IR-block isolation in a `bigint_rc.rs` test with a CFG-walk (the test could pass vacuously because `guard_statement_effects` creates `effect_exc_cont` before `effect_exc_unwind`, so a naive split silently returned the wrong block); extended D-208/ROADMAP.md to name the range-preheater as a fourth protected site.
4. `de1a01ef` — round-2 finding fix: extended the mechanism to `MirExpr::TupleLiteral` (mark/push/truncate-without-release, mirroring the existing `build_call_to_with_leading_args` pattern), with new IR and peak-RSS tests, both negative-verified (fail without the fix, pass with it). Full `pycc_codegen` suite (397 tests), `issue_638_bigint_exception_release.rs` (9 tests), clippy, `cargo doc`, and the full `llvm-cov --fail-under-lines 100 --fail-under-regions 100` coverage gate all pass at 100.00%/100.00%.

## Review loop (D-068/D-155, `ievo:deep-reviewer`)

- **Round 1:** 2 findings (1 warning, 1 note), both fixed in `051bb4cc`.
- **Round 2:** 4 findings — 1 warning (the TupleLiteral completeness gap, fixed in `de1a01ef`), 2 notes deferred as honestly-scoped non-drift (no RSS oracle for the range-preheater flavor; the new CFG-walk test parser is whole-module rather than per-function, harmless in the current single-function fixture), 1 note (this handoff entry) — now resolved by this file.
- **Round 3:** 0 findings across the full 11-point checklist, including an explicit check for a possible seventh unprotected site (list/dict/set literals — ruled out: `pycc_rt_int_untag_checked` panics synchronously within the same loop iteration rather than raising a catchable exception, so no earlier owning element can ever be orphaned by a later one). Self-reported caveat: this round's reviewer instance had no Bash tool and reconstructed the diff from full current-file reads rather than a literal `git diff`; judged sufficient given two prior real rounds with literal diff review and this session's own independent verification of round 2's finding.

All findings are recorded in `.harden/findings/issue-638.jsonl` (append-only, `chflags uappnd`).

## Judgment calls resolved autonomously (D-127)

Round 2's completeness gap was resolved via the `advisor` tool: extend the fix now (chosen) rather
than defer to a new issue, because (a) D-208's own "closed for every site" claim was already false
without the extension — leaving it would repeat the exact doc-drift class round 1 already fixed;
(b) the fix is a small, mechanical application of an already-proven pattern, not new design; (c)
deferring would cost more (narrowing three docs, filing a new issue, a third review round to
confirm the narrowing) than the fix itself.

Confirmed issue #636 (open, v0.4, "Balance D-182's tuple-literal ingress retain") does **not**
cover the same ground as the round-2 finding: #636 is a *borrowed*-element ingress-retain-without-release
imbalance blocked on D-124's container refcounting; the round-2/round-3 fix is an *owning*-element
exception-edge orphaning during construction. Both are now correctly distinguished in D-208,
`docs/ROADMAP.md`, and `docs/RUNTIME.md`.

## Next step

Open the pull request (`Fixes #638`), monitor CI via `ci-watch.sh`/`Monitor`, and merge once green
— no further plan deviations expected. After #638 merges, file a new, separately-scoped v0.4
GitHub issue for the still-outstanding third leak flavor identified during this issue's planning
(a per-iteration reassigned bigint name leaking when an exception is caught later in the same loop
iteration) — deferred until after this merge to avoid a second-writer collision in this worktree.

## Paused autopilot

- **Directive scope:** open-ended (`/goal fix all opened issues`, standing, no specific milestone or issue named).
- **Active milestone:** v0.4 (issue #638's own milestone).
- **Last iteration outcome:** #638 implementation and three-round review loop complete; PR not yet opened as of this entry.
- **Exact next step:** push this branch, open the PR (`Fixes #638`), monitor via `ci-watch.sh`, merge, then file the deferred per-iteration-reassigned-name leak issue, then re-enter `issue-select`'s Step 1 baseline for the next issue.
- **In-run denylist:** none — this is the only issue this run has worked.
