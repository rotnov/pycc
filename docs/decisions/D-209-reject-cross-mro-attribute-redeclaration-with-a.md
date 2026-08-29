---
id: D-209
title: "Reject a cross-MRO attribute redeclaration with a differing declared type"
status: accepted
---

## D-209: Reject a cross-MRO attribute redeclaration with a differing declared type

- Status: accepted (issue [#676](https://github.com/rotnov/pycc/issues/676)).
- Context:
  [D-187](./D-187-widen-a-bool-into-an-int-declared-attribute.md) fixed a
  `bool`-into-`int`-attribute widening for a single class, but its own
  *Consequences* section flagged a residual gap left open for #676: the
  widening keys off the base expression's **static** type, so a
  base-class method that assigns into `self.<attr>` resolves the slot's
  declared type from *that base class's own* MRO-attribute entry
  (`pycc_mir`'s `mro_attrs`/`class_def_of`), while nothing stopped a
  derived class from redeclaring the same attribute name with an
  incompatible type. Issue #676 filed this as a bool/int-specific
  asymmetry (`Base.set_true` setting `self.v = True` on a `Derived`
  instance whose own `__init__` redeclares `self.v = 0`), where `True`
  read back as the smallint `0` instead of a diagnostic or the correct
  value.

  Investigating #676 found the defect is not bool/int-specific. The same
  static-type/MRO-walk architecture already let a **compiling,
  non-diagnosed program segfault today** for a `float`/`int` pair: a base
  class declares `self.v = 1.0` in `__init__` and assigns `self.v = 2.5`
  in another method; a derived class's own `__init__` redeclares
  `self.v = 1` (an `int`); calling the base method on a `Derived`
  instance then reads/writes the slot as the wrong scalar representation
  and aborts at run time (exit 139). Any fix scoped narrowly to bool/int
  would leave this general case open.
- Decision:
  Reject any cross-MRO attribute redeclaration with a differing declared
  type, at class-definition time, with a new diagnostic `T0052`, rather
  than attempting to coerce at the assignment site.

  The check is a new whole-module validation pass in `pycc_types`
  (`check_incompatible_attribute_redeclarations`), called from
  `pycc_types::check` and from `checked_function_signatures` (so both the
  `pycc check` path and the `pycc build`/`check_and_resolve` path reject
  it, matching how `check_incompatible_redefinitions` is already called
  from both places) — before any `Environment`/signature-inference work,
  since it only needs each class's own already-lowered `attrs` and `mro`,
  both populated at HIR-lowering time.

  The comparison rule is symmetric and direction-independent: for each
  class `C`, walk `C`'s own C3-linearized MRO (`HirClassDef::mro`, which
  includes `C` itself and every ancestor). For every unordered pair of
  distinct classes `(X, Y)` drawn from that MRO where both declare an
  attribute of the same name in their own (non-inherited) `attrs`, if the
  declared `Ty` differs, emit `T0052` — regardless of which type is
  "wider", and regardless of whether `X`/`Y` are in an ancestor-descendant
  relationship. This also rejects a diamond conflict between two sibling
  base classes that neither is the other's ancestor, through their common
  descendant's own MRO, even when that descendant never redeclares the
  attribute itself. Only a *differing* type triggers the diagnostic: an
  identical redeclaration across the MRO (the existing `mro_attrs` dedup
  case) and an ordinary same-class assignment of an admissible narrower
  value (D-187's `bool`-into-`int` case) are both unaffected, since the
  check only ever compares two distinct classes' own declared types,
  never a single class's declared type against an assigned value.
- Alternatives:
  - *Coerce at the assignment site*, mirroring D-187's own resolution for
    the single-class case. Rejected as unsound, not merely less elegant:
    `Base.set_true`'s method body is compiled exactly once (static
    dispatch, no vtable, no per-subclass monomorphization —
    `docs/TYPE_SYSTEM.md`'s own architecture statement). An unconditional
    coercion inserted at that one compiled call site cannot distinguish,
    at run time, a bare `Base` instance (slot genuinely `bool`) from a
    `Derived` instance (slot re-declared `int`) — there is no per-instance
    type tag it could branch on. A decisive counter-example proves this:
    a bare `Base()` instance calling `Base.set_true()` today correctly
    prints `True` (no `Derived` in play at all); an unconditional coercion
    at that shared call site would corrupt this already-correct case
    instead of fixing the reported one. There is no third option without
    whole-program closed-world specialization, which is out of scope here
    and not how pycc's method-compilation model works.
  - *Thread the declared slot type through the read/write MIR sites and
    branch per instance.* Would need a per-instance runtime type tag pycc
    does not have; rejected for the same reason as coercion above.
  - *Scope the fix to the bool/int pair the issue reported.* Rejected:
    the same architecture already produces an undiagnosed segfault for
    other type pairs (e.g. `float`/`int`), so a bool/int-only rule would
    leave a real, worse-severity defect (memory-unsafety-class runtime
    abort) open. A general type-equality comparison costs no more to
    implement than a type-pair-specific coercion.
  - *Classify this as a capability gap (`C0001`) rather than a type
    error.* Rejected: `mypy --strict` already rejects this shape as a
    Liskov-substitution violation ("Definition of attribute ...
    incompatible with supertype"). CPython runs the program, but no
    conforming static type checker accepts it, so labeling it a `T00NN`
    type error is honest and matches the closest structural precedent,
    `T0031` (an `@override` violation: also a subclass shape that is
    dynamically legal in CPython but statically unsound).
- Consequences:
  - Both the write-side symptom #676 reported (`HirStmt::AttrSet`,
    `pycc_mir/src/stmt.rs`) and its read-side twin (`HirExpr::AttrGet`,
    `pycc_mir/src/expr.rs`) are closed for free: a program reaching either
    site with a divergent cross-MRO attribute type is now rejected at
    class-definition time, before MIR lowering ever runs. No branch was
    added at either site, at `matching.rs`'s pattern-match attribute
    access, or at `mro_attr_count`'s slot allocation — every one of those
    sites only ever sees a single consistent `Ty` per attribute name
    across the MRO by construction now.
  - D-187's residual bullet (the MRO gap left open for #676) is resolved:
    a base-class method assigning into an attribute a derived class
    redeclares with a different type is now a compile-time `T0052`, not a
    silent mis-decode or runtime abort.
  - `mro_attrs_overrides_type_for_a_redeclared_attribute_with_a_different_type`
    (`crates/pycc_mir/src/tests/class_mro.rs`) constructs its `HirClassDef`s
    directly and bypasses `pycc_types::check` entirely, so it remains valid
    after this change and pins `mro_attrs`'s own internal override
    mechanics independent of whether real Python source can ever reach
    that state through the type checker.
  - `scalar_to_slot_word`/`slot_word_to_scalar` stay untouched, as does
    D-187's own decision and rationale — this entry only resolves D-187's
    forward reference to #676, it does not correct D-187 itself.
