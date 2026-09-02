---
id: D-220
title: "The type checker collects one diagnostic per failing function and merges the solver's and the annotation checker's lists solver-first per function (issue #864, Part 3)"
status: accepted
---

## D-220: The type checker collects one diagnostic per failing function and merges the solver's and the annotation checker's lists solver-first per function (issue #864, Part 3)

- Status: accepted
- Context: D-217 (Part 1 of #864) made the driver's failure payload a
  `Vec<Diagnostic>` and recorded that Part 3 (#868) would "collect per
  function in the type checker"; D-219 (Part 2) kept HIR failures stopping
  before the type checker and deferred the type checker "to #868 with
  evidence". `pycc_types::check` and `check_and_resolve` still returned one
  `Diagnostic`, so a corpus file with three broken functions reported one.

  Four facts measured against the tree at `a65d1a16` shaped the decision.
  First, "solver-first" was a per-module rule, implemented twice: `check`
  inlined it and `check_and_resolve` reached it through
  `checked_function_signatures`. The sequence is (a) the redefinition and
  attribute-redeclaration pre-checks, (b) if every function is fully
  annotated, one pass of the annotation checker against the annotations
  (the "concrete fast path"), done if clean, (c) otherwise the
  private-helper solver, reporting *its* first diagnostic on failure, (d)
  otherwise the annotation checker with the solver's signatures. So the
  byte-stable first diagnostic of a fully annotated module with errors is
  the solver's view of the first failing body even though the checker
  already found something in (b) -- pinned by
  `concrete_fast_path_preserves_solver_first_diagnostic_selection`.
  Second, the solver walks every function body, so most body errors are
  solver diagnostics (`return "a"` in an `-> int` function is `T0022`;
  `-"s"` is `T0021`), while attribute reads on non-instances (`T0043`),
  undefined names, wrong arity, and a failing top-level `x: int = "s"`
  (`T0025`) reach the driver only from the checker. A solver-only list
  would therefore hide every checker-only class behind any solver error
  elsewhere in the file. Third, some solver diagnostics are raised only in
  post-body phases: every `BinOp` allocates a fresh inference term and
  defers its operator check to `propagate_binop_constraints`, which runs
  after every body was walked, so `x + "s"` in `f` is reported *after* a
  later `g`'s body -- verified by appending an `h` with a body-walk error,
  which then wins. Such a diagnostic is unattributable to a function.
  Fourth, every `pycc_types` diagnostic still carries the `Span::new(0, 0)`
  placeholder D-043 records (165 sites, none with a real span), so every
  type diagnostic renders at `:1:1`.

- Decision:
  1. **Granularity is the function, methods included.** `pycc_types::
     check_all(&HirModule) -> Result<(), Vec<Diagnostic>>` and
     `check_and_resolve_all(&HirModule) -> Result<HirModule,
     Vec<Diagnostic>>` report one diagnostic per failing
     `HirItem::Function` -- a function's own first error, exactly what
     `check_function_in` returned before -- and at most one module-level
     diagnostic. Methods are `HirItem::Function`s with mangled
     `Class.method` names, so a class with two broken methods yields two
     diagnostics with no special casing. The `Err` is never empty.
     `check` and `check_and_resolve` stay as first-element views (D-217's
     `parse`/`parse_all` and D-219's `lower_checked` precedent, including
     the libcore `expect` on the never-empty list), so no test, bench, or
     downstream caller moves; only `src/frontend.rs` calls the `_all`
     entry points, forwarding the `Vec` exactly as it forwards HIR's.
  2. **Every collector is keyed internally.** A collector returns
     `Vec<(Option<usize>, Diagnostic)>`: `Some(i)` is the index of the
     failing function in `hir.items`, `None` is module level (a top-level
     statement or a whole-module phase). The public list is the keyed list
     with the keys dropped; nothing sorts by span or deduplicates by
     content -- two functions with the same error produce two identical
     renders, which is what "one per failing function" means.
  3. **Module-level phases stay one-element.** The two pre-checks
     (`check_incompatible_redefinitions`,
     `check_incompatible_attribute_redeclarations`) return alone and
     nothing else runs: both are whole-module consistency checks whose
     failure makes every later function's environment unreliable. The
     annotation checker's pass 2 (top-level statements over a growing
     environment) stops at its first failure as `(None, d)` and pass 3 is
     not run on the partial environment; pass 3 (function bodies against
     the final environment, D-041) runs for every function and records
     `(Some(i), d)` per failure. The solver's top-level constraint walk
     stops at its first failure as `(None, d)`; its per-body loop records
     `(Some(i), d)` from the body walk or the implicit-return unification
     and continues with the next body; when any body was collected, the
     post-body phases (`propagate_binop_constraints`,
     `apply_annotation_defaults`, the `T0021 cannot infer` resolution
     loop) do not run -- none of them ran after a body failure before
     either -- so a solver list is either exactly `[(None, d)]` or entirely
     `Some`-keyed. The post-check phases of `check_and_resolve_all` (the
     signature materialization clone, `monomorphize`, `unroll_enum_loops`)
     run only after checking passed and keep returning one diagnostic; they
     are not "the type pass's collection".
  4. **The merge is solver-first per function.** With `S` the solver's
     keyed list and `C` the concrete fast path's keyed list (present only
     when every function is annotated): if `C` exists and is empty, `Ok`
     (the fast path, unchanged); otherwise run the solver. On `Err(S)`: if
     `S` is `None`-keyed, report `S` alone; otherwise report `S` in its own
     order, then every entry of `C` whose key is not among `S`'s keys, in
     `C`'s order. On `Ok(signatures)`, report the annotation checker's
     per-function list against those signatures. Per key the solver's
     diagnostic wins when both phases flagged a function (the old rule,
     applied to every key instead of the first); functions only the checker
     flagged are still reported. The `None` rule exists because a
     post-phase solver diagnostic cannot be matched by key to the checker's
     `Some(f)` entry for the same `x + "s"`, so appending `C` would report
     the one error twice under two keys. Order is "solver entries, then
     checker-only entries", not a global item-order interleave: a
     checker-only diagnostic for an earlier function would otherwise
     precede the solver's first flagged function and break the next rule.
  5. **First-diagnostic invariant (D-217 rule 2).** The pre-checks come
     first as before, rule 4 emits `S[0]` first, and the solver-passes arm
     emits pass 2's first failure or the first failing function's first
     error in item order -- the same precedence the single-diagnostic
     driver had. `check(hir)` is therefore byte-identical to the pre-#868
     result for every input; a differential sweep of every `.py` file under
     `tests/` against the `a65d1a16` binary found zero first-line
     differences, and unit tests pin literal `(code, message)` pairs from
     that binary for each phase.
  6. **HIR failures still stop before the type checker (D-219 rule 2).**
     Nothing here touches `pycc_hir` or `lower_frontend`; the type checker
     only ever sees a fully lowered module, so no poisoned set crosses the
     crate boundary.

