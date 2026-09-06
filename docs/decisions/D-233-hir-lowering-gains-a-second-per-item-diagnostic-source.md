---
id: D-233
title: "HIR lowering gains a second, syntactic per-item diagnostic source: the enum-call scan (issue #944, amending D-219)"
status: accepted
---

## D-233: HIR lowering gains a second, syntactic per-item diagnostic source: the enum-call scan (issue #944, amending D-219)
- Status: accepted
- Context: `HirExpr` carries no spans, so every diagnostic `pycc_types` emits
  against an expression renders at `1:1`
  ([#877](https://github.com/rotnov/pycc/issues/877) tracks threading spans
  through that crate). The enum-call `C0001` that
  [#921](https://github.com/rotnov/pycc/issues/921) added to
  `pycc_types::class::resolve_instantiation` is one of those, and
  [#944](https://github.com/rotnov/pycc/issues/944) asks for it at the call
  expression. Unlike its abstract-class, protocol-class, and
  builtin-exception siblings in the same ladder, the enum case can be
  decided *syntactically*: a top-level class whose sole base is the bare
  name `Enum`/`StrEnum` is an enum class to `lower_class` already, so an
  AST-level scan inside `pycc_hir` still has the call's range. But
  [D-219](D-219-hir-per-item-diagnostic-collection-with-poisoned-binding-cascade-suppression.md)
  decision 1 describes `lower_module` as having exactly one diagnostic
  source -- "one diagnostic per failing item ... and skips that item" -- and
  an accepted decision is never edited, so adding a second source needs its
  own entry.
- Decision:
  1. `lower_module` runs a second, **syntactic** per-item collection source
     -- the enum-call scan in `pycc_hir::class::enum_call` -- on every
     top-level item after that item's own lowering outcome, `Ok` or `Err`
     alike. The scan reports one `C0001` per bare-name call to an enum class
     at the call expression's own span, with the #942 message shared through
     `pycc_hir::enum_class_call_message` so the spanned and the span-less
     rejection render byte-identically. The scan walks an item only when
     at least one enum class is in its name set: the unconditional walk
     cost the `pycc check` frontend bench about 7% (PR #971's
     `frontend-perf-gate`, threshold 7%), and a module with no enum class
     -- the common case -- must not pay for a diagnostic it can never emit:
     the module frame (every name the module body binds) is built on the
     first item scanned, never for such a module.
  2. Its diagnostics are appended immediately after the item's own
     diagnostic, so the collection order is still loop order
     ([D-217](D-217-report-every-frontend-diagnostic-per-pass-with.md) rule
     3) and, *per item*, the item's own diagnostic still comes first.
  3. An `Ok` item may therefore contribute diagnostics, and a failing item
     may carry `1 + N`. D-219 decision 1's "one diagnostic per failing item"
     now describes the *lowering* source only.
  4. `lower_checked`'s first-element view is unchanged in shape, but its
     first element may now be a scan diagnostic: `c = Color()` followed by
     an unsupported `with` statement reported the `with` `C0001` alone
     before, and reports the enum `C0001` at the call first now. That is the
     intended fix, and it is outside D-217 rule 2's byte-stability promise,
     which is scoped to the #864 parts rather than a permanent freeze of
     every first diagnostic's span.
  5. The scan models scope-local bindings so a call on a name that shadows
     the enum class keeps its accurate `T0021` from `pycc_types`: a stack of
     frames (the module body, each `def` with its parameters, each `lambda`
     with its parameters) records the names bound directly in that scope
     (`Store` names, `except ... as`, `match` captures) and suppresses the
     report when any frame binds the callee. The frames are not
     position-aware, do not descend into a nested scope, and record no
     `def`/`class`/`import` name; a class body gets no frame. The scan and
     the frames fold `if`/`elif TYPE_CHECKING:` bodies exactly as
     `lower_stmt` does (#790, D-223), so a call or a binding inside such a
     dead body is neither reported nor a shadow. A name that a second
     module-level `def`, `class`, `import`, or `type` statement also binds
     is dropped from the name set for the whole module: that program is a
     collision the class item reports, and the scan must not put a
     false-kind enum-call report ahead of it (an identical repeated import
     binds the same definition twice, is no collision, and stays in the
     set; one enum imported under one name through two module paths is
     indistinguishable from a rebinding, because HIR records no
     defining-module provenance for a re-exported class, and is one more
     residual shape the span-less guard reports at `1:1`; a
     `from __future__` import binds nothing and never counts). Every residual
     is enumerated in the module doc of `class::enum_call` and pinned by a
     test: over-suppression is the only failure mode on an item that lowers
     (the call still fails in `pycc_types`), and the one false-kind report
     is a second diagnostic on a class-body call that already fails.
  6. D-219 decisions 2-5 (no partial HIR is type-checked, poison
     classification, rebinding un-poisons, the first failing item is never
     skipped) are untouched: the scan filters the currently poisoned names
     out of its class set and never poisons anything. The cascade
     suppression itself stays a property of the lowering source only
     (decision 3): the scan runs after every item whatever its outcome, so
     an item whose own diagnostic was silenced as a cascade can still
     report a true enum-call `C0001`. Poison stays
     order-dependent for it, as for D-219 itself: a `def` that calls
     `Color(1)` *before* a failing `class Color(Enum): pass` is scanned
     before the class item poisons `Color`, so that program reports the
     (true) enum call and then the class's own diagnostic; the reverse order
     is a suppressed cascade.
- Alternatives:
  - *Emit in `lower_expr`'s `Expr::Call` arm with the call's range.*
    Rejected: `lower_expr` has no class table and no scope, so it would need
    a new parameter threaded through a pervasive signature, and it still
    could not see a class defined *after* the `def` that calls it; the
    pre-scan's syntactic pre-collection is what makes source order
    irrelevant.
  - *Thread the call span through HIR (`HirExpr::Call { span }`).* That is
    #877; it touches every `HirExpr` constructor and every `pycc_types`
    consumer, and it would fix the sibling diagnostics too. Not this issue.
  - *A bare-name scan with the shadowing limit merely documented* (the
    superseded PR #943's shape). Rejected: it turns six accurate `T0021`s
    (parameter, local, `except ... as`, `match` capture, module-level
    assignment, module-level `for` target) into a misleading enum-call
    `C0001` against programs CPython also rejects as "`int` is not
    callable"; the frame stack is ~50 lines inside the visitor.
  - *Scope decorators, defaults, and annotations only to the `def`'s own
    frame* (the first-cut simplification). Rejected during review: a walrus
    in a default value binds the enum's name in the enclosing scope, so a
    later module-level `Color()` was reported as a false-kind enum call
    outside a class body. The binder now walks a nested definition's
    decorators, type parameters, parameters, return annotation, and class
    bases into the enclosing frame (Python's actual rule); the `def`'s own
    frame still sees them too, a harmless over-suppression whose only
    observable consequence is a missed *second* diagnostic on an item that
    already fails (a decorator or default expression is `C0001` on its own).
  - *Key the scan on `enum_members` non-emptiness.* Rejected for the reason
    #942 keyed the guard on `is_enum`: a docstring-only enum (#744) has no
    members.
- Consequences: `lower_all` callers that read the first diagnostic may now
  see a scan diagnostic first -- deliberate. Any future syntactic per-item
  check has a precedent and a slot (after the item's own outcome, before the
  next item) rather than needing its own ordering rule.
  `resolve_instantiation`'s guard becomes defense in depth: it stays
  end-to-end reachable through import ordering (a `def` that calls an
  imported enum before the import that makes it known) and the scan's
  over-suppression cases, and it keeps a direct-HIR unit test so the D-014
  coverage gate still executes the `is_enum` branch now that the CLI path
  is intercepted in HIR. The abstract/protocol/builtin-exception siblings
  stay at `1:1` until #877.
