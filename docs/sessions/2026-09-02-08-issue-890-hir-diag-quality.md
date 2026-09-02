# 2026-09-02-08 -- Issue #890: C0001 messages name the rejected construct

## Status: delivered by the pull request that carries this file

Worktree `/Users/denis/projects/pycc-worktrees/issue-890-hir-diag-quality`,
branch `feat/issue-890-hir-diag-quality`, developed on `origin/main` at
`89cefaa3` (the tree right after #880 merged). Implements
[#890](https://github.com/rotnov/pycc/issues/890) as pull request
[#897](https://github.com/rotnov/pycc/pull/897), which carries `Fixes #890`.
Plan: <https://github.com/rotnov/pycc/issues/890#issuecomment-5514049261>.
No new decision record: the change is message text plus a helper, inside the
existing C0001 contract.

## How this task was run

Standard `issue-implement` flow under D-127/D-142/D-143: preflight and
staleness triage in the orchestrating session (all four sites still present
at `cfd858a1`; issue is owner-authored), `issue-to-plan` dispatched in an
isolated agent (its first dispatch died to an API rate limit after drafting;
the second resumed from the draft and ran three adversarial review rounds),
implementation dispatched in a second isolated agent, then the D-068 review
and this file in the orchestrating session.

## What changed

- `crates/pycc_ast/src/lib.rs`: `stmt_kind_name` / `expr_kind_name`,
  exhaustive over every `Stmt` and `Expr` variant, no wildcard arm, with a
  table test pinning every phrase, pairwise distinctness, and the D-219
  cascade-prefix constraint.
- `crates/pycc_hir`: the ten sites that formatted an AST node with `{:?}`
  and the two kind-less catch-alls now name the construct; the `dict.get()`
  arity message no longer claims the receiver is a dict; a `debug_assert!`
  in `unsupported()` rejects any C0001 message carrying an AST `Debug`
  dump. Touched oversized files were decomposed as part of the change:
  `expr.rs` 1927 -> 1277 lines (`expr/tests.rs`, `expr/container_call.rs`),
  `stmt.rs` 1330 -> 1055 (`stmt/tests.rs`, `stmt/for_loop.rs`), `class.rs`
  5688 -> 5443 (`class/protocol.rs`). `lib.rs` was left alone per the plan.
- Tests: 18 new `tests/diagnostics/c0001_*` fixtures (one construct each),
  7 regenerated `.expected.txt`, a fixture scan for `NodeIndex(`, and exact
  wording unit tests for every rewritten site.
- Docs: `docs/DIAGNOSTICS.md` (C0001 contract names the construct; the
  D-217 sentence scoped to the #864 parts rather than "across releases"),
  `crates/pycc_diag/src/explain.rs` example list.

## D-068 review and harden batch

One round of the pinned `ievo:deep-reviewer` over the full
`origin/main..HEAD` range: no actionable findings. The reviewer's sandbox had
no `git`, so the orchestrating session verified extraction purity itself
with `git diff --color-moved=zebra --color-moved-ws=ignore-all-space` over
the extraction commits: the only non-moved lines are the dispatching wrapper,
the `pub(super)` visibility changes, and the re-indented moved tests. With
zero findings there is no `.harden/findings/issue-890.jsonl` pile
(`check_harden_findings.py` rejects an empty one) and the harden batch had
nothing to cluster.

## Gate results (from the committed tree, exit status captured directly)

- `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets --
  -D warnings`, `cargo build --workspace`: exit 0.
- `cargo llvm-cov --workspace --fail-under-lines 100 --fail-under-regions
  100`: exit 0, TOTAL 100.00% regions / functions / lines, 0 missed; no
  exemption added.
- `scripts/` unittest suite (986 tests), `validate_agent_policies.py`,
  `validate_agent_assets.py`, `check_scratch_dir_usage.py`,
  `generate_decisions_index.py --check`, `check_ci_permissions.rb`,
  `check_roadmap_evidence.rb` + its test: all exit 0 (the Ruby checkers
  need `LC_ALL=en_US.UTF-8` under this machine's system Ruby 2.6).
- `cargo doc --workspace --no-deps`: the four pre-existing private-link
  warnings only.

## Acceptance evidence

Differential sweep of all 96 `tests/diagnostics/*.py` inputs against a
`pycc` built from `origin/main`: 71 identical first diagnostics, 18 new
fixtures, exactly the 7 intentionally rewritten messages differ.

## Deliberately left out

- The five `{:?}` sites that print a small operator or literal enum
  (`comparison operator not supported yet: In`, `[Lt, Lt]`) already name the
  construct and stay as they are.
- `expr.rs` (#552) and `class.rs` (#548) remain over the ~1,000-line
  threshold; the extraction here narrows them but does not close either
  tracker, and commenting on those issues is outside this task's
  authorized writes.

## Known state at drafting time

- Open pull requests: #897 only; `origin/main` at `89cefaa3`.
- Issue #890 had exactly one comment (the plan).

## Where to resume

After #897 merges, the standing external-corpus coverage loop continues from
the sweep-2 tracker set (#881-#895, all v0.4): the remaining items are
feature gaps, each its own `issue-implement` run.