- Alternatives:
  - *Solver list only when the solver fails (drop `C`).* Rejected: it hides
    every checker-only class (`T0043`, `T0025`, undefined names, arity)
    behind any solver error elsewhere in the file -- a corpus file with one
    `return "a"` typo would report nothing about its attribute errors.
  - *Global item-order interleave of `S` and `C`.* Rejected: breaks D-217
    rule 2 whenever a checker-only failure precedes the solver's first
    flagged function.
  - *Continue pass 2 or the solver's top-level walk past a failure.*
    Rejected: the environment after a failed top-level statement is
    partial, so every later lookup can produce a false `T0021`; a
    top-level statement is rarely the failing construct in corpus code, and
    the cascades avoided are the exact noise D-219 excludes.
  - *Snapshot and restore the solver's union-find per body so a failed body
    leaves no trace.* Rejected: `unify_terms` returns before mutating the
    representative on a conflict, so the union-find only ever holds
    constraints that were accepted; the unifications a failing body made
    before its error are genuine constraints that body imposes, and erasing
    them would make a later body's diagnostic depend on whether an
    unrelated statement in an earlier body happened to fail. The copy is
    also O(variables) per function on the failure path for no reporting
    gain.
  - *Per-statement collection inside a body.* Out of scope, as in D-219: a
    body is checked over a flowing environment.
  - *Running the type checker after a HIR failure with D-219's poisoned
    set.* Still rejected for D-219's reasons; nothing here changes that
    evidence.
  - *Changing `check`/`check_and_resolve`'s return type.* Rejected: about
    310 call sites for no behaviour gain; the first-error view is what
    they want.

- Consequences:
  - Under-reporting after a module-level failure is accepted and documented
    in `docs/DIAGNOSTICS.md` and `docs/CLI_SPEC.md`. A pre-check failure
    or a module-level failure of the solver (a top-level solver conflict
    or a post-body phase failure -- including the pre-existing `T0021
    cannot infer` for an unresolvable unannotated helper, which already
    hid every other diagnostic in the file) stops the pass at one
    diagnostic. The annotation checker's own failing top-level statement
    (its pass 2) stops *that* collector at one entry and skips its pass 3;
    under rule 4 that entry is reported alone only when the solver flagged
    no function, and otherwise follows the solver's per-function entries.
    Either way the root cause is reported; fixing it surfaces the rest.
  - One observable coupling: after body `f` fails, the constraints it
    accepted before failing stay in the solver, so a later body `g` may be
    reported for a conflict with a type `f` established, and fixing `f` may
    change `g`'s message. The diagnostic is still a genuine inconsistency
    in the module. The acceptance fixture is fully annotated so no test
    depends on it.
  - Every type diagnostic still renders at `:1:1` (D-043), so the second
    and third renders of `tests/diagnostics/t0022_types_per_function.*` are
    distinguished by message only, and two functions with the same error
    fingerprint identically for a consumer keyed on code plus span shape.
    This part does not widen to add spans; a follow-up issue (filed at PR
    time, #TBD) tracks threading real spans through `pycc_types`, inheriting
    D-043's "regenerate fixtures, never hand-edit" rule.
  - The module-level driver moves from `crates/pycc_types/src/lib.rs` (past
    the ~1,000-line decomposition threshold, #544) into
    `crates/pycc_types/src/module.rs` with its own `module/tests.rs`, and
    the solver's signature entry points move from `constraints.rs` into
    `constraints/signatures.rs`; the move is its own pure-move commit so a
    first-diagnostic drift is attributable to the behaviour commit.
  - `tests/diagnostics/t0022_types_per_function.*` pins three diagnostics
    in the merge order `f` (solver, `T0022`), `h` (solver, `T0021`), `g`
    (checker-only, `T0043`) -- not source order -- with the first render
    unchanged. No existing `.expected.*` file changed.
