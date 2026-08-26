# 2026-08-26-02 -- Issue #790 / PR #791: rebase and D-068 review fix round

## Status: delivered by the pull request that carries this file

Continues `docs/sessions/2026-08-25-04-issue-790-typing-type-checking.md`
(not edited, per this project's "new snapshot is always a new file" rule).
That entry covered the original implementation; this one covers a later
session that rebased the branch twice onto an advancing `origin/main`
(concurrent agents merging #786 and #794 mid-task) and then processed the
D-068 pinned `ievo:deep-reviewer`'s findings against the rebased diff.

Worktree: a fresh scratch worktree created from `origin/feat/issue-790-tc-23e19060`
(the pre-existing `/Users/denis/projects/pycc-worktrees/issue-790-tc-23e19060`
was left untouched to respect "one writer per worktree"). Branch head at the
time this file was written: rebased onto `origin/main` at `e77c1b65`
(post-#786, post-#794), commit history rewritten by the rebase (no new merge
commits).

## Rebase

Two rebase passes, tracking `origin/main` as it advanced:

1. Onto `af2384fa` (post-#778): two `docs/ROADMAP.md` conflicts, both
   simple additive-entry conflicts (kept both sides' content, in source
   order).
2. Onto `e77c1b65` (post-#786, post-#794): applied with **zero** new
   conflicts. `git diff --stat` against the first rebase's diff was
   identical, confirming the first pass's conflict resolutions were
   complete and durable.

Full local gate (`cargo build --workspace`, `cargo test --workspace`,
`cargo clippy --workspace --all-targets -- -D warnings`, `cargo llvm-cov
--workspace --fail-under-lines 100 --fail-under-regions 100`) was green
after both rebases.

## D-068 review and fix round

Dispatched the pinned `ievo:deep-reviewer` against the rebased diff
(merge-base through HEAD). Four findings, all evaluated per D-127 autonomous
judgment (consulted this session's own advisor tool for the harder calls):

1. **API contract fidelity (fixed):** the call-site marker-rejection
   `if`-chain in both `crates/pycc_types/src/expr.rs` and
   `crates/pycc_types/src/constraints.rs` had specialized arms for
   `EnumMarker`/`AnnotationMarker`/`CastMarker` but fell through to the
   generic `marker_is_not_a_value` for `TypeCheckingMarker`, contradicting
   `type_checking_marker_is_not_a_value`'s own doc comment, which claims to
   cover the call-site case too. Added the missing arm in both files
   (`typing.TYPE_CHECKING(...)` now reports the dedicated message) and three
   new tests per file-pair mirroring the existing `CastMarker` call-site
   tests (`qualified_type_checking_marker_called_is_t0021` and its
   annotated-function/private-helper variants in
   `crates/pycc_types/src/tests.rs`).
2. **Test/impl drift (fixed):** no test exercised a *bare* `if
   TYPE_CHECKING:` with **no** `else` through an actual `build`+`run` --
   every existing test either only ran `pycc check`, or had a live `else`/
   `elif`. Added
   `a_bare_type_checking_guard_with_no_else_builds_and_runs` to
   `tests/issue_790_typing_type_checking.rs`, proving the folded
   `HirStmt::If { body: vec![], orelse: vec![] }` reaches codegen and
   executes as a genuine no-op (the statement after the guard still runs).
3. **Error-path coverage / silent-divergence risk (deferred, tracked by
   [#798](https://github.com/rotnov/pycc/issues/798), filed in milestone
   v0.3):** `is_type_checking_guard` is purely syntactic and not
   import-gated -- a module that never imports `typing`/`TYPE_CHECKING` but
   defines its own truthy module-level `TYPE_CHECKING` would silently
   diverge from CPython (guarded body runs under CPython, folded away here).
   This is the same silent-divergence class of bug the project has already
   treated as a real defect (#767/D-198, #740/D-195), so it was not simply
   waved off as cosmetic. A precise fix requires threading a new
   import-availability flag through `pycc_hir`'s entire recursive
   statement-lowering descent -- investigation found this touches at least
   8 function signatures (`lower_stmt`, `lower_body`,
   `lower_elif_else_clauses`, `lower_match`, `lower_except_handler` in
   `stmt.rs`/`stmt/exception.rs`, `lower_function` in `func.rs`, and
   `lower_class`/`lower_method` in `class.rs`) across 5 files, with dozens
   of pass-through call sites and new coverage-mandated test paths under
   D-014's 100% gate for each newly reachable branch. Judged genuinely large
   relative to the rest of this PR's diff and better served by its own
   focused PR. Documented the gap explicitly in `is_type_checking_guard`'s
   own doc comment (`crates/pycc_hir/src/stmt.rs`) and in the `docs/ROADMAP.md`
   #790 entry, and filed #798 with the concrete fix shape and the exact
   function list above.
4. **Doc drift (fixed):** `is_type_checking_guard`'s doc comment claimed
   precedent from `is_final` "above", but `is_final` is a local `let`
   binding inside the `Stmt::AnnAssign` arm further down the file, not a
   function defined earlier. Corrected the wording.

Re-ran the full local gate after the fixes (all four together, one gate
run): `cargo build --workspace`, `cargo test --workspace`, `cargo clippy
--workspace --all-targets -- -D warnings`, `cargo llvm-cov --workspace
--fail-under-lines 100 --fail-under-regions 100` -- all green, 100.00%
lines/regions maintained. Re-dispatched the D-068 reviewer against the
updated diff per this project's "re-run after fixes" rule; no further
actionable findings.

## Test evidence added this round

- `crates/pycc_types/src/tests.rs`: 3 new call-site rejection tests for
  `typing.TYPE_CHECKING(...)` (module level, inside an annotated function,
  inside a private helper -- covering both the validation-pass path in
  `expr.rs` and the solver path in `constraints.rs`).
- `tests/issue_790_typing_type_checking.rs`: 1 new build+run test for a bare
  `if TYPE_CHECKING:` with no `else`.

## Where to resume

Nothing outstanding from this task beyond the PR's own CI and merge. Follow
-up work for the deferred import-gating fix lives in
[#798](https://github.com/rotnov/pycc/issues/798), milestone v0.3, not in
this PR.
