# 2026-08-07 checkpoint 04 — issue #356 merged; issue #402 corrected and closed out with a docs-only fix

## Status

PR [#399](https://github.com/rotnov/pycc/pull/399) (issue [#356](https://github.com/rotnov/pycc/issues/356))
**merged** as `c3b5b7b`. PR [#404](https://github.com/rotnov/pycc/pull/404) (issue
[#402](https://github.com/rotnov/pycc/issues/402), docs-only) **merged** as `58e6955`.
`origin/main` tip at the time this entry was written: `58e6955648069001a2f0524c8bf017d1348319a`.

## PR #399 (issue #356) — merge

Picked up from checkpoint
[03](2026-08-07-03-issue-356-ievo-reviewer-structural-verification.md), which left the PR open
with a clean D-068/D-155 review loop and passing CI. Before merge, CI had to absorb one more
flake beyond what checkpoint 03 already handled (the nbody perf gate): `build-test-coverage`
failed on `Check pycc check throughput floor (<50ms/1000 LOC, D-084)` — measured 70.74ms against
a 50ms threshold. Since PR #399's diff is docs/scripts-only (no Rust or binary code touched), a
throughput regression is structurally impossible to attribute to the diff; treated as shared-
runner timing noise, same reasoning already established for the nbody gate, and resolved with
one `gh run rerun --failed`. The re-run passed clean across every check including `ci-gate`.
D-078 state was re-verified (`origin/main` unchanged, PR head unchanged, zero unresolved review
threads via a `gh api graphql` query for `reviewThreads`) immediately before merging with
`gh pr merge 399 --merge --delete-branch`.

## Issue #402 — discovered, then partially retracted

While diagnosing PR #399's coverage-gate failure, an unreachable-code claim was investigated for
`check_incompatible_redefinitions`'s post-resolution call inside `check_and_resolve`
(`crates/pycc_types/src/lib.rs`). Issue #402 was filed asserting that call could **never** fire
— that `check_and_resolve`'s resolution loop always collapses two same-named function
definitions onto one shared resolved signature before the check ever runs, making its error path
dead.

That claim was itself wrong, caught before other work built on it: PR #403 added a test
(`check_and_resolve_rejects_incompatible_redefinition_after_inference`) proving the call *does*
fire and correctly rejects a redefinition once the two definitions' resolved **arities** differ.
The real gap is narrower — same-arity, `Ty::Infer`-involving redefinitions converge onto one
shared resolved signature via `check_and_resolve`'s `HashMap<String, (Vec<Ty>, Ty)>` (keyed by
name, not by item) and slip through uncaught; different-arity ones do not.

Issue #402's body was corrected in place (`gh issue edit`, `> **Correction (2026-08-07):**`
callout with struck-through retracted claims, reproduction/impact sections left intact since
they already used a same-arity fixture and remain accurate), plus an explanatory comment citing
PR #403's test as evidence
([comment](https://github.com/rotnov/pycc/issues/402#issuecomment-5218030870)). The issue was
**not closed** — the same-arity gap it originally reproduced is still real and still open.

## PR #404 — correct the overclaim in comments and docs

Five places in the tree repeated #402's original (now-retracted) claim that the post-resolution
call was dead/unreachable or "being removed separately": two comments in
`crates/pycc_types/src/lib.rs` (the call site inside `check_and_resolve`, and
`check_incompatible_redefinitions`'s own doc comment), `check()`'s own doc comment, a test
comment on `check_incompatible_redefinitions_skips_infer_signature_functions`, a misattributed
comment on a fully-concrete fixture in `tests/issue_22_execution_order.rs`
(`incompatible_redefinition_is_a_build_error`, which is actually rejected by the *pre*-resolution
check, not the post-resolution one), and `docs/ROADMAP.md`'s issue #22 resolution bullet. All six
corrected to state the precise same-arity-vs-different-arity boundary instead of the blanket
overclaim. No logic change — doc/comment-only diff.

Two D-068 review rounds: round 1 found all six of the above; round 2 (after fixes) came back
clean (two non-blocking notes only). Added a `docs/AGENT_RETROSPECTIVE.md` entry on the process
mistake that produced the original overclaim: proving a branch "unreachable" from a test that
varied only one of two independent dimensions (arity vs. same-arity-with-Infer) of its own
equality comparison. CI green on every check including `ci-gate`; D-078 state re-verified clean
immediately before merge (`gh pr merge 404 --merge --delete-branch`).

## Local gate evidence

- `cargo build -p pycc_types` — clean (verified from the correct worktree cwd after an earlier
  false-positive build from the wrong directory silently validated unedited files).
- `cargo test -p pycc_types incompatible_redefinition` — 6/6 pass.
- `cargo doc --workspace --no-deps` — clean.
- `cargo test --test issue_22_execution_order` — the touched test passes; 8 unrelated
  "`pycc build` should succeed" tests fail identically on a `git stash`-baseline of the same
  tree, confirmed pre-existing/environmental (likely a missing native linker/toolchain in this
  sandbox), not caused by this diff.

## Where to resume

This session's standing task, per checkpoint 03's own note, is to resume the D-068/D-155-gated
"ultra review" for PR [#357](https://github.com/rotnov/pycc/pull/357)
("Add ultra-review: periodic evidence-gated review that files prioritized issues"). That PR was
opened against a stale, pre-D-151-decomposition tree — it still touches the now-nonexistent
monolithic `docs/DECISIONS.md` — and needs its own rebase/reconciliation against the current
`main` (`58e6955`) before its review loop can resume meaningfully. Check its state first
(`gh pr view 357 --repo rotnov/pycc`) before deciding how much of the original diff still
applies.
