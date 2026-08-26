---
id: D-204
title: "Widen Optional[T] codegen to float and bool inner types (Part 3 of #747)"
status: accepted
---

## D-204: Widen Optional[T] codegen to float and bool inner types (Part 3 of #747)
- Status: accepted
- Context: D-197 (#763, Part 1 of #747) scoped `Optional[T]`/`T | None` codegen to
  `T == int` only, with `T0049` rejecting every other inner type as a versioned
  capability gap. D-201 (#769, Part 2 of #747) added flow-sensitive narrowing on
  top of that same `Optional[int]`-only scope. Issue #809 ("Part 3 of #747")
  widens the accepted inner types to the remaining non-refcounted scalar types
  this compiler already represents as a plain LLVM value rather than a heap
  pointer: `float` and `bool`. `Optional[str]`, `Optional[list[...]]`, and every
  other refcounted/pointer inner type, plus general (non-`Optional`) unions
  `A | B`, stay explicitly out of scope — each is either a materially different
  representation problem (a nullable pointer already has a natural niche, unlike
  a nullable scalar) or the much larger general-union type-checking problem
  D-197 already deferred.

- Decision:
  1. **`T0049`'s gate widens from `Ty::Int` to `matches!(inner, Ty::Int | Ty::Float
     | Ty::Bool)`** (`crates/pycc_hir/src/func.rs`). No new diagnostic code is
     minted; `T0049`'s existing meaning ("a recognized `T | None` shape whose
     inner type this compiler does not yet support") is unchanged, only its
     accepted set grows.
  2. **The runtime representation stays the explicit `{ payload, present: i8 }`
     struct D-197 chose**, with `payload` typed per inner type rather than
     forced to `i64`: `i64` (D-141 encoded) for `Ty::Int`, plain `f64` for
     `Ty::Float`, plain `i8` for `Ty::Bool`. This is `ty_to_basic_type`'s
     existing `Ty::Optional` arm, which already recursed into the inner type's
     own representation and needed no change — only the *other* three
     producers of an `Optional` payload value needed to stop assuming
     `Ty::Int`:
     - `default_value_for_type`'s `Ty::Optional` arm and `declare_module_globals`'s
       own copy of the same placeholder-construction logic both used to build
       every inner type's placeholder payload via `tag_smallint_const` (a
       `Ty::Int`-specific D-141-encoded zero), unconditionally. For `Ty::Float`/
       `Ty::Bool` this hands an `i64` constant to a struct field typed `f64`/
       `i8` — an LLVM constant type mismatch that only surfaces as a low-level
       backend fatal error (`"invalid number of bytes"`), not a Rust-level
       panic or a verifier rejection at the point of construction. Both call
       sites now special-case `Ty::Int` (keep `tag_smallint_const`) and recurse
       into `default_value_for_type` itself for every other inner type. This
       bug was caught empirically during this issue's own implementation: a
       minimal `x: bool | None = True` module-scope program crashed the LLVM
       backend before this fix, isolated by bisection to the plain-assignment
       path (not narrowing/unwrap) and traced to `declare_module_globals`'s
       initializer specifically.
     - `coerce_scalar_to_type`'s bare-payload-to-`Optional` widening arm used
       to key its payload conversion on the *coerced* `Scalar` variant alone.
       Naively adding `Scalar::Float`/`Scalar::Bool` arms next to the existing
       `Scalar::Int` one would have silently accepted a mismatched payload
       (e.g. a `Scalar::Float` widening into a declared `Optional[int]` slot,
       since the recursive `coerce_scalar_to_type` call with target `Ty::Int`
       has no `float -> int` conversion arm and falls through unchanged). The
       match now keys on `(inner.as_ref(), coerced)` as a pair, so only the
       three matching combinations succeed and everything else — including
       that mismatch — reaches the existing defensive panic.
     - `MirExpr::OptionalUnwrap`'s codegen arm used to always return
       `Scalar::Int(payload.into_int_value())` regardless of declared inner
       type — dead-but-latent code under D-197's `int`-only scope, now live
       and broken by this widening (`.into_int_value()` on an `f64`
       `BasicValueEnum` panics for `Optional[float]`; for `Optional[bool]` it
       would silently mislabel a plain `i8` as a D-141-encoded word). Now
       dispatches on the inner `Ty` the same way the payload-typed struct
       already does.
  3. **`truthy`'s `Scalar::Optional` arm dispatches on the extracted payload's
     LLVM type**, not a passed-in `Ty` (the function receives no `Ty` for a
     `Scalar`'s inner type). `ty_to_basic_type`'s `Ty::Optional` arm makes this
     unambiguous per inner type — `f64` for `Ty::Float`, `i64` for `Ty::Int`,
     `i8` for `Ty::Bool` — and every producer of a `Scalar::Optional` reaching
     this function already builds the struct with the target's own
     inner-typed payload, so the dispatch is exact rather than a heuristic.
     `Ty::Float`'s branch mirrors `Scalar::Float`'s own unordered-not-equal
     zero compare (CPython's `bool(0.0)`/`bool(-0.0)` is `False`, `NaN` is
     truthy); `Ty::Bool`'s plain `i8` `0`/`1` already *is* its own truthiness,
     no conversion needed.
  4. **The `Optional[bool]`/bare-`None`-placeholder LLVM struct-type collision
     is real but harmless, and needed no code change.** `Optional[bool]`'s
     real representation is the anonymous LLVM struct `{ i8, i8 }`; the
     bare-`None` placeholder `coerce_scalar_to_type` builds before any target
     type is known is also `{ i8, i8 }`. LLVM literal (non-identified) struct
     types are uniqued per-`Context` by field-type list, so these are not
     merely equal in shape — they are the same `StructType` value
     (`crates/pycc_codegen/src/tests.rs`'s
     `optional_bool_none_placeholder_and_real_absent_value_are_the_same_llvm_struct_type`
     pins this directly). This causes no bug because nothing in this codebase
     ever discriminates a `Scalar::Optional`'s inner type by introspecting its
     `StructValue`'s LLVM type — every `coerce_scalar_to_type` call site (and
     every other consumer) always carries or re-derives the target `Ty`
     independently.
     `optional_bool_absent_value_truthiness_and_narrowed_unwrap_are_both_correct`
     proves the empirical end-to-end consequence: an absent `Optional[bool]`
     value is still falsy, and `is not None` narrowing still correctly does
     not enter the unwrap branch.
  5. **`bigint_rc.rs`'s `OptionalUnwrap`-related refcount guards needed no new
     scoping.** `int_value_is_a_duplicate_reference`'s `MirExpr::
     OptionalUnwrap(_, _) => true` arm is reached only for the `T = int` case
     because its own caller, `int_temporary_word`, already gates on
     `source_expr.ty() == pycc_mir::Ty::Int` before this function runs at all
     — a stale doc comment there was corrected to explain this rather than
     assert the arm's own `.ty()` is unconditionally `Ty::Int`.
     `retain_if_int_duplicate`'s `OptionalUnwrap` arm was already correctly
     scoped via its enclosing `if let Scalar::Int(word) = scalar` guard,
     unchanged. Two new tests
     (`a_narrowed_optional_float_unwrap_emits_no_bigint_refcount_calls`,
     `a_narrowed_optional_bool_unwrap_emits_no_bigint_refcount_calls`) pin
     that a narrowed `Optional[float]`/`Optional[bool]` unwrap emits zero
     bigint retain/release calls, empirically confirming the existing
     `Ty::Int`-only scoping needed no widening.
  6. **`storage_slot_at_entry`'s per-local entry-block zero-initialization
     stays `Ty::Int`-only** (`Str`, `Int`, and `Optional(Int)` are the only
     types it stores a placeholder into before the first real assignment);
     `Optional[float]`/`Optional[bool]` locals are investigated and left
     uninitialized deliberately, not as an oversight. The `Ty::Int` case needs
     a *valid* D-141-encoded placeholder specifically because
     `release_int_slot_before_store` unconditionally releases the slot's
     previous payload before every store, including the first one, and a raw
     zero word is not guaranteed to decode validly under D-141's tagged
     encoding. `Float`/`Bool` payloads are never refcounted, so no
     release-before-store ever reads this slot, and neither `f64` nor `i8`
     has a `classify_encoded_int`-style fail-closed panic on an arbitrary bit
     pattern — an uninitialized read is unreachable in practice regardless
     (the separate `initialized` guard flag already traps a genuine
     read-before-write) and defined-if-reached regardless (LLVM's own
     `poison`/`undef` semantics for a scalar load, not a decode failure).

- Alternatives:
  - Widen to every scalar and container type in one pass instead of stopping at
    `{int, float, bool}`: rejected because `Optional[str]` (and any other
    refcounted/pointer inner type) is a materially different representation
    problem — a nullable pointer already has a natural `null` niche, which
    would call for reconsidering whether the explicit `{ payload, present }`
    tag is still the right representation for that case, rather than
    mechanically reusing it. Keeping this PR to the three inner types that
    slot into the *existing* explicit-tag representation with no design change
    keeps the diff a mechanical widening instead of a second representation
    decision.
  - Fix the `declare_module_globals`/`default_value_for_type` bug by special-
    casing only `Ty::Bool` (the one inner type that actually crashed) rather
    than generalizing to "recurse for every non-`Int` inner type": rejected as
    fragile — `Ty::Float` was equally wrong before this fix (an `i64` constant
    into an `f64` field is exactly the same class of mismatch), it simply
    happened not to crash the LLVM backend the same way in the cases tested.
    The general fix removes the whole bug class rather than patching the one
    inner type that happened to reproduce loudest.

- Consequences:
  - `Optional[float]`/`Optional[bool]` are now real, tested, conformance-proven
    features: declared, assigned (from a bare payload, from `None`, and via
    reassignment), returned, module-global and function-local, tested with
    `is`/`is not None`, narrowed, and read for truthiness — mirroring
    `Optional[int]`'s existing coverage. `tests/fixtures/pep_0604_union.py`
    proves this byte-for-byte against CPython 3.14 for both `--debug` and
    `--release` codegen profiles.
  - `T0049` narrows further: the next inner type this project widens to (most
    plausibly `str`) needs its own design decision for the representation
    question this entry's "Alternatives" section flags, not just a mechanical
    repeat of this entry's pattern.
  - The `declare_module_globals`/`default_value_for_type` fix is a general bug
    fix, not scoped only to the two new inner types — any future `Ty::Optional`
    inner type this compiler ever adds automatically gets a correctly-typed
    placeholder payload from `default_value_for_type`'s own per-type default,
    with no `declare_module_globals`-side change required.
