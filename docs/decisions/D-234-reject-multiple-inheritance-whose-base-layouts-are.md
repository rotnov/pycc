---
id: D-234
title: "Reject multiple inheritance whose base layouts are not prefixes of the derived layout"
status: accepted
---

## D-234: Reject multiple inheritance whose base layouts are not prefixes of the derived layout

- Status: accepted
- Context:
  [D-154](./D-154-class-instance-runtime-layout-stays-opaque.md) makes an
  instance a flat array of attribute slots and lowers every method exactly
  **once**, against its own class's layout, with static dispatch and no vtable.
  A derived class's layout is computed from its MRO most-base-first, each
  unique attribute name taking one slot. Those two facts are only compatible
  while every class in the MRO sees its own attributes at the *same slot
  indices* it was lowered against — that is, while every ancestor's layout is
  a **prefix** of the derived layout.

  It is not, in general. For `class A: self.a` / `class B: self.b` /
  `class C(A, B)`, the MRO is `[C, A, B]`, so the layout walk assigns `b` slot
  0 and `a` slot 1, while `A`'s own methods were lowered against `A`'s layout
  `[a]` and still read slot 0. The failure is not uniformly loud: the slot the
  running constructor never wrote aborts in `pycc_rt`
  (`invalid encoded int word 0x0`, the documented exit-`101` boundary), while
  the slot it aliased over returns a **silently wrong value** where CPython
  raises `AttributeError` or prints a different number. Issue
  [#969](https://github.com/rotnov/pycc/issues/969); the reviewed plan is that
  issue's plan comment. The shape was previously pinned as a known limitation
  in `tests/issue_966_inherited_init_rank.rs`
  ([D-232](./D-232-rank-an-implicit-object-style-constructor-last-in.md)),
  which fixed constructor *ranking* over the same class shapes but explicitly
  left the layout defect open.

- Decision: follow the precedent
  [D-224](./D-224-restrict-class-level-attributes-to-scalar.md) sets for a
  shape the flat-slot model cannot represent -- reject it with `C0001` at
  HIR-lowering time rather than mis-compile it -- and change no lowering. `pycc_hir`'s `validate_mro_slot_layout`
  (`crates/pycc_hir/src/class/mro.rs`) runs at the end of `lower_class`, on the
  finished `HirClassDef`, and rejects with `C0001` when the class has more than
  one base and some class in its MRO has a flat layout whose **attribute-name
  sequence is not a prefix** of the derived class's. The diagnostic names the
  offending ancestor and states that its layout is not a prefix of the derived
  class's, so its own methods would address the wrong attribute slots.

  The layout walk itself moves out of `pycc_mir` into
  `pycc_hir::flat_attr_layout`, taking an already-resolved `&[&HirClassDef]`;
  `pycc_mir::class::mro_attrs` keeps its signature, its MRO-def collection loop
  and its ghost-class panic path, and delegates the walk. The gate and the
  lowering therefore cannot drift on what counts as a slot — merged
  `@dataclass` fields, a builtin exception class's empty attribute list.

  The placement is forced: `attrs` is only complete after the `@dataclass`
  field merge assigns it, well below `walk_class_body`, and `validate_bases`
  runs before any attribute is known. A base is always defined earlier in the
  module, so every base's `attrs` is final by the time the gate reads it.

- Alternatives:
  - **Reorder the slot walk.** Impossible, not merely expensive. Whenever two
    classes in one MRO have layouts that agree on slots `0..k` and then name
    different attributes at slot `k`, no single flat ordering can put both
    names at index `k`, so both cannot be prefixes of the derived layout. Two
    non-empty disjoint base layouts are the degenerate `k = 0` case.
  - **Refine the gate to reachability** ("reject only when a mis-addressed
    member can actually run"). Unsound: `super()` re-enters a member the
    derived class shadows. With `class B: self.n; def f` / `class C: self.X` /
    `class D(B, C)` overriding both `__init__` and `f` as `return super().f()`,
    nothing of `B`'s is reachable by name from `D`, yet `B.f` runs and reads a
    mis-addressed slot. Pinned in `tests/issue_969_mi_slot_layout.rs`.
  - **A body-walk plus `super()`-scan variant** of the same refinement: roughly
    three times the code under the D-014 100%-region gate, and it needs base
    method bodies at a seam that has only `HirClassDef`s. Deferred, not
    rejected on merit.
  - **The real fix — per-derived-class re-lowering of inherited methods, or a
    runtime name-to-slot indirection.** This is an architectural change to
    D-154's one-lowering-per-method flat-slot model, spanning `pycc_mir`,
    `pycc_codegen` and `pycc_rt`. Explicitly foreclosed for now; it is the
    tracked path that would lift this restriction, and D-154 itself is left
    unedited (an accepted entry is never rewritten).

- Consequences:
  - A whole class of silently wrong answers becomes a compile error at the
    class header. Accepted shapes are unchanged: single inheritance of any
    depth, a methods-only mixin or class-attribute-only second base, two bases
    declaring the *same* attribute names (they share one slot, so
    `tests/fixtures/pep_3135_super.py`'s `class Mixed(Slow, Fast)` stays
    green), a builtin exception base, a diamond with attribute-free
    intermediates, and a one-sided diamond where only one branch adds an
    attribute.
  - **Two deliberate narrowings**: programs CPython runs and pycc compiled
    correctly are now rejected, because the gate is a layout predicate and not
    a reachability analysis.
    1. `tests/issue_915_super_class_attr.rs`'s `class D(B, C)` where `B`
       declares a class attribute plus `self.n` and `C` declares `self.X`:
       layout(`D`) = `[X, n]`, layout(`B`) = `[n]`. It printed the right answer
       because nothing on the mis-addressed path ran. #915's own `super()`
       resolution is unchanged; only this class shape no longer reaches it.
    2. A diamond `Base`/`A(Base)`/`B(Base)`/`C(A, B)` in which **both**
       branches add an instance attribute of their own.
    `crates/pycc_types/src/tests.rs`'s in-crate mirror of narrowing 1 keeps
    covering the same `super()` path with `B`'s instance attribute dropped,
    which leaves `B`'s layout empty and therefore a prefix of everything.
  - **The name-only comparison depends on `T0052`.** A slot is (name, type),
    but the predicate compares names alone. That is sufficient only because
    [D-210](./D-210-reject-cross-mro-attribute-redeclaration-with-a.md)'s
    `T0052` (`crates/pycc_types/src/redeclaration.rs`) independently rejects
    two classes in one MRO declaring the same attribute name with differing
    types, so equal names imply equal slot types. Relaxing `T0052` requires
    revisiting this gate. Recorded in `validate_mro_slot_layout`'s own doc
    comment.
  - **One ordering change**, scoped to the module-level `T0052` in
    `pycc_types`: this gate runs during HIR lowering, ahead of type checking,
    so a program violating both is now reported as `C0001` rather than
    `T0052`. `pycc_hir`'s own `@dataclass` field-merge `T0052` is unaffected —
    it still fires earlier in the same `lower_class` call. No test pins the
    module-level combination.
  - **Enum classes are irrelevant here for a reason worth recording.** They do
    carry two real slots (`value`, `name`), but
    [#941](https://github.com/rotnov/pycc/issues/941)'s `validate_bases` gate
    rejects using an enum class as a base at all, so those slots can never
    enter another class's MRO. If that gate is ever relaxed, this one must be
    revisited. Protocol classes genuinely carry no attributes.
  - A user who hits a narrowing has no in-language workaround short of
    restructuring the class hierarchy; the diagnostic says so in D-224's
    "not supported yet" wording, and this entry names the redesign that would
    lift it.
