---
id: D-198
title: "`cast` erasure limits `cast` to representation-preserving targets"
status: accepted
---

## D-198: `cast` erasure limits `cast` to representation-preserving targets

- Status: accepted
- Context: #767 added `typing.cast(T, value)` as a special-cased builtin call.
  `typing.cast` is a runtime no-op in CPython, and pycc implements it by
  *erasure*: `crates/pycc_mir/src/expr.rs` lowers the whole call expression to
  its second argument alone, so no `MirExpr::Call` for `cast` ever reaches
  codegen and the feature costs nothing at runtime. CPython's and mypy's
  `cast` are deliberately *unchecked* — `cast(str, 5)` is legal in both, and a
  static checker is required to believe the assertion. pycc cannot believe it
  unconditionally: because the call is erased with no conversion emitted, a
  target type whose native representation differs from the value's own leaves
  the checker validating the rest of the program against `T` while the emitted
  code still carries the value's real representation. `cast(str, 5)` would
  therefore reach either a `pycc_codegen` local-type-drift `debug_assert_eq!`
  panic in a debug build or silently misinterpreted bits in a release one —
  type confusion introduced by a construct that is supposed to be free. This
  was found by the pinned local reviewer on #767 before merge.
- Decision: `check_cast` (`crates/pycc_types/src/class.rs`) accepts a
  `cast(T, value)` only when erasing it preserves both the value's runtime
  representation and its attribute layout, and otherwise rejects it with
  `C0001`, the versioned capability code. Two cases qualify: the target is
  the value's own type, or the target is one of the value's class's MRO
  ancestors (an up-cast). `bool` -> `int` is rejected along with every other
  cross-representation pair despite `bool` being a static subtype of `int`:
  the representation table gives `bool` a standalone `i8` against `int`'s
  `i64`-compatible word, and no widening is emitted for an erased call.
  A genuine down-cast — the target is a strict *descendant* of the value's
  class — is also rejected, on layout grounds rather than representation
  grounds: see the second-review-pass paragraph below. pycc does not verify
  the nominal relationship for the up-cast/identity subset it does accept —
  the assertion itself stays as unchecked as CPython's and mypy's for that
  subset — but the down-cast direction is a checked rejection, not an
  unchecked pass-through.
- Second review pass (post-merge-gate, pre-merge): the pinned local reviewer
  flagged that the first version of this decision accepted every
  `Instance` -> `Instance` pair, including genuine down-casts, on the
  reasoning that every class instance is one heap-object pointer regardless
  of class. That reasoning covers *representation* but not *layout*: erasure
  means `pycc_mir` never learns the checker-verified target type, so it
  keeps resolving attribute/method access against the value's *real* MIR
  type. A down-cast the checker accepted as the (larger) target class would
  then have the checker validate access against attributes the erased MIR
  expression's (smaller) real class does not have — reaching either a
  `pycc_mir` panic (an unannotated binding or inline access never finds the
  attribute in the real class's MRO) or an out-of-bounds `pycc_rt`
  instance-slot abort at runtime (an `AnnAssign` re-anchors the MIR type to
  the target class without the object ever being allocated with that
  class's extra slots). `cast_shares_representation` was narrowed to require
  MRO ancestry-or-identity for the `Instance` -> `Instance` case, closing
  the hole at `check_cast` before either failure path is reachable.
  Down-casting — `cast`'s single most common legitimate use in ordinary
  Python, typically paired with an `isinstance` narrowing check — is
  therefore deferred rather than supported by this issue: a future slice
  needs either a runtime class tag or a layout-compatibility check that
  erasure alone cannot provide.
- Alternatives:
  - *Accept every `cast` unconditionally, as CPython and mypy do.* Rejected:
    it is precisely the type-confusion bug above. Full CPython fidelity here
    requires either a real representation conversion or a checker that never
    trusts the cast, neither of which the erasure implementation provides.
  - *Reuse `is_assignable_env` as the gate.* Rejected after review: that is a
    *subtyping* test, not a representation test. It admits `bool` -> `int`
    (a real representation change) while restricting `Instance` -> `Instance`
    to protocol conformance rather than MRO ancestry.
  - *Accept every `Instance` -> `Instance` pair unconditionally, including
    down-casts.* The first version of this decision took this approach.
    Rejected after a second review pass found it unsound: see the
    second-review-pass paragraph above. A representation-only test is not
    enough for classes, because erasure also drops the checker-verified
    target type that would otherwise justify treating the down-cast result's
    wider attribute set as safe to access.
  - *Emit a real conversion instead of erasing the call.* Rejected for this
    slice: it would make `typing.cast` cost runtime work that CPython's
    does not, and it has no meaning at all for the class-to-class case that
    motivates `cast` in the first place.
  - *Report the rejection as `T0021` (type mismatch).* Rejected: the program
    is not ill-typed Python, and the wording would have to say "in this pycc
    version" under a code that does not mean that. `C0001` is the versioned
    capability code and already covers "an implemented special-cased builtin
    called with an argument shape this version does not support", which is
    what this is; `cast(list[int], x)` from the same issue is already filed
    under it.
- Consequences: `cast` is a *representation-preserving* assertion in pycc,
  not the fully unchecked one PEP 484 describes — a documented divergence,
  recorded in `docs/DIAGNOSTICS.md`'s `C0001` prose and
  `docs/STDLIB_PLAN.md`'s `typing` row. A program that uses `cast` to
  reinterpret a scalar (`cast(int, some_bool)`) or to narrow to a subclass
  (`cast(Derived, base)`) is rejected rather than miscompiled or crashed at
  runtime, and rejecting under a versioned code keeps the door open: a later
  slice that emits a real conversion, that tracks representation separately
  from the static type, or that threads a runtime class tag through to
  validate layout compatibility, can accept those calls without
  contradicting an accepted by-design rejection. The check runs only on the
  validation-pass route; `constraints.rs`'s solver mirror, reached only for a
  return-type-inferred private helper, has no resolved `Ty` for the value at
  that point and does not perform it — the same asymmetry between the two
  passes that `check_isinstance` already documents.
