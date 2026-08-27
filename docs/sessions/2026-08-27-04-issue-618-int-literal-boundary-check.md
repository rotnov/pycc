# Session: issue #618 — compile-time int-literal boundary check (T0051)

Date: 2026-08-27
Branch: `feat/issue-618-int-literal-boundary-check`
Base: `origin/main` at `c8176cc5` (v0.4 milestone)

## What this delivers

Closes [#618](https://github.com/rotnov/pycc/issues/618): an out-of-range
`int` *literal* written directly in one of D-141's runtime `int`-boundary
positions is now rejected by `pycc check`/`pycc build` at compile time with
a new spanned diagnostic, `T0051`, instead of type-checking successfully and
only aborting at run time (`pycc_rt_int_untag_checked`, exit 101) — the
consequence D-178 (PR #617, closing #148) knowingly accepted for the literal
case when it made an out-of-range literal materialize as a heap bigint
everywhere else.

A bigint value reaching the same position through *arithmetic* (not a
literal) is completely unaffected and still hits the existing runtime abort,
exactly as D-178 left it.

## Architectural decision: one PR, entirely in `pycc_hir`

Issue #618's own filed text, and D-178's original deferral note, both
estimated this needs "a boundary-position notion threaded across those 14
sites in three passes (HIR, MIR, codegen)". That estimate was investigated
at implementation time and found inaccurate — recorded in the new decision
[D-207](../decisions/D-207-compile-time-int-literal-boundary-check-is-a.md),
which supersedes D-178's deferral note for the literal case:

- `pycc check` only ever runs `pycc_hir::lower_checked` followed by
  `pycc_types::check` (`src/main.rs`'s `check_frontend`); MIR and codegen
  never run during `check`, so a check catching the defect during HIR
  lowering alone is sufficient.
- [D-179](../decisions/D-179-range-loops-drive-bigint-bounds-steps-and.md)
  had *already* removed the `range()` operand from D-141's boundary
  inventory before #618 was even filed (`range()` is fully bigint-capable
  via `pycc_rt_range_normalize_operand`/`pycc_rt_range_continue`), leaving
  **13** positions, not 14 — a fact #618's own filing missed.
- Of those 13, 12 are resolved purely syntactically during HIR lowering
  (no type information needed): `list`/`dict`/`set` literals, subscripts,
  slices, and the bare-name-receiver + attribute-name shape already used to
  recognize `list.append`/`dict.get`/`set.add`.
- The 13th, `str * int` repeat count, needs type information `pycc_hir`
  does not have to distinguish a `str`-typed *variable* from any other
  operand. Rather than routing it through `pycc_types` (which has no real
  spans — `crates/pycc_types/src/expr.rs` has 158 `Span::new(0, 0)`
  placeholders, a separate architectural project), it is narrowed to only
  fire when the string side is itself a string *literal*
  (`"ab" * <huge int>`). A `str`-typed variable multiplied by an oversized
  literal (`s * <huge int>`) is a documented, accepted gap and still hits
  the runtime abort.

The whole fix is a single new module, `crates/pycc_hir/src/int_boundary.rs`
(`fits_tagged_smallint`, `int_literal_boundary_diagnostic`,
`check_boundary_literal`), wired into the 12 call sites in
`crates/pycc_hir/src/expr.rs` and `crates/pycc_hir/src/stmt.rs`, plus the
narrowed 13th in the `str * int` binop-lowering path. No MIR or codegen
change.

## A real regression caught and fixed this session

An earlier pass of this work (before this session) had `lower_range_call`
in `crates/pycc_hir/src/expr.rs` call `check_boundary_literal` on every
`range()` argument, incorrectly treating `range()` as one of the boundary
positions — contradicting D-179, which was written specifically to take
`range()` out of that inventory. Running the full workspace test suite
surfaced 6 failures in `tests/issue_146_bigint_release.rs`: previously
working bigint-range functionality (e.g. a bigint loop bound) started
raising a spurious T0051 on an in-range literal argument.

Fixed by removing the `check_boundary_literal` call from `lower_range_call`,
replacing the two unit tests that had asserted T0051 fired for `range()`
with two tests asserting successful lowering, and correcting the
"14 positions" language (and the false claim that `range()` was one of the
syntactically-resolved cases) to "13 positions" across
`crates/pycc_hir/src/int_boundary.rs`, `crates/pycc_diag/src/explain.rs`,
`docs/DIAGNOSTICS.md`, `docs/ROADMAP.md`, `docs/RUNTIME.md`,
`docs/decisions/D-178-...md`, and the new `docs/decisions/D-207-...md`.
This correction is the reason D-207 exists as its own decision file rather
than folding straight into D-178: the "14 vs 13" and "three passes vs one"
corrections needed a citable record independent of D-178's original,
now-stale estimate.

Separately, 6 pre-existing tests in `tests/issue_148_oversized_int_literal.rs`
failed after the fix — not a bug, but a stale premise: those tests asserted
the *old* D-178/#148 behavior (literal compiles, aborts at runtime) that
#618 intentionally supersedes for the literal case. Rewrote them to assert
`pycc check` now rejects with T0051, and added a new test,
`an_oversized_literal_as_a_str_variable_repeat_count_still_hits_the_runtime_int_boundary`,
to pin the documented `str`-variable-repeat-count gap.

## Verification performed this session

- `cargo test --workspace`: full green (1442+ tests across all binaries,
  0 failed).
- `cargo llvm-cov --workspace --fail-under-lines 100 --fail-under-regions 100`:
  **100.00%** lines, regions, and functions across the entire workspace —
  D-014's hard coverage gate holds with the range() fix and the rewritten
  `issue_148` tests included.
- `cargo doc --workspace --no-deps`: succeeds cleanly (only pre-existing,
  unrelated `rustdoc::private_intra_doc_links` warnings in
  `pycc_scratch`/`pycc_types`, not introduced by this change).
- `LANG=en_US.UTF-8 LC_ALL=en_US.UTF-8 ruby -E UTF-8
  scripts/test_check_roadmap_evidence.rb` and
  `scripts/check_roadmap_evidence.rb`: pass (237 runs / 0 failures; roadmap
  evidence policy passed). The `LANG`/`LC_ALL`/`-E UTF-8` prefix is required
  in this environment or the script throws a spurious
  `invalid byte sequence in US-ASCII` unrelated to actual content.
- `python3 scripts/generate_decisions_index.py docs/decisions
  docs/decisions/README.md --check`: up to date after adding D-207.
- `python3 scripts/manage_ci_bypass.py status`: branch protection matches
  the documented baseline — no open CI-bypass incident to reconcile.
- `python3 scripts/check_claude_reviewer_binding.py`: `ievo@ievo-skills`
  binding structurally intact (0.80.19 installed, 0.80.24 available).
- D-068 pinned local reviewer (`ievo:deep-reviewer`) run against the staged
  diff — see disposition below.

## Files changed

- `crates/pycc_hir/src/int_boundary.rs` (new) — the boundary-check module.
- `crates/pycc_hir/src/expr.rs`, `crates/pycc_hir/src/stmt.rs`,
  `crates/pycc_hir/src/lib.rs` — wiring the 13 checked positions and the
  `range()` exclusion.
- `crates/pycc_diag/src/explain.rs` — new `T0051` `EXPLANATIONS` entry.
- `docs/DIAGNOSTICS.md`, `docs/RUNTIME.md`, `docs/TYPE_SYSTEM.md`,
  `docs/ROADMAP.md` — updated for #618/T0051 and the 13-position count.
- `docs/decisions/D-178-materialize-out-of-range-int-literals-through-a.md`
  — superseded-in-part note, corrected position count.
- `docs/decisions/D-207-compile-time-int-literal-boundary-check-is-a.md`
  (new) — the "single-pass `pycc_hir` fix, not three" decision record.
- `docs/decisions/README.md` — regenerated index.
- `tests/int_literal_boundary_check.rs` (new),
  `tests/diagnostics/t0051_int_literal_boundary_list_index.{py,expected.txt}`
  (new), `tests/diagnostics_test.rs`, `tests/issue_148_oversized_int_literal.rs`
  — end-to-end and unit coverage for T0051 and the two D-014 completion
  criteria (literal rejected at compile time; arithmetic bigint unaffected).

## Where a fresh session should look to resume

If this file is being read before the pull request for #618 has merged,
check `gh pr view` on branch `feat/issue-618-int-literal-boundary-check`
for current CI/review state before assuming any step below is still
pending. As of writing this entry, remaining steps were: address any D-068
reviewer findings, open the pull request (closing #618), wait for CI
(including the 100% coverage gate) to go green, and self-merge per D-127.
