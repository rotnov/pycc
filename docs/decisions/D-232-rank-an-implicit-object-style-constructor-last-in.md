---
id: D-232
title: "Rank an implicit object-style constructor last in constructor resolution"
status: accepted
---

## D-232: Rank an implicit object-style constructor last in constructor resolution

- Status: accepted
- Context:
  [D-225](./D-225-synthesize-an-implicit-zero-argument-constructor.md)
  synthesizes an implicit zero-argument `__init__` for a class that declares
  none and inherits none through its MRO, and writes it into that class's
  **own** method table. Every constructor walk in the compiler then resolved
  "the first `__init__` along the MRO", so for `class C(A, B)` with `A`
  init-less and `B` declaring a real `__init__`, `A`'s implicit stub won:
  `B.__init__` never ran, `B`'s attribute slots stayed uninitialized, and
  reading one aborted the process in `pycc_rt`
  (`invalid encoded int word 0x0`, the documented exit-`101` boundary). The
  checker mis-resolved the same way, rejecting `C(5)` with `T0021` for a
  program CPython runs. CPython does not have this problem because the
  equivalent constructor is `object.__init__`, which sits at the **end** of
  every MRO rather than at the position of whichever base happened to omit
  one. The defect is a *ranking* defect, not a walk defect: the walks were
  already correct, most-derived-first. Issue
  [#966](https://github.com/rotnov/pycc/issues/966); the reviewed plan is
  that issue's comment 5558167328.
- Decision: record the provenance of D-225's synthesis on
  `HirClassDef::implicit_object_init`, set only at `ensure_init`'s call site
  in `lower_class` and only from `ensure_init`'s own report, and rank a
  flagged class **last** wherever the compiler resolves a *constructor*.

  1. The flag is provenance, never shape. An explicitly written
     `def __init__(self) -> None: pass` lowers to the identical empty body
     and is a real constructor that must keep ranking first — CPython raises
     `AttributeError` for `class C(A, B)` where `A` declares that
     constructor, and pycc must not silently call `B`'s instead. No consumer
     may infer the flag from a constructor's parameter list or body. This
     mirrors [D-188](./D-188-synthesize-hirclassdefs-for-the-builtin-exception.md)'s
     rule that `is_synthetic_class` is provenance-based.
  2. `init::synthesize_dataclass_init` is shared with the `@dataclass` path,
     whose generated `__init__` is a *real* constructor. The flag is
     therefore never set inside that helper — only at the `ensure_init` call
     site — so a dataclass base keeps ranking first.
  3. Five sites rank constructors and all five take the skip:
     `pycc_mir`'s `Instantiate` lowering and the `super()` arm of
     `MethodCall`; `pycc_types`' `class::binding::resolve_instantiation` and
     `class::resolve_super_method_call`; and
     `pycc_types::exception::reject_own_constructor`. The `pycc_mir` and
     `pycc_types` pairs must move together, or the checker and the lowering
     resolve different constructors.
  4. The first four sites fall back to a flagged constructor when the whole
     MRO is implicit — `class A: pass` / `class B: pass` / `class C(A, B)`
     must still construct, and `class A: pass` / `class C(A)` calling
     `super().__init__()` must still resolve. Each keeps its pre-existing
     "no `__init__` in this MRO at all" arm unchanged behind that fallback.
     `reject_own_constructor` deliberately has **no** fallback arm: it runs
     only on tagged classes, and `pycc_hir::exception` gives the root
     `Exception` — and only it — a real, never-flagged `__init__`, so the
     walk always terminates on an unflagged candidate and a fallback arm
     would be unreachable code under D-014.
  5. The two `super()` skips are gated on `method == "__init__"`. Those
     walkers are otherwise name-agnostic and stay that way: a flagged class
     is still an ordinary owner of any other method.

  The rule is scoped to **constructor resolution**, deliberately and
  narrowly — see the Consequences note on the generic method-call surface.
- Alternatives:
  - *Alias the derived class's `__init__` entry to the winning mangled
    name.* Refuted empirically: both `pycc_mir` and
    `pycc_types::class::binding` reconstruct `format!("{mro_class}.__init__")`
    from the MRO entry rather than reading the mapped value, so an alias
    would be ignored by exactly the sites that decide the outcome.
  - *Detect the implicit constructor by shape* (empty body, one parameter).
    Rejected: indistinguishable from a user-written
    `def __init__(self) -> None: pass`, whose CPython semantics are the
    opposite. `tests/issue_966_inherited_init_rank.rs` pins that program's
    runtime abort precisely because the abort is the discriminator proving
    no shape-based detection crept in.
  - *Keep the D-189 rule-4 rejection with a special case.* Rejected: it
    would preserve a rejection CPython does not make, at the cost of an
    exception to the single principle this entry states.
  - *Name the field `synthesized_init`.* Rejected: it collides in meaning
    with D-188's `is_synthetic_class` provenance and reads as covering the
    dataclass-synthesized constructor, which it must not.
- Consequences: `class C(A, B)` with an init-less first base now constructs
  and runs as CPython does, at both the runtime and the checker level. Two
  accepted behaviour flips follow, both moving toward CPython and D-189
  consistency:

  This entry **supersedes
  [D-189](./D-189-assign-user-exception-classes-a-compile-time.md) rule 4**,
  which keyed raisability on "the first `__init__` along its MRO is the
  synthetic `Exception.__init__`"; that ranking now skips an implicit
  constructor. Consequently `class Base: pass` /
  `class MyError(Base, Exception): pass` / `raise MyError("boom")` is now
  **raisable**, where it was `C0001`. It also **supersedes D-225's own
  consequence text**, which named that exact program as one that "stays
  `C0001`" and called reversing it irreversible. That claim is withdrawn:
  the construct it protected was a rejection of a program CPython accepts,
  and the reversal makes pycc more permissive rather than less, so no
  program that compiled before stops compiling because of it. D-189 and
  D-225 are not edited; accepted decisions are superseded by reference.

  Second, binding that same class as a *value* (`m = MyError()`) is now
  rejected `C0001` *cannot instantiate exception class ... as a value*,
  where it previously compiled. With `Exception.__init__` now the resolved
  constructor, the D-188 synthetic-owner guard in `resolve_instantiation`
  fires as it already did for every other raisable class; the one shape that
  slipped through no longer does. Both flips are asserted with runtime
  evidence in `tests/issue_912_no_init_class.rs`.

  The **generic `__init__` method-call surface is deliberately left
  unranked**: `c.__init__()` / `c.__init__(5)` on an already-constructed
  instance still resolves through `class::method_call::resolve_method_call`
  and `pycc_mir`'s ordinary `MethodCall` walk, which are name-agnostic by
  design. So `c.__init__(5)` is still `T0021` on a class whose `C(5)` now
  works, and `c.__init__()` silently no-ops against the implicit stub. That
  is why this entry's rule says *constructor resolution* rather than
  "everywhere": stating a broader principle than the code implements would
  make the decision log wrong rather than merely incomplete. The limitation
  is recorded in `docs/TYPE_SYSTEM.md`, and ranking that surface is a
  separate design question about special-casing a method name inside a
  general walk.

  A pre-existing MRO **slot-aliasing** defect in `pycc_mir`'s `mro_attrs` is
  unchanged and now more reachable: a class with two or more slot-bearing
  bases gets a flat layout assigned most-base-first, while each base's own
  `__init__` addresses slots against that base's own layout. One read aborts
  loudly, another returns a silently wrong value. It is independent of this
  entry, pinned in `tests/issue_966_inherited_init_rank.rs`, documented in
  `docs/TYPE_SYSTEM.md`, and tracked separately.

  `HirClassDef` gains a field, so all ~160 literal construction sites carry
  it; the struct has no `Default`, so a missed site is a compile error
  rather than a silent wrong value.
