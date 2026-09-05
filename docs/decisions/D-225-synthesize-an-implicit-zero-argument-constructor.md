---
id: D-225
title: "Synthesize an implicit zero-argument constructor for a class with no `__init__`"
status: accepted
---

## D-225: Synthesize an implicit zero-argument constructor for a class with no `__init__`
- Status: accepted
- Context: [#912](https://github.com/rotnov/pycc/issues/912) reports that
  `class Config:` carrying only class-level attributes — the shape
  [D-224](./D-224-restrict-class-level-attributes-to-scalar.md)/[#911](https://github.com/rotnov/pycc/issues/911)
  just made expressible — is rejected with `C0001` the moment it is written,
  because `lower_class` required every class to declare an `__init__` or to
  inherit one through its MRO ([#432](https://github.com/rotnov/pycc/issues/432)).
  The same rejection covers `class A: pass`, a methods-only class, a
  property-only class, and a `@staticmethod`-only namespace class. In CPython
  none of these are special: they inherit `object.__init__`, which accepts a
  bare `C()` and does nothing. The rejection is also load-bearing further
  down the pipeline — `pycc_types::class::binding::resolve_instantiation` and
  `pycc_mir`'s `Instantiate` lowering both `panic!` when no `__init__` is
  reachable in the MRO, with panic text naming `lower_class` as the pass that
  is supposed to have made that state unreachable.
- Decision: a class that declares no `__init__` and inherits none from its MRO
  **is instantiable**, through an implicit zero-argument constructor
  synthesized at HIR-lowering time. `crates/pycc_hir/src/class/init.rs`'s
  `ensure_init` runs at exactly the point the old rejection did: if the MRO
  (skipping the class itself, most-derived-first, matching #432's existing
  resolution) provides an `__init__`, nothing is synthesized and the inherited
  constructor is used unchanged; otherwise `synthesize_dataclass_init(&class_name, &[])`
  emits `def <Class>.__init__(self: Instance(<Class>)) -> None:` with an empty
  body, and it is appended to the class's method table and to the module's
  item list exactly as the `@dataclass` path already appends its own generated
  `__init__`. The synthesized constructor establishes no attribute slots, so
  `mro_attr_count` and the D-154 slot layout are untouched; calling it with an
  argument is an ordinary `T0021` arity error. Because it is a real entry in
  the method table, every downstream pass may now rely on a class lowered
  through `lower_class` having a resolvable `__init__` — the two `panic!`
  sites above are restated as internal-error assertions of that guarantee
  rather than as a claim about a rejection that no longer exists.
- Alternatives:
  - *Accept the class as a namespace only — resolvable for `C.MAX`, but still
    `C0001` on `C()`*. This is a smaller HIR change and closes #912's literal
    complaint, but it invents a Python-unlike class kind: CPython has no class
    that exists yet cannot be called. It would need its own diagnostic, its
    own documentation of which classes are and are not callable, and it would
    have to be reversed the first time anyone writes `c = Config()`. Rejected
    as a semantics decision, not a scope decision.
  - *Synthesize the constructor later — in `pycc_types` or `pycc_mir`, at the
    instantiation site*. This puts the synthesis behind the type checker, so
    the arity error for `Config(1)` would have to be invented separately, and
    it splits the "every class has an `__init__`" invariant across two crates
    instead of establishing it where the class table is built.
  - *Write a second, purpose-built synthesizer rather than reusing
    `synthesize_dataclass_init`*. The dataclass synthesizer already degenerates
    to precisely the wanted shape on an empty field list; a parallel function
    would duplicate the `HirItem::Function` construction and the mangling
    convention, and the two would drift.
  - *Keep the rejection and require an explicit `def __init__(self) -> None: pass`*.
    This is what the tree did, and #912 is the report that it is wrong: the
    boilerplate is not something CPython asks for, and the `C0001` fires on
    the most ordinary class body in the language.
- Consequences: `class A: pass` compiles and runs, which removes a `C0001` that
  five unrelated tests had been using merely as a convenient "unsupported yet"
  vehicle; those are re-vehicled onto `async def`, the furthest-scheduled
  unsupported construct, so they no longer sit on a feature that is about to
  ship. Three tests that were *about* the rejection are inverted to assert the
  synthesis. The class model gains a genuine invariant — a non-enum class
  lowered through `lower_class` always has an `__init__` — and one known hole
  in it: an enum class early-returns through `lower_enum_class` before
  `ensure_init` runs and still reaches `pycc_mir` with no constructor, which
  panics; that is [#921](https://github.com/rotnov/pycc/issues/921) and is
  deliberately not fixed here. *Follow-up (2026-09-05): #921 closed the hole
  -- calling an enum class is `C0001` at the call site, reported by
  `pycc_hir::class::enum_call` with a span-less guard in
  `pycc_types::class::resolve_instantiation` behind it, and
  `HirClassDef.is_enum` records the enum provenance; the invariant above now
  holds without a hole.* The #541 Part 2 exception rules are unchanged
  but now reachable in a new shape: a user-declared ancestor with a
  *synthesized* `__init__` is still a non-synthetic ancestor under
  [D-188](./D-188-synthesize-hirclassdefs-for-the-builtin-exception.md)
  provenance, so `class Base: pass` / `class MyError(Base, Exception): pass` /
  `raise MyError("boom")` stays `C0001`. Reversing this decision means
  reinstating a rejection for a construct users will by then have written, so
  treat it as irreversible.
