---
id: D-187
title: "Widen a bool into an int-declared attribute slot in MIR, not in codegen"
status: accepted
---

## D-187: Widen a bool into an int-declared attribute slot in MIR, not in codegen

- Status: accepted (issue [#627](https://github.com/rotnov/pycc/issues/627)).
  **Corrects [D-180](./D-180-refcount-heap-bigints-and-release-them-at-named.md)
  *Consequences* item 6 only.** D-180 as a whole stays accepted and is not
  superseded: its refcount representation, its guarded-call and
  block-split invariants, its release sites, and every other residual item
  in that list are unchanged. The single claim this entry corrects is item
  6's assertion that the bool-into-`int`-attribute leak "cannot" be pinned
  by a test because the shape reads back as `0` no matter what the
  refcounting does. That is no longer true, and the same change that makes
  it false also makes `MirStmt::AttrSet`'s release gate agree with D-180
  *Decision* item 4's slot-typed invariant -- by coincidence rather than by
  construction, for the one widening that exists today. The gate itself
  stays value-typed; see *Decision* below for why that distinction matters.
- Context:
  `pycc_types` accepts `bool` where `int` is declared -- `int` admits
  `bool` as a subtype at a checked boundary
  ([docs/TYPE_SYSTEM.md](../TYPE_SYSTEM.md)) -- and an instance-attribute
  store is such a boundary. D-141 makes that widening *observable*: a
  `bool` crossing into an `int`-typed location keeps its runtime identity
  and prints `True`/`False`, not `1`/`0`.

  Every other boundary already honoured this. An annotated assignment
  (`x: int = True`) wraps the value in `MirExpr::IntBoundary` during HIR to
  MIR lowering; a call argument, a return value, and a `@property` setter
  all pass through `coerce_scalar_to_type`, which encodes the word. The
  direct `MirStmt::AttrSet` slot path did not. `scalar_to_slot_word` stored
  the raw `bool` as a `zext`, so `c.n = True` on an `int`-declared
  attribute wrote the word `1`, which `classify_encoded_int` reads as the
  smallint `0`; `c.n = False` wrote the word `0`, which is not a valid
  encoded int word at all, aborting the next read with
  `pycc_rt: invalid encoded int word 0x0` (exit 134). The same program
  spelled through a `@property`/setter pair already printed `True`, so the
  compiler was internally inconsistent with itself.

  The same defect had a second face. `MirStmt::AttrSet` carries only a slot
  *index*, so codegen gates its pre-store bigint release on the value's
  `Ty`. A `bool` value reported `Ty::Bool`, the gate did not fire, and a
  bigint already in the slot was stranded -- D-180 *Consequences* item 6.
- Decision:
  Encode the widening in MIR, at the point where the declared slot type is
  still in hand. `pycc_mir`'s `HirStmt::AttrSet` arm takes the slot's `Ty`
  from the same `mro_attrs` tuple its slot index came from, and wraps the
  lowered value in `MirExpr::IntBoundary` when that `Ty` is `Int` while the
  value's own `ty()` is `Bool`. This is the mechanism D-141 names -- "MIR
  represents an annotated initializer boundary with `MirExpr::IntBoundary`,
  not synthetic arithmetic" -- and it mirrors the `HirStmt::AnnAssign` arm
  almost line for line.

  Because `MirExpr::IntBoundary::ty()` is `Ty::Int`, codegen's existing
  value-typed release gate now fires for this shape, closing D-180
  *Consequences* item 6 without adding a `ty` field to `MirStmt::AttrSet`.
  The gate stays *literally* value-typed and only **coincides** with
  D-180 *Decision* item 4's slot-typed invariant, because `bool` into
  `int` is the sole widening `pycc_types` permits at an attribute
  boundary -- `bool` into a `float` attribute is rejected with
  `error[T0021]`. A future widening would break the coincidence silently;
  that risk is recorded in the `MirStmt::AttrSet` codegen arm's own
  comment.

  `scalar_to_slot_word` and `slot_word_to_scalar` are deliberately left
  untouched, and `MirStmt::AttrSet`'s shape is unchanged.
- Alternatives:
  - *Emit D-141 markers inside `scalar_to_slot_word`'s `Scalar::Bool` arm*
    (issue #627's own literal completion criterion 1, signature unchanged).
    Rejected: that function is also the store path for `bool`-**declared**
    slots, and its mirror reads a `Ty::Bool` slot with
    `build_int_truncate(raw, i8)`. The `False` marker `0b0010` truncates to
    a non-zero `i8`, so `c.flag = False; print(c.flag)` would print `True` --
    a worse bug than the one being fixed. The criterion is implementable
    only by additionally threading the declared slot `Ty` into
    `scalar_to_slot_word`, which is the next alternative.
    `tests/fixtures/bool_int_runtime_identity.py` now carries a
    `bool`-declared attribute store specifically so a future attempt at this
    fails loudly.
  - *Add a `ty` field to `MirStmt::AttrSet` and thread the declared type
    into codegen.* This is D-180's own "rejected for Part 1" alternative,
    and it additionally churns every `MirStmt::AttrSet` literal across the
    MIR and codegen test modules. `MirExpr::IntBoundary` is a third path
    D-180 did not consider, not a reversal of that rejection.
  - *Reject `bool`-into-`int`-attribute in `pycc_types`.* Contradicts
    `docs/TYPE_SYSTEM.md`'s subtype rule, and would have to reject the
    already-accepted `annotated: int = True` shape too.
  - *Coerce in codegen via `coerce_scalar_to_type`.* Works, but puts the
    boundary decision in the backend when D-141 names MIR as its home, and
    leaves `value.ty()` reporting `Bool`, so D-180 *Consequences* item 6
    would stay open.
- Consequences:
  - `obj.attr = <bool>` into an `int`-declared attribute now prints
    `True`/`False` byte-for-byte with CPython instead of printing `0` and
    then aborting. Pinned by
    `tests/fixtures/bool_int_runtime_identity.py`, which gained a class
    with both an `int`-declared and a `bool`-declared slot.
  - D-180 *Consequences* item 6 is closed. Its evidence is a composition of
    two pins, neither sufficient alone: `pycc_mir`'s
    `an_attr_set_of_a_bool_into_an_int_declared_slot_widens_through_int_boundary`
    pins that the lowering now produces a `Ty::Int`-reporting value, and
    `pycc_codegen`'s
    `an_int_attribute_slot_store_of_a_bool_emits_a_guarded_release` pins
    that such a value emits the guarded release. The end-to-end effect is
    measured by
    `a_bool_overwriting_a_bigint_attribute_does_not_grow_with_the_iteration_count`
    in `tests/issue_146_bigint_release.rs`, which reads a peak-RSS ratio of
    ~1.92 before this change and passes its `< 1.35` bound after.
  - The widening keys off the base expression's **static** type. A
    base-class method assigning a `bool` into an attribute a derived class
    re-declares as `int` still missed it, and reached the same unencoded-word
    path this decision fixes for the direct case -- with the same asymmetry
    the *Context* section describes: `True` read back as the smallint `0`,
    while `False` aborted the next read with `pycc_rt: invalid encoded int
    word 0x0` (exit 134). This was unchanged by this decision, not a
    regression it introduced, and has since been resolved by
    [D-209](./D-209-reject-cross-mro-attribute-redeclaration-with-a.md)
    (issue [#676](https://github.com/rotnov/pycc/issues/676)): a conflicting
    cross-MRO attribute redeclaration is now diagnosed with `T0052` at
    class-definition time rather than coerced silently, closing both this
    direct symptom and its read-side twin.
  - Every other `scalar_to_slot_word` caller is unaffected, since the
    function is untouched. `emit_enum_member_inits`, its only other caller,
    passes statically known `Int` and `Str` scalars.
