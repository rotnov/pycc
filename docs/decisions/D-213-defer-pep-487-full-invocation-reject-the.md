---
id: D-213
title: "Defer PEP 487 full invocation; reject the inherited-__init_subclass__ soundness gap"
status: accepted
---

## D-213: Defer PEP 487 full invocation; reject the inherited-`__init_subclass__` soundness gap

- Status: accepted
- Context:
  [#585](https://github.com/rotnov/pycc/issues/585) (filed from the PEP 487
  investigation in #580, Part 4 of #572) found that pycc's class-lowering
  model (`crates/pycc_hir/src/class.rs`) recognizes `__init_subclass__` and
  `__set_name__` as valid method names but never actually invokes either
  hook. CPython calls `__init_subclass__` automatically at every subclass's
  creation; pycc's compile-time class-creation model has no mechanism to run
  a side-effecting statement at that point, so full invocation semantics
  would require either a runtime call-out at class-creation time (not
  something this compiler's current codegen model supports) or a
  compile-time interpreter for the hook's body (far beyond this issue's
  scope). `tests/conformance.rs` compares pycc's stdout with the pinned
  CPython 3.14.7 oracle byte-for-byte, and per #580's own rule an inert
  (no-observable-side-effect) fixture is disallowed, since it would claim
  PEP 487 conformance while demonstrating nothing about it. No fixture that
  exercises real invocation semantics can be written today.

  Independent of that full-invocation question, #585 also identified a
  narrower soundness gap in the code that already exists. The
  static-evaluability guard added by #435 (Part B) only fires when the
  **derived** class also defines `__init_subclass__`:

  ```rust
  if methods.iter().any(|(name, _)| name == "__init_subclass__") {
      let base_has_init_subclass = /* MRO walk, skipping self */;
      if base_has_init_subclass { validate_init_subclass_body(...)?; }
  }
  ```

  A base class that defines `__init_subclass__` with a side-effecting body
  was accepted unconditionally, and a subclass that does **not** define its
  own `__init_subclass__` was never checked against that inherited hook at
  all -- CPython runs the base's hook at every subclass's creation; pycc
  silently ran nothing and rejected nothing. This is a genuine correctness
  gap: a subclass whose base has a side-effecting `__init_subclass__`
  compiled and ran with observably different behavior from CPython, with no
  diagnostic warning the author that the hook was ignored.

- Decision:
  Split the two questions and resolve only the narrower one now:

  1. **Full PEP 487 invocation semantics (`__init_subclass__` actually
     running, and `__set_name__` triggering on class-level attribute
     assignment) are deferred out of v0.4 scope.** No fixture can be written
     today per #580's own inertness rule, and no fixture-backed row flip
     in `docs/PYTHON_STANDARDS.md` is possible until real invocation exists.
     A future issue picks this up once pycc's class-creation model can
     support real side effects at that point (or once a different resolution
     to the fixture-inertness problem is found).
  2. **The soundness gap is fixed now, independent of full invocation.**
     `crates/pycc_hir/src/class.rs`'s validation is extended so that when a
     subclass does **not** define its own `__init_subclass__` but a base in
     its MRO does, the compiler re-validates that inherited hook's body
     against the *subclass's creation site* (not the base's own definition
     site), rejecting it with the existing `unsupported()`/`C0001`
     diagnostic helper if it is not statically evaluable (only `pass` or a
     bare docstring). A base class that is never subclassed within the
     compilation unit stays legal regardless of its `__init_subclass__`
     body's shape -- CPython never invokes the hook until a subclass
     actually exists, so rejecting at the base's own definition would
     over-reject legal, standalone library-shaped classes.

     Implementation-wise, this needed access to an already-lowered base
     class's original method body, which `HirClassDef` (the type
     `defined_classes` carries) does not retain. Rather than add a new field
     to `HirClassDef` (155 construction sites across `pycc_hir`, `pycc_types`,
     `pycc_mir`, and their test suites -- a wide blast radius for a single
     boolean this fix does not need to persist past HIR-lowering), `lower_class`
     gained one additional parameter, `base_class_asts: &[(String, &pycc_ast::StmtClassDef)]`,
     threaded alongside `defined_classes` from `lower_checked`'s single
     module-lowering loop. It pairs every already-lowered *user-authored*
     class name with its original AST, so the new inherited-hook check can
     re-walk the base's real method body. A synthetic builtin-exception
     class (seeded into `defined_classes` with no AST counterpart) is simply
     absent from this slice; the check treats that absence as "nothing
     further to validate" -- consistent with this file's existing posture
     toward any base it cannot introspect elsewhere in `class.rs`.

  Issue #585's own completion criteria 1 and 2 are satisfied by this ADR and
  the code fix above. Criteria 3 (author
  `tests/fixtures/pep_0487_init_subclass.py`) and 4 (flip the PEP 487 row in
  `docs/PYTHON_STANDARDS.md`) are explicitly **not** satisfied and remain
  open -- the issue text itself gates them on "once a fixture can genuinely
  exercise the hook" and "once green CI evidence exists", neither of which
  a reject-only fix produces. Per this decision, #585 is **narrowed rather
  than closed**: its remaining scope is exactly criteria 3-4 (full
  invocation semantics), tracked as a comment on the issue, not folded into
  the umbrella issues D-192 exempts (this is ordinary, milestone-scoped
  incremental work, not a cross-cutting process area).

