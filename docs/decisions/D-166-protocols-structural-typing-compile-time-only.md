---
id: D-166
title: "PEP 544 protocols and structural typing: compile-time-only interface descriptions with static dispatch via monomorphization"
status: accepted
---

## D-166: PEP 544 protocols and structural typing: compile-time-only interface descriptions with static dispatch via monomorphization
- Status: accepted
- Context: PEP 544 introduces `Protocol` classes for structural typing — a
  class `C` structurally conforms to protocol `P` if `C` has every method
  and attribute `P` requires, with compatible signatures/types. In
  CPython, protocols are purely a static-type-checking construct with no
  runtime representation, except that `@runtime_checkable` protocols
  support `isinstance` checks (which only verify attribute *presence*, not
  signature compatibility). pycc's architecture (D-006, D-160) uses static
  dispatch exclusively — no vtables, no runtime type tags. The question is
  how to represent protocols in pycc's type system and how to handle
  protocol-typed values at codegen time.
- Decision:
  1. **New `Ty::Protocol(Box<String>)` variant.** Carries the protocol
     class name. Distinct from `Ty::Instance` so that `is_assignable` and
     every match arm can distinguish "this value is an instance of a
     concrete class" from "this value is typed as a protocol." `Box<String>`
     (not `Box<str>`) maintains the D-109 16-byte `size_of::<Ty>()`
     ceiling, mirroring `Ty::Instance`'s own documented reasoning.
  2. **`HirClassDef` gains `is_protocol: bool`, `runtime_checkable: bool`,
     `protocol_members: Vec<ProtocolMember>`, and `abstract_methods:
     Vec<String>` fields.** A `ProtocolMember` is either a method
     requirement (name + parameter types + return type) or an attribute
     requirement (name + type). A protocol class whose sole base is the
     `Protocol` marker has `bases = []` and `mro = [self_name]` (the
     `Protocol` base is consumed as a marker, exactly like `Enum`).
     Protocol inheritance (`class Q(P):` where `P` is a user-defined
     protocol) has `bases = ["P"]` and `mro = ["Q", "P"]` — `P` is a real
     base, and `Q` inherits `P`'s `protocol_members`. Protocol methods have
     `...` (Ellipsis) or `pass` bodies — they are never lowered to
     `HirItem::Function`s.
  3. **`Protocol` is a bare-name base marker** (like `Enum`), recognized in
     `lower_class` via `is_protocol_base_name("Protocol") -> bool`.
     `@runtime_checkable` is a class decorator recognized alongside
     `@dataclass`. `abc.ABC` is a bare-name base marker; `@abstractmethod`
     is a method decorator. All are recognized without requiring `from
     typing import ...` or `from abc import ...`, matching the
     `Final`/`Annotated`/`Enum` precedent.
  4. **Structural conformance checking** is a compile-time function
     `is_structurally_conforming(env, class_name, protocol_name) -> bool`.
     It walks the class's MRO to collect all methods, attributes, and
     properties, then checks that every `ProtocolMember` in the protocol's
     `HirClassDef` (including inherited members from protocol bases) is
     satisfied. "Compatible" uses the existing `is_assignable` for types
     and a signature-compatibility check for methods.
  5. **`is_assignable_with_env(env, from, to) -> bool`** wraps
     `is_assignable(from, to)`; if that fails and `to` is `Ty::Protocol`,
     it calls `is_structurally_conforming`. Every `is_assignable` call site
     in `pycc_types` that has `env` available is updated to use
     `is_assignable_with_env`.
  6. **Protocol-typed local variables** (`x: Drawable = Circle()`) are
     allowed. The type checker stores `Ty::Protocol("Drawable")` as the
     binding type, checks structural conformance, and type-checks
     method/attribute access against the protocol's `protocol_members`. The
     first assignment's concrete type determines the variable's concrete
     representation for codegen (D-040). A later assignment with a
     *different* concrete type is rejected with `T0023`.
  7. **Protocol-typed function parameters** are monomorphized. The
     monomorphization pass scans call sites and for each concrete type
     passed as a protocol-typed parameter, creates a specialized copy with
     `Ty::Protocol("P")` replaced by `Ty::Instance("C")` throughout the
     body, following the existing `0gen_` convention.
  8. **`isinstance(x, P)` against a `@runtime_checkable` protocol** is
     evaluated at compile time: the result is `true` iff `x`'s type
     structurally conforms to `P`. A non-`@runtime_checkable` protocol used
     in `isinstance` is rejected with `T0021`. `issubclass(C, P)` against a
     protocol is rejected with `T0021` (PEP 544 disallows structural
     `issubclass`).
  9. **`abc.ABC` and `@abstractmethod`** are compile-time-only markers. A
     class inheriting from `ABC` is abstract: it cannot be instantiated.
     `@abstractmethod`-decorated methods mark required overrides; a
     concrete subclass missing an override is rejected with `C0001`. No
     `ABCMeta` runtime machinery.
- Alternatives:
  - **Represent protocols as `Ty::Instance` with an `is_protocol` flag on
    `HirClassDef`.** Rejected: `is_assignable` and every match arm would
    need to consult `HirClassDef` to distinguish a protocol from a concrete
    class, even in contexts where only `Ty` is available. A dedicated
    `Ty::Protocol` variant makes the distinction type-level and zero-cost
    in match arms that don't handle it.
  - **Vtable dispatch for protocol-typed variables.** Rejected by D-160:
    v0.3 has no runtime polymorphism. See D-006 for the full rationale.
  - **Full PEP 544 generic protocols.** Rejected for v0.3: pycc's generic
    infrastructure (D-134) is limited to a single scalar-only type
    parameter. A protocol with a type parameter is rejected with `C0001`.
- Consequences:
  - **`@runtime_checkable` semantics gap.** CPython's
    `@runtime_checkable` `isinstance` only checks attribute *presence*, not
    signature compatibility. pycc's compile-time check is stricter (it
    checks full structural conformance). This is a deliberate deviation:
    pycc's static type system has full type information at compile time.
  - **No generic protocols in v0.3.** A protocol with a type parameter is
    rejected with `C0001`.
  - **No `issubclass` structural check.** PEP 544 disallows it; only
    explicit protocol inheritance makes `issubclass(Q, P)` valid.
  - **No vtable / dynamic dispatch for protocols.** Post-v0.3 feature
    (D-006's "explicit `dyn`-like use" clause).
  - **`abc.ABCMeta` / `__subclasshook__`** are out of scope. `abc.ABC` and
    `@abstractmethod` are compile-time-only markers.
