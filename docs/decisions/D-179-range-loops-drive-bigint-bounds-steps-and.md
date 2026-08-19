---
id: D-179
title: "range() loops drive bigint bounds, steps, and induction variables"
status: accepted
---

## D-179: `range()` loops drive bigint bounds, steps, and induction variables

- Status: accepted (closes issue [#147](https://github.com/rotnov/pycc/issues/147)).
  Supersedes the `range`-operand halves of
  [D-141](./D-141-preserve-bool-identity-in-the-int-compatible-abi.md) and
  [D-178](./D-178-materialize-out-of-range-int-literals-through-a.md); both
  remain accepted for everything else they decide.
- Context:
  [D-141](./D-141-preserve-bool-identity-in-the-int-compatible-abi.md) made every
  `range()` operand pass through `pycc_rt_int_untag_checked` and re-tag as an
  ordinary smallint before entering the induction phi. That achieved the
  bool-identity contract it was written for -- a range consumes the numeric
  *value* of its arguments and produces ordinary integer objects, so `True`
  must not survive into the loop target -- but it did so by round-tripping
  through a raw `i64`, and that decoder rejects every heap `BigIntObj`.
  `pycc_rt_range_continue` compounded the restriction by calling
  `require_inline_int` on all three of its own arguments.

  The result was three separate failures, all reported as
  `pycc_rt: int boundary does not support bigint-valued values yet` at
  [D-072](./D-072-print-s-none-result-cannot-be-used-as-a-nested.md)'s exit
  `101`: a bigint bound, a bigint step, and -- the least obvious one -- an
  ordinary smallint loop whose induction variable crossed the tagged range
  *mid-loop*, because `pycc_rt_int_add` promotes on overflow and the next
  `range_continue` call then saw a bigint `i`. That last shape needs no
  out-of-range literal anywhere in the source.

  [D-178](./D-178-materialize-out-of-range-int-literals-through-a.md) widened
  the reach of all three by letting an out-of-range *literal* compile, and
  recorded the `range` operand as one of fourteen accepted runtime `int`
  boundaries. A loop guard is not the same kind of boundary as container
  ingress: rejecting a value a container cannot yet store is a storage gap,
  whereas refusing to *count* past 2^62 is a control-flow gap in a construct
  the language guarantees works over arbitrary integers.
- Decision: make `range` bigint-capable end to end, without changing MIR or
  any runtime function signature already in the ABI.
  1. `pycc_rt` gains a private `encoded_int_cmp` that orders two encoded
     int-compatible words across the whole representation. Two inline
     operands take a plain `i64` comparison so an ordinary smallint loop
     allocates nothing per iteration; as soon as either side is a bigint, both
     decode to sign-magnitude and compare sign-first, then by magnitude
     (reversed for two negatives). **Zero is decided by magnitude, never by
     the `negative` flag** -- `bigint_add_signed` normalizes an
     equal-magnitude, opposite-sign result to `negative: false, limbs: [0]`,
     the same trap `pycc_rt_int_truthy` already documents.
  2. `range_continue` drops `require_inline_int` and routes all three
     operands through `encoded_int_cmp`, including the zero-step check. The
     step's *value* decides, not its word: a bigint-tagged zero is a non-zero
     pointer word and must still raise the zero-step boundary rather than
     looping forever.
  3. A new `pycc_rt_range_normalize_operand` replaces the
     `range_untag_operand` call codegen used to emit. It is encoded-word in,
     encoded-word out: the `False`/`True` markers become the ordinary
     smallints `0`/`1`, a smallint or a heap bigint passes through unchanged,
     and a malformed word still fails closed inside `classify_encoded_int`.
     D-141's bool contract is therefore preserved exactly, by the same
     runtime that owns the representation, and all twelve call sites (the
     `ForRange` statement plus the list, set, and dict comprehension
     emitters) inherit the change through the single
     `range_operand_to_tagged_int` helper.

  This decision does **not** touch the other thirteen D-178 boundaries.
  `pycc_rt_int_cmp`'s general bigint comparison, container ingress, indices,
  slice bounds, and `str` repeat counts keep their existing boundaries; issue
  [#618](https://github.com/rotnov/pycc/issues/618) still owns them, and its
  completion criterion 1 (which lists the `range` operand among the positions
  it must fix) is satisfied early by this decision rather than contradicted.
- Alternatives:
  - *Delete the operand guard entirely and pass the encoded word straight
    into the phi.* Rejected: that silently regresses D-141: `range(True)`
    would forward the `True` marker into the loop target and print `True`
    instead of `0`, which CPython does not do.
  - *Demote a bigint back to a smallint whenever its value fits.* Rejected:
    that is a workspace-wide change to
    [D-058](./D-058-int-overflow-to-bigint-d-001-is-a-minimal-hand.md)'s
    "once promoted, stays promoted" rule, affecting `int_add`/`int_sub`
    output identity everywhere, to fix one construct.
  - *Split into two pull requests, runtime then codegen.* Rejected: codegen
    cannot link against an export that does not exist yet, and
    [D-014](./D-014-100-test-coverage-requirement.md)'s 100% region
    gate rejects a runtime primitive with no consumer.
  - *Widen `pycc_rt_int_cmp` instead of adding a second comparison.*
    Rejected: `int_cmp` is Python's `<`/`==` operator surface, whose bigint
    behavior is #618's design question (including `int`/`float` mixing).
    Loop control needs only a total order over `int`, and coupling the two
    would drag an unrelated open design into this change.
- Consequences:
  - `for` loops, and all three comprehension forms, work over arbitrary
    `int` bounds and steps within the compiler's `i64`-literal capability.
    The mid-loop promotion shape works without any bigint appearing in the
    source at all.
  - The `range` operand leaves D-178's boundary inventory: thirteen positions
    remain, and the count and enumeration are corrected in `docs/RUNTIME.md`,
    `docs/TYPE_SYSTEM.md`, `docs/ROADMAP.md`, and D-178 itself.
  - D-058's leak concession gets its widest exposure so far: a loop whose
    induction variable has promoted allocates and leaks one `BigIntObj` per
    iteration, where before it aborted on the first such iteration. The leak
    is bounded by the loop's own trip count rather than unbounded, so the
    concession's original "no construct can leak this in a hot loop"
    justification is narrowed rather than abandoned. Freeing bigints is
    D-060-style refcounting work that stays out of scope.
  - `encoded_int_cmp` is a second comparison path alongside `int_cmp`. When
    #618 makes `int_cmp` bigint-capable, the two should be reconciled --
    most likely by `int_cmp` delegating to `encoded_int_cmp` -- rather than
    left to drift.
  - Loop-control cost is unchanged for ordinary smallint loops. Sending all
    three operands through `encoded_int_cmp` unconditionally measured a
    consistent regression on a 50-million-iteration release loop
    (`for i in range(50000000): total = total + 1`): five paired rounds gave
    a warm 0.32s before the change and 0.39s after, roughly +22%. No CI gate
    would have caught it -- `scripts/check_replicated_paired_perf_regression.rb`
    times `pycc check` (frontend compilation), and `tests/nbody_bench.rs` is
    `#[ignore]`d and is not invoked by any workflow -- so it was found by
    direct measurement against the pre-change driver. `range_continue`
    therefore keeps an explicit three-inline-operand fast path that orders
    plain `i64`s; the same measurement after adding it gives a warm 0.30s
    against the same 0.32s baseline. The fast path is a performance shortcut
    only: `inline_int_value` returns `None` for a bigint, so every promoted
    operand still takes the general path, and a malformed word fails closed
    in `classify_encoded_int` either way.