- Alternatives:
  - *Close #585 outright, treating the reject-only fix as full closure.*
    Rejected: the issue's title and its own completion criteria explicitly
    describe the invocation gap ("`__init_subclass__` never runs"), which
    this change does not resolve. Closing it would misrepresent the
    remaining, real, un-implemented PEP 487 semantics as done.
  - *Add a new `HirClassDef` field (e.g. `init_subclass_side_effecting:
    bool`) computed once at each class's own lowering, instead of a second
    AST-carrying parameter.* Considered and rejected for this change: it
    would touch all ~155 `HirClassDef` construction sites across three
    crates (most of them test fixtures with no `__init_subclass__` at all),
    for a boolean this fix only ever needs to consult during the same
    module-lowering pass that already has the original AST in hand. The
    chosen `base_class_asts` parameter is scoped to exactly the two call
    sites (`pycc_hir::lower_checked`, and the `mro.rs` unit test harness)
    that construct `defined_classes` in the first place.
  - *Validate every base in the MRO that defines `__init_subclass__`,
    instead of only the first one found by `mro.iter().skip(1).find(...)`.*
    Rejected as unnecessarily broad: exactly one `__init_subclass__` in the
    MRO is the one CPython's normal method-resolution order would actually
    invoke (the nearest one a `super().__init_subclass__()` chain would
    reach first); validating every MRO entry that happens to define the
    name would reject programs CPython accepts (a grandparent's
    side-effecting hook, shadowed by a statically-evaluable parent
    override, is never actually invoked by CPython and should not be
    rejected by pycc either). The existing `base_has_init_subclass` gate
    already used `.any(...)` for the boolean question ("does *some* base
    define it") and is left unchanged; the new code separately answers
    "which one" via `.find(...)` only when it needs the actual body.
  - *Attribute the diagnostic to the base class's own `__init_subclass__`
    definition site (`fd.range`) instead of the subclass's creation site
    (`def.range`).* Rejected: the base's own definition is legal on its own
    (per the boundary case above) -- the failure is specific to *this*
    subclass creating an invocation obligation pycc cannot honor, so the
    subclass statement is the accurate location for the diagnostic.

- Consequences:
  - #585 is narrowed (not closed): criteria 1-2 (this ADR, the soundness
    fix) are done; criteria 3-4 (fixture, `PYTHON_STANDARDS.md` row flip)
    remain open, gated on a future full-invocation-semantics issue.
  - This fix intentionally covers only the case where the subclass does
    **not** define its own `__init_subclass__`. A separate, still-open gap
    ([#854](https://github.com/rotnov/pycc/issues/854), found by the D-068
    pinned reviewer's pass on this change) is not covered: when a subclass
    *does* define its own `__init_subclass__` override, CPython's actual
    invocation target at that subclass's own creation is still the nearest
    ancestor's hook, never the subclass's own definition --
    `super(new_cls, new_cls).__init_subclass__(...)` in CPython's
    `type_new_init_subclass` starts its MRO lookup immediately after
    `new_cls`, so `new_cls`'s own `__init_subclass__` is never the one
    invoked at its own creation (it only matters later, when something
    subclasses `new_cls`). `crates/pycc_hir/src/class.rs`'s pre-existing
    (pre-#585) own-body check does not reflect this, so a subclass with a
    trivial override and a side-effecting ancestor hook is still silently
    accepted. #854 also tracks a related gap where a `@classmethod`-decorated
    `__init_subclass__` is invisible to this guard entirely. Both are left
    for a follow-up change since fixing them changes #435's existing test
    contract in both directions and needs its own test-fixture pass, not a
    mechanical extension of this one.
  - `docs/ROADMAP.md` and `docs/TYPE_SYSTEM.md`'s PEP 487 descriptions are
    updated in the same pull request: the inherited-side-effecting-hook case
    is no longer "silently ignored" -- it is now rejected at compile time
    with a diagnostic, cross-referencing this entry.
  - `validate_init_subclass_body` gained an `inherited: bool` parameter that
    selects the diagnostic wording (own-hook vs. inherited-hook), rather
    than a second near-duplicate function, keeping the `C0001` message
    source single.
  - `lower_class`'s public (`pub(crate)`) signature gained a fifth
    parameter, `base_class_asts`; its two call sites
    (`pycc_hir::lower_checked` and `class::mro::tests`) were updated. No
    other crate calls `lower_class` directly.
  - No `HirClassDef` construction site needed to change: the fix is fully
    contained within `pycc_hir`'s own class-lowering pass.
