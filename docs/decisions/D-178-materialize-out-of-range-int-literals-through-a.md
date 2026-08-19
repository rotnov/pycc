---
id: D-178
title: "Materialize out-of-range int literals through a runtime constructor"
status: accepted
---

## D-178: Materialize out-of-range int literals through a runtime constructor

- Status: accepted
- Context:
  [D-061](./D-061-int-s-fast-path-i64-slot-is-a-low-bit-tagged-63.md) gives `int` a 63-bit tagged
  fast path, and [D-058](./D-058-int-overflow-to-bigint-d-001-is-a-minimal-hand.md) gives
  arithmetic that leaves that range a heap `BigIntObj` to promote into.
  Literals never got the same treatment. `pycc_codegen`'s `tag_smallint_const`
  folded an `int` literal into the tagged encoding at compile time and
  `panic!`ed when the value did not round-trip, so `x = [4611686018427387904]`
  aborted the compiler with exit `101` while the arithmetically identical
  `b = 4611686018427387903 + 1` compiled and ran. The same helper also folded
  `enum` member discriminants, and `pycc_hir` validates those only against
  `i64` range, so `pycc check` accepted a program that then aborted the
  compiler at code generation ([#148](https://github.com/rotnov/pycc/issues/148),
  duplicated by [#616](https://github.com/rotnov/pycc/issues/616)). The
  representation needed to accept these values already existed; only the
  ingress from a literal was missing.

- Decision:
  Export a new never-panicking `extern "C"` runtime entry point,
  `pycc_rt_int_from_i64(v: i64) -> i64`, that returns the tagged word when `v`
  round-trips through D-061's encoding and allocates a `BigIntObj` otherwise.
  `pycc_codegen` gains `fits_tagged_smallint` and `emit_int_constant`
  (carved into `crates/pycc_codegen/src/int_const.rs`); both the
  `MirExpr::IntLiteral` lowering and the `enum`-discriminant fold route
  through it, keeping the compile-time immediate for every in-range value and
  emitting a call only for the rest.

  The bigint is materialized **per evaluation**, not cached in a per-literal
  LLVM global. This mirrors `emit_string_literal`, which emits constant bytes
  and calls `pycc_rt_str_from_literal` at each evaluation: a bigint literal in
  a loop therefore allocates one `BigIntObj` per iteration, leaked under
  D-058's existing concession rather than under a new one. An `enum`
  discriminant materializes once, at module init.

  `tag_smallint_const` survives for `declare_module_globals`, which builds an
  LLVM constant initializer with no `Builder` and therefore no insertion point
  from which to call a runtime function. Its only argument is the literal `0`;
  the out-of-range arm is a defensive panic covered by a direct unit test.

  **Accepting a later failure at 14 runtime `int` boundaries is part of this
  decision, not a side effect of it.** Allowing an out-of-range literal to
  compile lets it reach D-141's runtime `int` boundary in positions the
  compiler previously never reached with a literal: a container value
  (`list`/`set`/`dict` literals, `append`/`add`, comprehension elements,
  `dict.get` defaults, subscript-assign values), an index or repeat count, a
  `range` operand, and a slice bound. All of them abort with exit `101` and
  `pycc_rt: int boundary does not support bigint-valued values yet`. This was
  measured, not assumed: every one of those positions *already* aborted
  identically for an arithmetically promoted bigint before this change. What
  the change removes is the inconsistency where the same program shape failed
  in two different phases depending on how the value was spelled; it invents
  no failure mode and adds no boundary a user could not already hit. The cost
  is stated plainly: for the value positions CPython accepts the value
  outright, so the abort is a genuine capability gap that now surfaces at run
  time rather than at compile time, and `pycc check` alone no longer catches
  it. The positions are pinned by
  `tests/issue_148_oversized_int_literal.rs`, documented in
  [RUNTIME.md](../RUNTIME.md), [TYPE_SYSTEM.md](../TYPE_SYSTEM.md), and
  [ROADMAP.md](../ROADMAP.md), and a compile-time diagnostic for them is
  deferred to its own issue.

- Alternatives:
  - *Keep rejecting an out-of-range literal, as a `C0001` diagnostic instead of
    a panic.* Rejected: it regresses a program CPython accepts, and the heap
    representation needed to accept it already shipped.
  - *Widen the HIR/MIR literal carrier to arbitrary precision.* Rejected as a
    separate architectural seam. A literal beyond `i64` range already produces
    a spanned `C0001` capability diagnostic and exits `1`, which is the
    documented, tested behavior this decision leaves untouched.
  - *Cache each materialized bigint in a per-literal LLVM global.* Rejected:
    it contradicts `emit_string_literal`'s established precedent and buys
    nothing D-058 does not already concede.
  - *Make `tag_smallint_const` fallible and `.expect()` at the global site.*
    Rejected: the only caller passes `0`, so the `.expect` region would be
    permanently unreachable — a D-014 region-gate failure with no legitimate
    exemption.
  - *Add a compile-time "out-of-range literal in an `int`-boundary position"
    diagnostic in this change.* Rejected as scope: it needs a new HIR/MIR
    notion of boundary position spanning 14 sites across three passes, which
    AGENTS.md's decomposition rule places in its own issue.

- Consequences:
  - `pycc_rt_int_from_i64` is a new public runtime ABI entry point. Like every
    other `pycc_rt_*` export it is effectively irreversible once generated
    code calls it.
  - Every in-range `int` literal is still a compile-time immediate: no runtime
    call, no allocation, no measurable codegen change.
  - An out-of-range literal costs one runtime call and one leaked allocation
    per evaluation. Removing the leak is D-058's problem, not this decision's.
  - The bigint operations that are still unimplemented — comparison (int↔int,
    every operator), `*`, `//`, `%`, `**`, a negative exponent, and
    `int`→`float` conversion — became reachable from a one-line program. They
    keep their existing accepted `pycc_rt:` boundaries, now pinned by CLI-level
    tests rather than only by arithmetic-promotion tests.
  - `crates/pycc_codegen/src/int_const.rs` is a narrow, cohesion-driven carve
    out of a 19k-line `lib.rs` under AGENTS.md's decomposability rule.
    `crates/pycc_rt/src/lib.rs` was judged and deliberately left whole: this
    change adds one function to it, and extracting the int-encoding helpers
    would relocate a large body of code the change does not otherwise touch.
