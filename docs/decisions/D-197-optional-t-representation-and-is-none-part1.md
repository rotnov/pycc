---
id: D-197
title: "Optional[T] representation, T | None parsing, and is/is not on None (Part 1 of #747)"
status: accepted
---

## D-197: Optional[T] representation, T | None parsing, and is/is not on None (Part 1 of #747)
- Status: accepted
- Context: issue #747 tracks general PEP 604 union-type support. Issue #763 scopes
  "Part 1": the minimum slice that gives `Optional[T]` (`T | None`) a real type,
  a real runtime representation, and a real presence test, without taking on the
  much larger general-union type-checking problem (`A | B` for arbitrary `A`/`B`)
  or CPython's full flow-sensitive narrowing model in the same change. This entry
  records the four design choices that slice required, plus the one scope cut
  discovered empirically while implementing it (narrowing).

- Decision:
  1. **New `Ty::Optional(Box<Ty>)` variant**, not a general `Ty::Union(Vec<Ty>)`.
     A `Vec`-based union would need to represent and type-check an unbounded
     number of shapes (`A | B | C`, nested unions, a union containing another
     union) for a feature this PR intentionally does not implement yet. A
     dedicated `Optional` variant models exactly PEP 604's most common case
     (a nullable value) and gives every downstream match (`is_assignable`,
     codegen's `ty_to_basic_type`, MIR lowering) a closed, two-armed shape to
     handle instead of an open-ended one. General unions, if ever added, are a
     separate variant and a separate PR; they are not this variant generalized.
  2. **Runtime representation is an explicit `{ inner: i64, present: i8 }`
     struct**, passed and stored by value — not a niche-packed encoding inside
     `inner` itself (e.g. reusing the D-141 int-representation's unused bit
     patterns as an in-band `None` sentinel). D-109's 16-byte scalar-passing
     ceiling comfortably fits this struct (9 bytes, padded to 16). A niche
     encoding would need to reserve a specific `i64` bit pattern as `None` for
     every future `Optional[T]` inner type `T` gets extended to, which is a
     much larger design commitment than this PR's `Optional[int]`-only scope
     needs to make. The explicit tag is simple, correct for every future inner
     type without re-deriving a niche, and costs one extra byte per value.
  3. **Codegen scope is `Optional[int]` only.** `annotation_to_ty` recognizes
     the `T | None` shape for any `T` this compiler can already express as an
     annotation (so `list[int] | None`, `SomeClass | None`, and a generic
     type parameter `T | None` all parse), but rejects every inner type other
     than `Ty::Int` with `T0049` before a `Ty::Optional` is ever constructed
     from source. This mirrors the identical `list[int]`-only (`T0034`,
     D-105) and `dict[str, int]`-only (`T0036`, D-122) scope cuts already
     established for v0.2: prove the representation and the full pipeline
     (HIR → types → MIR → codegen) end-to-end for one concrete inner type
     first, extend breadth later.
  4. **`is`/`is not` is scoped to one operand being syntactically `None`.**
     HIR lowering (`crates/pycc_hir/src/expr.rs`'s `Expr::Compare` arm) only
     produces `CmpOpKind::Is`/`IsNot` when one of the two operands is
     literally `Expr::NoneLiteral(_)`; every other `is`/`is not` shape keeps
     falling through to `C0001` exactly as before this PR, unaffected. Type
     checking (`pycc_types`'s `infer_expr_in`) then requires the *other*
     operand's static type to be `Ty::Optional(_)` or `Ty::None`, reusing the
     existing generic `T0021` code for the mismatch case (no new diagnostic
     needed — general object-identity comparison (`a is b` for two arbitrary
     objects) remains entirely unimplemented and un-scoped by this PR).

- Alternatives:
  - *General `Ty::Union(Vec<Ty>)` from the start* — rejected per decision 1:
    the extra generality buys nothing this PR's fixture or scope needs, and
    defers real, working `Optional[int]` behind a much larger type-checking
    surface (subtyping/assignability rules between arbitrary union members).
  - *Niche-packed `Optional` representation* (no explicit tag byte) —
    rejected per decision 2: attractive for `Optional[int]` specifically
    (smallints are always odd `i64` words per D-141; an even, non-boxed
    sentinel is available), but the aspirational "niche optimization" the
    representation would then encode is a one-`T`-at-a-time commitment
    inconsistent with keeping `Ty::Optional` open to future inner types.
    Deferred to a future PR that actually extends codegen breadth, not
    designed against speculatively here. Recorded in `docs/TYPE_SYSTEM.md`'s
    "unions `A \| B`" row as the eventual target representation.
  - *General `is`/`is not` object-identity comparison* — rejected per
    decision 4: CPython's `is` compares object identity for values of any
    type, but pycc has no general notion of object identity for scalar/value
    types (an `int` is not boxed in the general case, per D-141), so a
    general implementation is its own, much larger design problem. Scoping
    to the `None`-operand shape gives PEP 604's actual use case (presence
    testing) without prejudging that larger problem.
  - **Minimal `is None`/`is not None` flow-sensitive narrowing inside `if`/
    `else` branches** — the issue #763 plan initially scoped this as
    required, in-PR work (work item 5b), reasoning that the conformance
    fixture "cannot exist" without reading an `Optional[int]`'s unwrapped
    payload. This was checked empirically before implementing narrowing: a
    five-rung manual ladder (module-global and function-local `int | None`
    values, both operand orders, a function returning `int | None` with both
    branches, all read back only through `x is None`/`x is not None`
    presence booleans, never an unwrapped payload) built and ran correctly
    with `pycc build` in both `--debug` and `--release`, and matched the
    local `python3.14` (3.14.6) oracle byte-for-byte on every rung. The
    fixture at `tests/fixtures/pep_0604_union.py` is built the same way. The
    premise was therefore false: a `◐` (partial) PEP 604 conformance row is
    fully supportable by a presence-only fixture, and narrowing is not
    load-bearing for this PR. It is deferred to a follow-up issue (Part 2 of
    #747) rather than implemented here, per AGENTS.md's decomposition
    guidance: flow-sensitive narrowing is its own architectural seam
    (a new kind of context threaded through `if`/`else` branch checking),
    independently testable, and not required to ship this PR's own
    completion criteria. `crates/pycc_types/src/lib.rs`'s `is_assignable` doc
    comment is written to state this plainly (no narrowing exists anywhere
    in this crate yet) rather than pointing at narrowing logic that was
    never actually added.

- Consequences:
  - `Optional[int]` (either `int | None` or `None | int` spelling) is a real,
    working, conformance-proven type today: declared, assigned, returned from
    a function, and tested for presence with `is`/`is not None`, matching
    CPython byte-for-byte in both build profiles.
  - Every other union shape (`A | B` with neither side `None`, any 3+-operand
    chain, `Optional[T]` for `T != int`) is a clean, versioned `T0048`/`T0049`
    capability gap, not silently misparsed or miscompiled.
  - An `Optional[int]`-typed value cannot be narrowed back to plain `int` by
    this PR — using it where `int` is required is `T0021` unconditionally,
    even directly inside an `if x is not None:` branch. This is a real,
    user-visible limitation until the Part 2 follow-up lands; it does not
    block PEP 604's `docs/PYTHON_STANDARDS.md` row from flipping to `◐`
    (partial), since the row's own `not_proven` gaps list narrowing
    explicitly.
  - The `{ inner, present }` explicit-tag representation is one byte larger
    per value than an eventual niche-packed encoding would be; revisiting
    this trade only becomes relevant once codegen breadth extends past
    `Optional[int]`.
