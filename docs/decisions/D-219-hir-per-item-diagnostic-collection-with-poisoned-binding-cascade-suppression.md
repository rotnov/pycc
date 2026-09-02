---
id: D-219
title: "HIR lowering collects one diagnostic per top-level item and silently skips cascades of a skipped class or alias (issue #864, Part 2)"
status: accepted
---

## D-219: HIR lowering collects one diagnostic per top-level item and silently skips cascades of a skipped class or alias (issue #864, Part 2)

- Status: accepted
- Context: D-217 (Part 1 of #864) made the driver's failure payload a
  `Vec<Diagnostic>` and fanned out the parser, and recorded that Part 2
  (#867) would collect per top-level item in HIR lowering "with cascade
  suppression" without defining any of it. `pycc_hir::lower_checked` still
  returned on the first failing statement, so a corpus file whose first gap
  is an `import` on line 1 reported nothing about the rest of the file.

  Three facts measured against the tree shaped the decision. First, the only
  second-order HIR diagnostics that can follow a skipped item are two
  `C0001`s: the bare-name annotation arm of `annotation_to_ty` (`type
  annotation `A` is not supported yet`) and `validate_bases`'s unknown-base
  rejection. Both consult only the class table and the type-alias table, so
  only a *class* or *type-alias* binding can be their root: an annotation
  naming an import-, `def`-, or variable-bound name fails today with the same
  message whether or not that binding lowered, which makes it an independent
  gap rather than a cascade. `T0021` and `C0002` cascades cannot occur in this
  part at all -- `T0021` is the type checker's, which does not run after an
  HIR failure, and `C0002` is produced only for a failing `from ... import`
  statement itself. Second, a syntactic pre-scan of annotation positions and
  base lists cannot avoid false suppression: HIR resolves many names by
  spelling before any table lookup (`int`/`float`/`bool`/`str`/`Any`/`Self`/
  `Annotated`/`Final`/`TypeAlias`, the `Enum`/`Protocol`/`ABC` marker bases),
  a colliding later binding may leave an earlier valid one in place, a class
  may name itself in its own method annotations, and annotations inside a
  constant-folded `if TYPE_CHECKING:` block are never lowered -- a pre-scan
  would have silenced every `class Color(Enum)` after a failed `from enum
  import Enum, auto`. Third, `lower_checked` has roughly 320 callers, about
  110 of which consume the `Err` as a single `Diagnostic`.

- Decision:
  1. **Granularity is the top-level item.** `pycc_hir::lower_all(&ModModule)
     -> Result<HirModule, Vec<Diagnostic>>` lowers every top-level statement,
     pushes one diagnostic per failing item in source order, and skips that
     item as a unit: a failing `def` aborts only that function, a failing
     method aborts its whole class (a class-table entry is complete or
     absent), a failing alias, import, or plain statement is skipped whole.
     The `Err` is never empty. `lower_checked` stays as the first-element
     view (D-217's `parse`/`parse_all` precedent) so no test, bench, or
     downstream caller moves; only `src/frontend.rs::lower_frontend` calls
     `lower_all`.
  2. **HIR failures still stop before the type checker.** When `lower_all`
     collects anything, `lower_frontend` returns `FrontendFailure::Compile`
     with the HIR list and `pycc_types` does not run. A file still fails in
     exactly one pass (D-217 rule 3), no partial `HirModule` is ever
     type-checked, and `HirModule` gains no poisoned-name field: on `Ok` no
     item was skipped.
  3. **Cascade suppression is "lower first, then classify" over a poisoned
     set.** Every item is lowered; suppression is decided from the diagnostic
     it produced. The *poisonable* name of a statement is the class or
     type-alias name it would bind (`class C` -> `C`, `type X = ...` -> `X`,
     legacy `X: TypeAlias = ...` -> `X`); imports, `def`s, and assignments
     bind nothing poisonable. When an item fails for any reason its
     poisonable name is added to the set. When a later item fails with one
     of the two cascade-shaped `C0001`s naming a poisoned name, it is skipped
     *silently* -- no diagnostic of any kind -- and its own poisonable name
     is added too (transitive: `class B(A)` after a skipped `A` silences a
     following `class C(B)`). Every other failure is reported. The two
     producers build their messages through `pycc_hir::module`'s
     `unknown_annotation_name_message`/`unknown_base_message`, and the
     classifier `cascade_name` parses those back; a unit test round-trips
     them, so `docs/DIAGNOSTICS.md`'s "message text may change" stays true.
  4. **Rebinding un-poisons.** A later class or alias that binds a poisoned
     name and lowers successfully removes it from the set; a `def`, import,
     or assignment of the same name never does. Poisoned names never take
     part in the name-collision checks because a skipped item put nothing
     into any table.
  5. **First-diagnostic invariant (D-217 rule 2).** Nothing before the first
     failing item is skipped and its diagnostic is pushed unconditionally
     (the set is still empty), so the first collected diagnostic is
     byte-identical to the pre-#867 one. The post-loop phases (rotating the
     seeded exception classes, assigning exception type tags, seeding
     `Exception.__init__`) run only when nothing was collected, so the
     `>255 exception classes` `C0001` stays a one-element `Err`; the two
     exception-seeding gates are whole-module scans decided before the loop
     and unaffected by skipping. No re-sort anywhere (D-217 rule 3).

- Alternatives:
  - *A syntactic pre-scan of annotation positions and base lists before
    lowering.* Rejected: the four false-suppression classes above need an
    ever-growing exclusion list; classifying the real error has no such
    surface, since an item that lowered suppresses nothing.
  - *Poisoning every binding kind a skipped statement would introduce
    (imports, `def`s, assignments too), as the issue text said.* Rejected:
    the two cascade lookups cannot resolve those names, so an annotation
    naming one is a genuine gap today; poisoning `OrderedDict` after a
    skipped `from collections import OrderedDict` would silence a real
    `def f(d: OrderedDict)` diagnostic. Part 3 may widen the set if it ever
    flows poisoned bindings into `pycc_types`, where import- and def-bound
    names *are* looked up.
  - *A `note`/secondary diagnostic "skipped because `A` failed to lower".*
    Rejected: `Diagnostic` has no note or secondary-span concept, it would
    add a rendered line for consumers that fingerprint on code+span, and
    rustc-style silent suppression is the precedent users already read.
  - *Permanent poison (no un-poisoning).* Rejected: it would suppress a
    correct `def f(a: A)` after a correct redefinition of `A`.
  - *Running the type checker on the partial module and filtering with the
    poisoned set.* Rejected: it violates "one pass fails per file", it
    would immediately produce the `T0021` cascades the issue forbids unless
    the set crossed the crate boundary into every `pycc_types` name lookup,
    and module-global signature inference on a module with holes would
    report differently from the eventual full file. Revisit in #868 with
    evidence.
  - *Changing `lower_checked`'s return type.* Rejected: ~320 call sites for
    no behaviour gain; the first-error view is what they want.

- Consequences:
  - Under-reporting inside a cascade-skipped item is accepted and
    documented (`docs/DIAGNOSTICS.md`): the root-cause diagnostic is always
    reported, and fixing it surfaces the rest on the next run. One concrete
    instance: `class A:` fails, `def A() -> int` then lowers (no collision --
    `A` is in no table), and a later `def g(a: A)` is silent; had `class A`
    lowered, the user would instead have seen the def-vs-class collision
    `C0001`. Intended -- the root cause is what to fix first.
  - A cascade-shaped producer missing from the inventory means a *reported*
    cascade, never a hidden genuine gap -- the safe direction.
  - The module-level walk moves from `crates/pycc_hir/src/lib.rs` (past the
    ~1,000-line decomposition threshold) into `crates/pycc_hir/src/module.rs`
    with its own `module/tests.rs`; `lib.rs` re-exports `lower_all` and
    `lower_checked`.
  - `tests/diagnostics/c0001_issue_864_repro.*` now pins two `C0001`s (lines
    2 and 4, first render unchanged), and `c0001_hir_cascade_suppressed.*`
    pins an import gap, a rejected class, two silent cascades, and a later
    reported gap. Part 3 (#868) regenerates nothing here: with HIR failing,
    the type checker still does not run.
