---
id: D-181
title: "Release a heap bigint's birth reference at every consuming site"
status: accepted
---

## D-181: Release a heap bigint's birth reference at every consuming site

- Status: accepted (Part 2 of issue
  [#146](https://github.com/rotnov/pycc/issues/146), tracked as
  [#625](https://github.com/rotnov/pycc/issues/625)). **Narrows**
  [D-180](./D-180-refcount-heap-bigints-and-release-them-at-named.md) rather
  than superseding it. D-180's refcount header, its two runtime entry points,
  its inline guard, its storage-slot-typed release gating, its retain site
  list, and six of its seven residual accepted leaks are all unchanged and
  remain accepted. The single half this decision narrows is D-180's residual
  item 7 -- unbound arithmetic temporaries, including bigint literals -- which
  is now bounded to the two cases enumerated under *Consequences* below
  instead of covering every temporary a program evaluates.
- Context:
  D-180 gave `BigIntObj` a refcount and retired it wherever a *named* storage
  location stops referring to a word. That covers assignment targets, `return`
  values, instance attributes, call arguments, and loop induction variables --
  every place a word has a home. It does not cover the words that never get
  one.

  Every `pycc_rt_int_*` bigint result is born owning exactly one reference
  (`BigIntObj::new` sets `rc == 1`, `tag_bigint` is the only path that hands
  the object out as a word), and `int_const::emit_int_constant` allocates one
  per evaluation for a literal outside [D-061](./D-061-int-is-a-tagged-63-bit-smallint-with-a-heap.md)'s
  tagged range. When such a word is consumed and discarded -- an inner
  operand of a larger expression, a discarded statement result, an `if`
  condition, a `print` argument -- nothing ever retired that birth reference.
  Measured against D-180's own `tests/issue_146_bigint_release.rs` harness and
  its `< 1.35` ratio gate, `y = (x + 1) + 2` in a loop peaked at 26,066,944
  bytes over 500k iterations against 50,167,808 over 1M (ratio 1.925), and
  `y = (x + x) + x` at 26,083,328 against 50,249,728 (ratio 1.926). The leak
  is trip-count-linear, exactly the property D-180 set out to remove.

  Neither of those two forms exercises the *literal* allocation: `1` and `2`
  are inside D-061's tagged range and `int_const::fits_tagged_smallint` folds
  them to immediates. A third acceptance form with an out-of-range literal is
  what covers that path.
- Decision:
  1. Ownership is decided at compile time from the shape of the *source
     expression*, never from the word. `int_value_is_a_duplicate_reference`
     classifies an expression as **borrowed** (it yields a second reference to
     a word something else owns) or **owning** (it freshly constructs a value
     already holding exactly one reference). `release_if_int_temporary` emits
     a guarded `pycc_rt_bigint_release` at a consuming site when, and only
     when, the source expression is owning.
  2. Three borrowed shapes: `Name { ty: Int }`, `AttrGet { ty: Int }`, and a
     `Ty::Int` `Subscript` on a **tuple** base. The tuple case is the one
     D-180 did not have to consider. `MirExpr::TupleLiteral` inserts the
     element word it is handed without retaining it, and `MirExpr::Subscript`'s
     tuple branch returns that same word unchanged, so a tuple field is a pure
     alias of whatever supplied it. Classifying it owning would make
     `t = (x + x, 1); y = t[0] + t[0]` a double free.
  3. The classification is an **exhaustive `match`**, not a `matches!`. A new
     `MirExpr` variant must then be a compile error here rather than a silent
     default. Every remaining variant shares one combined arm so the "owning"
     answer costs one coverage region rather than twenty.
  4. The retain side keeps its **own** predicate, deliberately not shared with
     the release side even though the two agree on today's three borrowed
     shapes. They fail in opposite directions: a missing retain leaks, while a
     missing "borrowed" classification on the release side frees a live word.
     Sharing one predicate would make every future ownership refinement a
     simultaneous edit to an over-approximating and an under-approximating
     consumer, with no compiler help distinguishing them.
  5. `retain_if_int_duplicate` gains a matching tuple-`Subscript` arm, because
     binding a tuple element to an `int` local makes that local a second owner
     of a word the tuple's supplier still holds. The arm is balanced at each of
     that helper's four call sites: `MirStmt::Assign` by the slot's own
     release-before-store, `MirStmt::Return` by the caller's new release of the
     `Call { ty: Int }` result, and the call-argument and `MirStmt::AttrSet`
     sites not at all -- exactly D-180 residual 3's existing shape, not a new
     leak class.

     The arm carries an **unenforced precondition**: it is sound only while
     the tuple field holds no owned reference of its own, i.e. while the name
     that supplied the element is still bound to that word. See the known
     defect under *Consequences*.
  6. Release sites are the sites that consume a word and then discard it:
     both operands of a `Ty::Int` `BinOp`, both operands of a `Compare` on its
     int path, `MirStmt::ExprStmt`'s discarded result, five `truthy`
     conditions (`MirStmt::If`, `MirStmt::While`, and the `if`-filter of each
     of the three comprehension emitters), `print`'s own argument in
     `emit_eval_print_arg`, and `MirExpr::FString`'s `Interpolation` arm.
     Every one of them releases **after** the consuming call, never before:
     `truthy` and `to_str` both read a bigint's limbs, so releasing first
     could free the word being read.

     Deliberately **not** release sites, because the value's new home takes
     ownership there: the assignment value in `MirStmt::Assign`, the `return`
     value, a call argument, and an `AttrSet` value.
  7. `MirExpr` is a tree -- no short-circuit `BoolOp`, no ternary, no
     `AugAssign` node -- and this crate's emitters evaluate each node exactly
     once, so every produced word has exactly one consumer and each release
     dominates nothing else that reads it. **Dominance holds; exactly-once does
     not.** A [D-173](./D-173-guard-statement-effects-checks-for-a-pending.md)
     exception edge can branch out of an expression after an operand has been
     evaluated but before the release is reached. That skips a release, which
     can only leak -- see the residual list below.
  8. Other container egress (`DictGet`, `ListPop`, `DictGetOrDefault`, and the
     *list* branch of `Subscript`) classifies **owning**, not borrowed.
     [D-141](./D-141-int-is-one-encoded-i64-word-across-every.md)'s container
     ingress rejects bigints outright, so a container payload is never a heap
     word and the emitted release's inline guard is false in practice. Owning
     is the classification that stays correct if that boundary is ever widened
     with a matching egress retain, which D-180 already requires of whoever
     widens it.
  9. `Call { ty: Int }` is owning **conditionally**, and the premise is stated
     rather than assumed: a user function's `return` runs
     `retain_if_int_duplicate` on its value (D-180 decision 6) and that retain
     has no matching release at the callee's boundary (D-180 residual 3), so a
     returned word does arrive carrying a reference the caller may retire. If
     that boundary ever gains its own release, this classification must change
     with it. `len` needs no such argument: it lowers to
     `Call { callee: "len", ty: Int }` whose result is `raw_i64_to_tagged_int`'s
     odd-tagged smallint, so its release is an unconditional runtime no-op.
     There is no `MirExpr::Len` variant.
 10. Loop bounds take **two different recipes**, because the two emitter
     families do not have the same ownership contract.

     `MirStmt::ForRange` retains all three operands (D-180 decision 7), so the
     source expression's birth reference is retired immediately after those
     retains: the loop is already an owner by that point, and the object
     survives to `for_after`. For a fresh `start` operand and `n` executed
     iterations the arithmetic is unchanged from D-180's -- `n + 1` owned
     `current` values against `n` per-iteration releases plus one in
     `for_after` -- with the fresh operand contributing one extra birth
     reference matched by exactly one extra release in the preheader, and
     `stop`/`step` contributing one each matched by the two existing
     `for_after` releases. The loop's own three retains are **not**
     conditionalized on operand shape; D-180 warns that making the loop's
     ownership depend on where its operands came from is what breaks that
     arithmetic.

     The three comprehension emitters retain only `start_v` and never retain
     `stop_v`/`step_v`, and `pycc_rt_range_continue` re-reads both on every
     iteration. A fresh `start` operand is therefore released immediately as
     above, but a fresh `stop`/`step` must be released in `after_bb` instead --
     past the last read. Releasing one at the point it is built would be a
     use-after-free on trip two.
 11. `pycc_rt`'s int arithmetic exports carry the **no-aliasing invariant** the
     operand releases depend on: no `int` operation may return an operand's own
     encoded word. Every bigint result is built through `tag_bigint` on a
     freshly constructed object. This was already true by construction; it is
     now a stated contract on those exports and is pinned by
     `an_int_operation_never_returns_an_operand_s_own_word`, because an
     identity fast path (`a + 0 -> a`) added later would turn every operand
     release into a use-after-free on the value just produced.
- Alternatives:
  - **A statement-scoped deferred release list.** Collect every owning word
    evaluated during a statement and release them all at the statement's end.
    Rejected: it needs per-statement state threaded through every emitter, it
    holds objects alive longer than necessary inside loop bodies, and it has
    the same exception-path hole as the per-site release without the per-site
    version's locality.
  - **One shared predicate for retain and release.** Rejected under decision 4:
    the two sides' failure modes are opposites, and a single predicate makes
    that asymmetry invisible at exactly the moment someone refines it.
  - **Refcount inside `pycc_rt`, returning borrowed results.** Have the
    arithmetic exports consume their operands' references and hand back a
    borrowed word. Rejected: it moves an ownership contract from the emitter,
    which knows each value's source expression, into a runtime that sees only
    an `i64`, and it makes every smallint call pay for a convention it does not
    need.
  - **Defer the `lib.rs` submodule extraction to its own change.** Rejected:
    AGENTS.md's decomposability rule applies to the part a task touches, and
    the refcount unit is exactly that part. Extracting it here keeps the rule
    satisfied without a separate refactor PR against a 19,975-line file.
  - **Fix the second tuple direction here too.** Rejected as scope: giving a
    tuple field a real owner means retaining at `TupleLiteral` ingress and
    finding a release site for a value with no scope, which is the
    container-refcounting work
    [D-107](./D-107-list-t-pointers-get-their-own-pycc-codegen-scalar.md)/[D-124](./D-124-dict-str-int-set-int-refcounting-stays-leak-only.md)
    already track. Recorded below as a known defect instead.
- Consequences:
  - The trip-count-linear temporary leak is gone for arithmetic operands,
    discarded statement results, conditions, `print` arguments, f-string
    interpolations, and freshly built `range` bounds -- including the
    per-evaluation bigint *literal* allocation. Three new peak-RSS ratio gates
    in `tests/issue_146_bigint_release.rs` cover the two nested-temporary forms
    and the literal form.
  - The `Scalar` enum is now `Copy`. Every payload is an inkwell SSA handle,
    itself `Copy`, so a `Scalar` names a value the module already holds and
    duplicating the handle duplicates nothing. The release sites need to hand a
    value onward *and* still classify the word it carried.
  - The refcount unit -- `BigIntRefcount`, `emit_bigint_refcount_call`,
    `release_int_slot_before_store`, `retain_if_int_duplicate`, and this
    decision's own predicate and helpers -- now lives in
    `crates/pycc_codegen/src/bigint_rc.rs`, following
    `crates/pycc_codegen/src/int_const.rs`'s precedent. Nothing else was
    relocated.
  - **Remaining residual leaks** from D-180 residual item 7, both memory-safe:
    1. A fresh `int` word stored into a `TupleLiteral` element. `t = (x + x, 1)`
       leaks one object per evaluation. This is deliberately *not* a release
       site: the tuple keeps holding the word, and releasing at ingress would
       free a value `Subscript` can still read back out. Closing it requires
       giving tuple fields a real owner, which is the same work item as the
       defect below.
    2. The exception-path skip of decision 7. A pending exception raised
       between an operand's evaluation and its release branches away before the
       release, leaking that operand. Enumerated here rather than fixed:
       tying releases to D-173's exception edges is unwinding work, not
       refcounting work.

    D-180's other six residual accepted leaks are unchanged.
  - **A known, unfixed memory-safety defect**, tracked as
    [#633](https://github.com/rotnov/pycc/issues/633) -- a use-after-free, not a
    leak, and therefore recorded separately from the residual list above. A
    tuple field holds a word it never retained, so overwriting the *supplying
    name* before reading the field frees the object the field still points at:

    ```python
    b: int = 4611686018427387904
    t = (b, 1)
    b = 0                          # releases b's word -> rc 0, freed
    c: int = 4611686018427387905   # reuses the freed BigIntObj
    y: int = t[0]
    print(y)                       # prints ...905, expected ...904
    ```

    The mirror direction -- reading the field into a local and then
    overwriting *that* local -- was the same defect from the other side and is
    fixed by decision 5's retain arm. It is pinned by
    `a_bigint_read_out_of_a_tuple_is_not_freed_by_overwriting_the_reader`. The
    direction above is left broken on purpose and is *not* asserted as a
    passing test, so nothing in the tree enshrines its wrong output.
  - `Compare` and the `int_mul`/`int_floordiv`/`int_floormod`/`int_pow`
    `BinOp`s cannot be exercised with a heap bigint at all: all of them route
    through `require_inline_int`, which aborts on a bigint operand (the
    capability gap `docs/ROADMAP.md` already records). Their release sites are
    pinned at codegen depth instead, by IR-observer tests reading the emitted
    module through `compile_to_object_with_observer`. Those tests do not call
    `module.verify()` themselves, for the D-029 reason D-180 already records.
