# 2026-08-19-02 — issue #147, bigint-capable `range()`

## Overall status

Issue [#147](https://github.com/rotnov/pycc/issues/147) is implemented on the
task branch `claude/issue-147-bigint-range`, based on default-branch tip
`0ffd7aad` ("Merge pull request #617 from rotnov/claude/next-milestone-c1bab4").
Nothing is pushed and no pull request exists yet — opening it belongs to the
calling session that dispatched this work.

The implementation follows the plan published as a comment on issue #147 by a
prior `issue-to-plan` run; that plan already corrected nine errors in the
issue's own body and was treated as authoritative wherever the two disagreed.

## What landed

Two commits on the branch:

1. **Behavior + docs + decision entry.** `pycc_rt` gains a private sign-aware
   `encoded_int_cmp` (inline fast path; sign-magnitude decode only when a
   bigint is involved; zero decided by magnitude, never by the `negative`
   flag) and a `pycc_rt_range_normalize_operand` export that is encoded-word
   in, encoded-word out. `range_continue` compares all three operands through
   `encoded_int_cmp`, including the zero-step check. `pycc_codegen`'s single
   `range_operand_to_normalized_int` helper (renamed from
   `range_operand_to_tagged_int`) calls the normalizer instead of the
   `build_untag_checked` + `raw_i64_to_tagged_int` pair, so the `ForRange`
   statement and all three comprehension emitters inherit the change through
   one seam. Recorded as
   [D-179](../decisions/D-179-range-loops-drive-bigint-bounds-steps-and.md),
   superseding in part the `range`-operand halves of D-141 and D-178.
2. **Decomposition.** `crates/pycc_rt/src/int_encoding.rs` was split out of
   `lib.rs` under the "keep source files decomposable" rule, taking the
   representation layer this change touched (encoding constants, the
   classifier, `BigIntObj`, the magnitude helpers, `encoded_int_cmp`) and
   leaving every arithmetic/formatting operation and every
   `#[unsafe(no_mangle)] pub extern "C"` wrapper in `lib.rs`. No ABI symbol
   moved. `lib.rs` went 3457 → 3435 lines with 271 lines in the new module.

## Known follow-ups

- Issue [#618](https://github.com/rotnov/pycc/issues/618) still owns the other
  thirteen D-178 runtime `int` boundaries. Its completion criterion 1 lists the
  `range` operand among them; that sub-item is satisfied early by D-179 and
  should be struck when #618 is picked up, not re-implemented.
- `pycc_rt_int_cmp` keeps its own bigint boundary, so the crate now has two
  comparison paths. D-179's consequences section records that #618 should
  reconcile them (most likely by `int_cmp` delegating to `encoded_int_cmp`)
  rather than letting them drift.
- D-058's leak concession is now widest here: a `range` loop whose induction
  variable has promoted leaks one `BigIntObj` per iteration. Bounded by trip
  count, but the "no v0.1 construct can leak this in a hot loop" wording in
  `docs/RUNTIME.md` and in the struct's own doc comment was narrowed
  accordingly rather than left stale.
- `crates/pycc_codegen/src/lib.rs` is still ~19,100 lines. The plan
  deliberately scoped decomposition to `pycc_rt`; decomposing the codegen
  crate remains open and is its own task.

## Where a fresh session should look

- The plan comment on issue #147, then
  `docs/decisions/D-179-range-loops-drive-bigint-bounds-steps-and.md`.
- `crates/pycc_rt/src/int_encoding.rs` for the representation layer and
  `crates/pycc_rt/src/lib.rs`'s `range_continue` /
  `range_normalize_operand` for the loop-control logic.
- `tests/fixtures/bigint_range.py` (differential, registered `#[ignore]`d in
  `tests/conformance.rs`) and `tests/issue_147_bigint_range.rs` (public-CLI
  divergences and the still-aborting container-ingress position).
