---
id: D-006
title: "Generics: monomorphization; vtable dispatch only for explicit dynamic-Protocol use"
status: accepted
---

## D-006: Generics: monomorphization; vtable dispatch only for explicit dynamic-Protocol use
- Status: accepted
- Context: pycc is an AOT compiler for statically-typed Python. The core
  question for generics and protocol-typed values is whether method dispatch
  is resolved at compile time (monomorphization — one specialized copy per
  concrete type used at each call site) or at runtime (vtable — one shared
  copy, indirect dispatch through a per-instance type tag). D-160 (accepted)
  already established that v0.3 uses static dispatch exclusively: no
  vtables, no runtime type tags, no virtual method tables. `super()`
  resolves at compile time to the defining class's own MRO, not the
  caller's. Cooperative multiple inheritance is an accepted limitation.
  PEP 544 protocols (issue #380) are the natural extension of this model:
  a protocol defines a structural interface, and a concrete class conforms
  if it has every required method/attribute with compatible types. The
  conformance check is a compile-time operation against `HirClassDef`
  shapes — no runtime type tags, no vtables. Protocol-typed function
  parameters are monomorphized: for each concrete type passed as a
  protocol-typed parameter, a specialized copy of the function is created
  with `Ty::Protocol("P")` replaced by `Ty::Instance("C")` throughout the
  body, following the existing `0gen_` mangling convention (D-134).
- Decision: **Accept the first clause — static dispatch via
  monomorphization for all generic and protocol-typed code.** Every method
  call resolves to a compile-time-known mangled function symbol. No
  vtables, no runtime type tags. Protocol-typed parameters are
  monomorphized per concrete call-site type, reusing D-134's existing
  `0gen_` infrastructure. **Drop the `--opt-size` clause** — the
  `--opt-size` flag does not exist in pycc and was never implemented. If a
  future `--opt-size` flag is added, it would get its own decision entry at
  that time. The "explicit dynamic-Protocol use" clause (vtable dispatch
  for an explicit `dyn`-like qualifier) describes a **post-v0.3 feature**,
  not something v0.3 itself implements — D-160's accepted answer (no
  runtime polymorphism in v0.3) is the direct precedent.
- Alternatives:
  - **Vtable dispatch for all protocol-typed values.** Rejected by D-160:
    v0.3 has no runtime polymorphism. Introducing vtables for protocols
    would contradict the accepted decision and require runtime type tags
    on every instance — a cross-crate ABI change D-154 explicitly avoided.
  - **Type erasure without monomorphization for protocol-typed parameters.**
    Rejected: without monomorphization, the codegen would not know which
    concrete method to call for `x.method()` inside `def f(x: Drawable)`.
    Type erasure works for local variables (where the RHS provides the
    concrete type) but not for function parameters (where the concrete type
    is only known at the call site).
- Consequences:
  - **One variable, one concrete type.** A protocol-typed local variable
    (`x: Drawable = Circle()`) has its concrete representation fixed by
    the first assignment (D-040 sticky-representation rule). A later
    assignment with a *different* concrete type is rejected with `T0023`
    — this is the same limitation D-160 imposes on all polymorphism in
    v0.3.
  - **Protocol-typed function parameters are monomorphized.** Each
    concrete type passed as a protocol-typed parameter produces a
    specialized function copy. The specialized function's mangled name
    follows the existing `0gen_` convention (e.g.
    `0gen_f__Drawable_Circle`).
  - **No cooperative multiple inheritance for protocols.** Same
    limitation as D-160: `super()` resolves against the defining class's
    own MRO, not the caller's.
  - **`isinstance` against `@runtime_checkable` protocols is a
    compile-time structural conformance check**, not a runtime type-tag
    lookup. `issubclass` against a protocol is rejected (PEP 544 disallows
    structural `issubclass`).
  - **Post-v0.3: explicit `dyn`-like qualifier.** A future version may
    introduce an explicit `dyn`-like qualifier for opt-in vtable dispatch,
    at which point a new decision entry would record that design. v0.3
    does not implement this.
