---
id: D-180
title: "Refcount heap bigints and release them at named-storage and loop-induction sites"
status: accepted
---

## D-180: Refcount heap bigints and release them at named-storage and loop-induction sites

- Status: accepted (Part 1 of issue
  [#146](https://github.com/rotnov/pycc/issues/146), tracked as
  [#624](https://github.com/rotnov/pycc/issues/624)). **Narrows**
  [D-058](./D-058-int-overflow-to-bigint-d-001-is-a-minimal-hand.md) rather than superseding it:
  D-058's sign-magnitude limb representation, its base-2^32 little-endian
  layout, and its "once promoted, stays promoted" rule are all unchanged and
  remain accepted. The single half this decision narrows is D-058's "never
  freed" storage concession, which is now bounded to the residual set
  enumerated under *Consequences* below instead of covering every
  `BigIntObj` ever allocated.
- Context:
  D-058 accepted leaking every heap bigint on the grounds that promotion is
  an overflow-only path: a program that never crosses the D-061 tagged range
  allocates nothing, and one that crosses it a handful of times leaks a
  handful of small allocations. That reasoning depended on no ordinary
  construct being able to allocate bigints in a hot loop.

  [D-179](./D-179-range-loops-drive-bigint-bounds-steps-and.md) (issue
  [#147](https://github.com/rotnov/pycc/issues/147)) removed that dependency.
  A `range()` loop whose induction variable crosses the tagged range now
  keeps iterating instead of aborting at D-141's runtime `int` boundary, so
  each iteration's `pycc_rt_int_add` allocates one `BigIntObj` that nothing
  ever frees. The leak became linear in a loop's trip count rather than
  bounded by a program's count of overflow sites. Measured on macOS arm64 at
  `883312d9`, a one-million-iteration loop that rebinds a bigint local peaked
  at 98,336,768 bytes against a 1,982,464-byte control, and doubling the trip
  count doubled the peak exactly -- roughly 98 bytes per iteration, with no
  upper bound other than the loop's own length.

  `str` already had the answer in-tree. [D-060](./D-060-pycc-own-ownership-escape-rc-elision-is-confirmed.md)
  and [D-074](./D-074-preserve-checked-lexical-scope-and-scalar.md) give `PyStrObj` a non-atomic
  `rc: Cell<u32>`, release it at named-storage sites, and record the residual
  leaks explicitly rather than claiming completeness. The question this
  decision answers is not *whether* to refcount bigints -- the measurement
  above settles that -- but *which* sites participate, and what remains
  leaked afterwards.
- Decision:
  1. `BigIntObj` carries `rc: Cell<u32>`, the same non-atomic header shape as
     `PyStrObj`/`PyIntListObj`/`PyDictObj`/`PyIntSetObj`, for the same reason
     (pycc emits single-threaded programs). `BigIntObj::new` is the sole
     constructor and sets `rc == 1`; `tag_bigint` is the sole path that turns
     such an object into an encoded word, so every word handed out carries
     exactly one birth reference.
  2. Two runtime entry points move it: `pycc_rt_bigint_retain(word: i64)` and
     `pycc_rt_bigint_release(word: i64)`, both taking D-141's *encoded word*
     rather than a pointer, both no-ops for smallints and the two
     bool-identity markers, and both returning immediately on the word `0`
     before classification. `0` is not a valid encoded int -- it is
     `classify_encoded_int`'s fail-closed pattern -- but it is exactly what an
     `int` storage slot holds between its zero-initialization and its first
     store, so it must be a defined no-op rather than a panic.
  3. `pycc_codegen` emits an inline `(word & 0b11) == 0 && word != 0` test
     before every retain/release call. The runtime already handles the other
     kinds, so this guard is a performance contract, not a correctness one:
     [D-084](./D-084-pycc-check-s-throughput-floor-ci-step-runs-after.md)/D-140's nbody throughput floor
     is measured on entirely-smallint code, and routing every `int` assignment
     through an unconditional runtime call would regress it.
  4. Releases are gated on the **storage slot's declared type**, never on the
     assigned value's. `x: int = True` reaches codegen as a `Ty::Bool` value
     that `coerce_scalar_to_type` turns into an encoded word; value-type
     gating would skip the release of the bigint the slot still holds.
  5. The release lives inside `emit_assign` itself rather than at each of its
     call sites. Thirteen of that function's callers bind an `int` (ordinary
     assignment, the four `range`-induction binds, and the eight
     container-element binds), and a missed *release* is a silent leak that no
     value assertion can observe -- centralizing it makes "released before
     every int store" a property of the function instead of a property of an
     audit. `str` keeps its per-site `decref_str_slot_before_store` call: only
     a small minority of `emit_assign`'s callers bind a `str`, and moving it
     would be churn without the same payoff.
  6. Retains are applied at a **strictly smaller** set of sites than `str`'s.
     `incref_if_str_duplicate` runs wherever a `str` is evaluated, including
     two sites that merely consume the value (`print`'s argument and `to_str`'s
     operand); those paths decref again themselves, so the extra incref is
     balanced for `str`. An `int` retain there would be orphaned. A separate
     `retain_if_int_duplicate` therefore runs only where the value's new home
     takes ownership: an assignment target, a `return` value, an instance
     attribute, and a call argument.
  7. A `for`/comprehension loop over a `range` owns its induction values
     explicitly. `range_operand_to_normalized_int` passes a bigint operand
     through unchanged, so `for i in range(b, b, b)` gives one heap object
     three names. `MirStmt::ForRange` retains `start_v` as the *first owner of
     `current`* -- never as an independent third operand -- and retains
     `stop_v`/`step_v` once each, releasing those two in `for_after`. For `n`
     executed iterations there are `n + 1` owned `current` values (`start_v`
     plus one `int_add` result per iteration), matched by `n` per-iteration
     releases plus one `for_after` release. Separately, each bind of the
     visible target retains `current` and is matched by the next bind's
     release-before-store. The comprehension emitters follow the same contract
     minus the `stop_v`/`step_v` pair, which they neither retain nor release
     (balanced by construction, since they only read those operands).
- Alternatives:
  - **Keep D-058's blanket leak and revert D-179.** Rejected: D-179 closes a
    real correctness gap (`range` over the bigint domain aborted), and trading
    a correctness fix for a memory fix is a false choice when both are
    available.
  - **A tracing or arena collector for bigints.** Rejected as far out of
    proportion. `str` already establishes refcounting as this runtime's
    ownership model, and a second, different model for one type would be a
    project-wide design commitment with no consumer asking for it.
  - **Refcount every site in one change, including unbound arithmetic
    temporaries.** Rejected in favor of splitting: temporaries need a
    different mechanism (a value's *last use*, not a named location's
    lifetime), touch a separate seam in the emitter, and have their own test
    surface. They are Part 2, issue
    [#625](https://github.com/rotnov/pycc/issues/625).
  - **Thread the declared attribute `Ty` from `mir.class_defs` into
    `MirStmt::AttrSet`.** Rejected for Part 1: `AttrSet` carries only a slot
    *index*, and threading the type through is a scope addition whose only
    payoff is one narrow shape (a `bool` value assigned into an
    `int`-declared attribute). The consequence of not doing it is a leak,
    never a use-after-free -- see below.
  - **An unconditional runtime call, letting `pycc_rt` do the kind test.**
    Rejected on the D-084/D-140 throughput floor: the guard has to be inline
    or every smallint assignment pays a call.
  - **Widen D-141's container boundary so containers can hold bigints, and
    refcount container ingress/egress too.** Explicitly out of scope. What
    generalizes to containers later is this decision's *site list*, not its
    refcount mechanism, and a container-held bigint reference has no owner
    today because ingress rejects bigints outright. Whoever widens that
    boundary must add an egress retain at `build_int_list_get`,
    `build_int_list_pop`, `build_int_list_slice`, `build_dict_get_or_default`,
    `build_int_set_get`, and the `DictGet` arm as a precondition of the
    change; this decision does **not** discharge the standing
    [D-107](./D-107-list-t-pointers-get-their-own-pycc-codegen-scalar.md)/[D-124](./D-124-dict-str-int-set-int-refcounting-stays-leak-only.md)
    container-refcounting follow-ups.
- Consequences:
  - The unbounded, trip-count-linear leak D-179 introduced is gone. Peak RSS
    for a bigint-rebinding loop and for a bigint-domain `range` loop is now
    flat in the iteration count, gated by a ratio assertion in
    `tests/issue_146_bigint_release.rs` (a ratio, never an absolute bound:
    `rusage::ru_maxrss` is bytes on macOS/BSD and kilobytes on Linux).
  - `int` storage slots are now zero-initialized at function entry, exactly
    like `str` slots are null-initialized, so the release-before-store on a
    slot's first assignment reads a defined word rather than uninitialized
    stack.
  - **Residual accepted leaks**, all memory-safe (a leak, never a
    use-after-free), and all narrower than what D-058 accepted before:
    1. A `return` executed *inside* a `for` loop body skips that iteration's
       `current` release and both `for_after` operand releases, leaking the
       induction value, `stop_v`, `step_v`, and the target slot's last bind.
    2. An `int` parameter or local is not released at function return. This is
       exactly D-074's own `str` boundary, unchanged and for the same reason:
       real lifetime tracking is `pycc_own` (v0.5) work.
    3. A call argument's retain has no matching release at the callee's return
       boundary -- the callee's parameter slot holds it until the slot is
       overwritten. Consequence (2) is the general case of this.
    4. `emit_enum_member_inits` writes an owned `rc == 1` word into instance
       slot 0 that nothing ever releases. One allocation per bigint-valued
       enum member per program, materialized once at module init.
    5. **Module globals are deliberately not released at module exit**, unlike
       `str`'s own `compile_to_object` epilogue. The process is exiting; the
       release would run once per global immediately before `main` returns,
       adds code and a coverage obligation, and frees nothing the OS is not
       about to reclaim anyway. Stated here explicitly so the omission reads
       as a decision rather than an oversight.
    6. A `bool` value assigned into an `int`-declared *instance attribute*
       skips the attribute slot's release (see the `AttrSet` alternative
       above). This one is deliberately not pinned by a test, and cannot be:
       attempting the fixture uncovered a separate, pre-existing D-154
       defect -- `scalar_to_slot_word` stores a `Scalar::Bool` into an
       attribute slot as a raw `zext` rather than a D-141 encoded word, so
       `b.v = True` on an `int`-declared attribute reads back as `0` no
       matter what the refcounting does. Fixing that is out of #624's scope;
       a fixture written against today's behavior would only enshrine the
       wrong output.
    7. Unbound arithmetic temporaries -- including a bigint *literal*, which
       `int_const::emit_int_constant` materializes per evaluation -- are still
       leaked. This is the largest residual class and is Part 2 (#625).
  - The two structural properties this design depends on -- that no refcount
    call is ever reached without the inline guard, on the word the call
    itself receives, and that the block split leaves no stale phi
    predecessor -- are invisible in a program's output and in its peak RSS,
    so behavioral tests cannot see them. They are pinned instead at
    codegen depth by `an_int_slot_store_emits_a_guarded_release_of_the_word_it_overwrites`
    and `a_range_loop_over_one_aliased_bound_emits_guarded_retains_and_releases`
    in `crates/pycc_codegen`'s own test module, which read the emitted LLVM
    IR through `compile_to_object_with_observer` and additionally run LLVM's
    module verifier. They live in-crate because that observer is private.
  - `emit_bigint_refcount_call` splits the current basic block. Any caller
    that records a block for a phi incoming edge must re-read
    `get_insert_block()` *after* the call; `MirStmt::ForRange` and the three
    comprehension emitters carry that requirement in their own comments.
  - A future change must not route `range`'s operands through
    `retain_if_int_duplicate` in addition to the loop's own retains: `start_v`
    would then be retained twice and released once, permanently leaking every
    named `range` bound, and no value assertion would notice.
