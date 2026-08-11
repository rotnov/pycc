---
id: D-160
title: "super() uses static dispatch (no runtime polymorphism)"
status: accepted
---

## D-160: super() uses static dispatch (no runtime polymorphism)
- Status: accepted
- Context: Issue #433 requires implementing zero-argument `super()` (PEP 3135)
  and base class `__init__` calls. In CPython, `super()` resolves the next
  class in the MRO of `type(self)` — the most-derived type — at runtime. This
  enables cooperative multiple inheritance: a method in class `B` calling
  `super().method()` dispatches to the class after `B` in the *caller's*
  MRO, not `B`'s own MRO. The project's existing architecture (D-006) uses
  static dispatch exclusively — no vtables, no runtime type tags, no
  virtual method tables. Every method call resolves to a compile-time-known
  mangled function symbol. The question is whether `super()` should
  introduce runtime polymorphism to match CPython's cooperative semantics,
  or stay within the static-dispatch model.
- Decision: **Option A — static dispatch only.** `super()` resolves at
  compile time to the next class after the *defining* class in that class's
  own MRO, not the caller's MRO. `super().method(args)` lowers to a direct
  call to the resolved base method's mangled name, with `self` (the current
  method's first parameter) prepended as the implicit first argument. No
  vtable, no runtime type inspection, no new MIR or codegen nodes — the
  lowering reuses the existing `MirExpr::Call` infrastructure. This is
  consistent with D-006's monomorphization-only framing and with how
  inherited methods are already lowered (the `self` parameter is typed as
  the defining class, not the derived class, but the same object is passed).
- Alternatives:
  - **Option B — runtime polymorphism via vtable.** `super()` would use
    `type(self).__mro__` at runtime to find the next class after the
    defining class in the *instance's* MRO. This would match CPython's
    cooperative multiple inheritance semantics exactly, but requires a
    vtable or type-tag per instance, a runtime MRO table, and a dispatch
    mechanism — all of which D-006 explicitly rejects. The performance and
    complexity cost is disproportionate for a v0.x compiler targeting AOT
    compilation of statically-typed Python subsets.
  - **Option C — per-call-site monomorphization.** For each concrete
    `super().method()` call site, generate a specialized version of the
    method with `super()` resolved against the caller's MRO. This would
    achieve cooperative semantics without runtime dispatch, but requires
    whole-program call-graph analysis and method duplication, which is
    out of scope for the current architecture and adds significant
    complexity to the MIR/codegen pipeline.
- Consequences:
  - **Cooperative multiple inheritance does not work as in CPython.** In a
    diamond inheritance pattern (`D(B, C)` where both `B` and `C` inherit
    from `A`), `super()` inside `B.method()` resolves to `A` (the next
    class after `B` in `B`'s own MRO `[B, A]`), not `C` (the next class
    after `B` in `D`'s MRO `[D, B, C, A]`). This means `C.method()` is
    skipped. This is an accepted limitation of the static-dispatch model.
  - **Single-inheritance `super()` works correctly.** `super().__init__()`,
    `super().method()`, and multi-level chains (`C` → `B` → `A`) all
    resolve and dispatch correctly, since each class's own MRO is a linear
    chain in the single-inheritance case.
  - **`super().__init__()` with arguments works correctly.** Arguments are
    type-checked against the base class's `__init__` signature (excluding
    `self`), and the call lowers to a direct call to the base `__init__`
    with `self` prepended.
  - **`super().attr` (attribute read) works correctly.** The attribute is
    resolved starting from the next class in the defining class's MRO, and
    the slot index is computed from the full MRO's flat layout (the same
    index the base class's own methods would use).
  - **No new MIR or codegen nodes.** `super().method()` lowers to
    `MirExpr::Call` and `super().attr` lowers to `MirExpr::AttrGet` or
    `MirExpr::Call` (for properties), reusing existing infrastructure.
  - **HIR carries a new `HirExpr::Super` variant** that only ever appears
    as the `base` of a `MethodCall` or `AttrGet`. A bare `super()` is
    rejected at HIR-lowering time with `C0001`.
