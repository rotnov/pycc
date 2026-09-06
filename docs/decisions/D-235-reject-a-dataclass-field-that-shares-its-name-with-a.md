---
id: D-235
title: "Reject a dataclass field that shares its name with a `ClassVar` anywhere in the MRO"
status: accepted
---

## D-235: Reject a dataclass field that shares its name with a `ClassVar` anywhere in the MRO

- Status: accepted
- Context:
  [#911](https://github.com/rotnov/pycc/issues/911) registered `typing.ClassVar`
  and made an annotated class-body attribute a compile-time constant, but
  rejected the spelling outright inside a `@dataclass` body: merely stripping
  the wrapper there would have turned `LIMIT: ClassVar[int] = 10` into a
  *required* `__init__` parameter, a silent divergence from PEP 557.
  [#913](https://github.com/rotnov/pycc/issues/913) lifts that rejection by
  *routing* the declaration to the class-attribute path instead of stripping
  it, so the name never becomes a dataclass field at all.

  Routing rather than stripping is enough for the accepted surface — because
  `lower_class` fills the D-154 instance-slot layout from the same merged field
  list it synthesizes `__init__`/`__eq__`/`__repr__` from, a name that is not a
  field is excluded from all four at once. It is not enough for the *shapes it
  makes newly reachable*. Three of them let a dataclass field and a `ClassVar`
  carry the same name, and CPython's answer in each is a dataclass field
  **default** or an order-dependent field **removal**:

  * In one body, the two declaration orders disagree with each other.
    `x: int` then `x: ClassVar[int] = 1` drops the field (`A()` takes no
    argument, `A.x == 1`); the reverse order yields
    `__init__(self, x: int = 1)`, a field whose default is the class
    attribute's value.
  * Across an inheritance edge — `@dataclass class A: LIMIT: ClassVar[int] = 8`
    and `@dataclass class B(A): LIMIT: int` — CPython makes the base's value
    the field's default: `inspect.signature(B.__init__)` is
    `(self, LIMIT: int = 8) -> None`.
  * Across two sibling bases, the answer depends on **base order alone**.
    CPython processes fields in reverse-MRO order, so for
    `@dataclass class D(A, B)` where `A` contributes only
    `LIMIT: ClassVar[int] = 8` and `B` contributes `LIMIT: int` and
    `other: int`, `A`'s `ClassVar` removes `B`'s field entirely
    (`(self, other: int) -> None`), while `class D(B, A)` keeps it
    (`(self, LIMIT: int, other: int) -> None`).

  pycc rejects dataclass field defaults wholesale, and its merge loop walks
  only each base's `dataclass_fields`, never any base's `class_attrs`. Left
  unchecked, each shape produces a synthesized `__init__` whose arity differs
  from CPython's with no diagnostic — the D-198 class of defect.
  [#969](https://github.com/rotnov/pycc/issues/969)'s multiple-inheritance
  slot-layout gate ([D-234](./D-234-reject-multiple-inheritance-whose-base-layouts-are.md))
  does not catch the sibling-base shape, because a class contributing only a
  `ClassVar` declares no instance attributes at all. A fourth shape, a
  `ClassVar` named after a method the dataclass synthesizes, diverges the same
  way: CPython's `dataclasses` uses `_set_new_attribute`, which leaves an
  existing class `__dict__` entry alone, so the class attribute wins and the
  method is never synthesized.

- Decision: follow the precedent
  [D-224](./D-224-restrict-class-level-attributes-to-scalar.md) sets and
  [D-234](./D-234-reject-multiple-inheritance-whose-base-layouts-are.md)
  reaffirms — reject at HIR-lowering time with `C0001` rather than mis-compile
  — and change no lowering. Two checks are added in `pycc_hir`:

  1. In `lower_class` (`crates/pycc_hir/src/class.rs`), after the dataclass
     field merge is complete and **before** the merged list becomes the slot
     layout and feeds the three synthesis calls, every merged field name is
     looked up as a class attribute through the MRO (`class_attr_owner`, the
     class's own `class_attrs` first, then each base's, most-derived-first).
     A hit is `C0001` naming the field, the declaring class, and the reason.
     One check covers all three collision shapes, because the merged list is
     the only place they meet.
  2. In the class-body walk (`crates/pycc_hir/src/class/body.rs`), a
     `ClassVar` named `__init__`, `__eq__`, or `__repr__` inside a
     `@dataclass` body is `C0001` — the same three names an explicit `def` of
     that name is already rejected for. It cannot be left to the existing
     class-attribute collision check, which runs before synthesis pushes those
     names into the method table.

  The existing collision check's own rejection of a derived `ClassVar` that
  shadows a field inherited from a base dataclass is kept unchanged, and is
  likewise reachable only since #913.

- Alternatives:
  * **Model CPython's rules instead of rejecting.** Each of the three shapes
    resolves to a dataclass field default, which this compiler has no
    optional-parameter mechanism for; implementing one special case of
    defaults ahead of the general feature would fix the arity while leaving
    the sibling-base order dependence unrepresentable anyway.
  * **Reject only the two orders that actually mis-compile**, accepting
    `class D(B, A)`. The two base orders are indistinguishable to a reader,
    differ only in a token's position, and give opposite answers; accepting
    one while rejecting its mirror image is a worse contract than rejecting
    both, and this project has repeatedly preferred the coarse conservative
    gate (D-224's non-name-base read gate, D-234's layout gate).
  * **Put the checks in the class-body walk instead.** The walk sees only the
    current class's own body, so it cannot observe the sibling-base shape at
    all — the field and the class attribute both arrive from bases.
  * **Move the existing `reject_class_attr_collisions` call past the field
    merge** so it sees the populated `attrs`. That changes which diagnostic a
    program with two independent defects reports, for no gain the new check
    does not already provide.

- Consequences:
  * `ClassVar` inside a `@dataclass` body is supported: the name is a
    compile-time constant, readable through the class and through an instance,
    and absent from the synthesized `__init__`, `__eq__` and `__repr__` and
    from the D-154 slot layout. `tests/fixtures/pep_0557_dataclasses.py` proves
    it byte-for-byte against the pinned CPython oracle, and the PEP 557 row of
    `tests/fixtures/conformance-breadth-manifest.json` records `ClassVar` as
    proven.
  * **One deliberate narrowing: a program CPython runs and pycc could compile
    correctly is now rejected.** `@dataclass class D(B, A)`, where `B`
    contributes a `LIMIT: int` field and `A` a `LIMIT: ClassVar[int]`, is
    `C0001` even though CPython keeps the field and pycc's synthesis would
    have matched. The mirror-image base order is a mis-compile, and the two are
    distinguished only by base order. Pinned by
    `tests/issue_913_dataclass_classvar.rs::a_dataclass_field_colliding_with_a_sibling_bases_class_var_is_rejected`,
    which asserts *both* orders are rejected.
  * A value-less `ClassVar` in a dataclass body (`LIMIT: ClassVar[int]`) stays
    `C0001` though CPython accepts it — a consequence of #911's fold model, not
    of this entry, and pinned here so the behaviour is deliberate.
  * The diagnostic precedence is now four deep and is pinned by
    `a_dataclass_field_type_conflict_outranks_the_class_var_collision` (in
    both the integration suite and `pycc_hir`'s own tests): class-body-walk
    errors, then `reject_class_attr_collisions`, then the merge loop's
    `T0052`, then this entry's field/`ClassVar` check, then D-234's slot-layout
    gate. A later reorder of any pair fails a test rather than passing quietly.
  * Relaxing any of these rejections means implementing dataclass field
    defaults first; the sibling-base order dependence additionally needs a
    representation for "the field this base declares is removed by that base",
    which the flat merged-field list has no room for.
