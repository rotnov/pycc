---
id: D-207
title: "A compile-time int-literal boundary check is a single-pass pycc_hir fix, not three"
status: accepted
---

## D-207: A compile-time int-literal boundary check is a single-pass `pycc_hir` fix, not three

- Status: accepted
- Context:
  [D-178](./D-178-materialize-out-of-range-int-literals-through-a.md) made an
  out-of-range `int` literal materialize as a heap bigint everywhere,
  including at one of D-141's 14 runtime `int`-boundary positions (a
  container value, an index or repeat count, a slice bound). It knowingly
  accepted that this moved the failure for a literal at one of those
  positions from compile time to run time, and deferred a compile-time
  diagnostic to its own issue — [#618](https://github.com/rotnov/pycc/issues/618)
  — while stating, in its own "Alternatives" section, that fixing it "needs a
  new HIR/MIR notion of boundary position spanning 14 sites across three
  passes". #618's own filing carried that estimate forward verbatim and
  suggested the fix "plausibly" needed its own small sequence of pull
  requests, one per pass, per AGENTS.md's decomposition rule (large,
  independent architectural seams get their own pull requests).

  That estimate was never verified against the actual pipeline before being
  written down. `src/main.rs`'s `check_frontend` — the function `pycc check`
  actually calls — runs exactly two steps: `pycc_hir::lower_checked(&module)`
  followed by `pycc_types::check(&hir)`. Neither MIR nor codegen runs during
  `check` at all; a "three passes" fix is only necessary if the diagnostic
  must additionally fire during `build`/`run` through a different code path
  than `check`'s own, which it does not — `build`/`run` share `check`'s
  frontend and only diverge afterward. The 14-position count itself was
  already stale by the time #618 was filed: D-179 had already removed the
  `range()` operand from D-141's boundary inventory (`range()` is fully
  bigint-capable, so an out-of-range literal there is ordinary supported
  behavior, not a boundary failure), leaving 13. Of those 13 (every container
  literal element, `append`/`add`, `dict.get` defaults, subscript-assign
  values, comprehension elements, list index, slice bounds, and `str * int`
  repeat count), 12 are resolvable with nothing but the AST node already in
  hand during HIR lowering — no type information from `pycc_types` is needed
  to tell "this expression is an `int` literal with this value" from
  anything else. The 13th, `str * int` repeat count, is genuinely different:
  telling a `str`-typed variable from an `int`-typed one on the left operand
  needs type information `pycc_hir` does not have. But `pycc_types`, the pass
  that does
  have that information, is not a viable host for a *spanned* diagnostic
  here: 158 separate call sites in `crates/pycc_types/src/expr.rs`
  (`T0040`'s tuple-index diagnostic among them) already construct their
  `Diagnostic` with `Span::new(0, 0)`, because `HirExpr` carries no span
  information at all by the time it reaches `pycc_types` — spans exist only
  on the AST and are consumed into diagnostics during HIR lowering, then
  discarded. Building a real span for this one diagnostic in `pycc_types`
  would be a second, unrelated architectural project (giving `HirExpr` spans,
  or threading a parallel span map through `pycc_types`), not a natural
  extension of #618's scope.

- Decision:
  Implement the entire #618 fix as **one** pull request, entirely within
  `pycc_hir`, touching no other pass:
  - A new `crates/pycc_hir/src/int_boundary.rs` module exports
    `fits_tagged_smallint` (a third, deliberately duplicated copy of D-061's
    round-trip check — `pycc_rt::fits_smallint` and
    `pycc_codegen::int_const::fits_tagged_smallint` are the other two;
    `pycc_hir` cannot depend on either crate without a cycle, and no compiler
    crate links the target runtime as an ordinary Rust dependency) and
    `check_boundary_literal`, which builds a spanned `T0051` diagnostic from
    an already-lowered `HirExpr`, an AST-captured span, and a `&'static str`
    position label.
  - All 11 syntactically-resolvable positions in `crates/pycc_hir/src/expr.rs`
    and one in `crates/pycc_hir/src/stmt.rs` (dict subscript-assign) — 12
    fully-resolvable positions total — capture
    the operand's AST span via `pycc_ast::expr_range` *before* `lower_expr`
    discards the original `Expr`, then call `check_boundary_literal` with the
    lowered `HirExpr` and that span. `range()`'s own argument-lowering helper,
    `lower_range_call`, deliberately does not call `check_boundary_literal` at
    all — see the Context section above.
  - The 13th position, `str * int` repeat count, is narrowed rather than
    routed through `pycc_types`: it fires only when the string side is
    itself a string *literal* (`"ab" * <int-literal>`), not a `str`-typed
    variable. This is a deliberate, documented scope reduction, not a missed
    case — see [TYPE_SYSTEM.md](../TYPE_SYSTEM.md) rule 7 and
    [ROADMAP.md](../ROADMAP.md)'s Language surface row, both updated in the
    same pull request.
  - A new diagnostic code, `T0051`, registered in
    [DIAGNOSTICS.md](../DIAGNOSTICS.md) and `crates/pycc_diag/src/explain.rs`
    per D-150.

  This resolves #618's own open question — whether AGENTS.md's decomposition
  rule requires splitting the fix into a pass-ordered sequence of pull
  requests — as: no. There is only one architectural seam here (`pycc_hir`
  lowering), not three; decomposing a single-seam change into multiple pull
  requests would violate AGENTS.md's decomposition rule in the other
  direction (its bar is seam count, not line count, but it does not call for
  splitting a change that touches one seam into artificial fragments either).

- Alternatives:
  - *Follow #618's literal "three passes" suggestion and split the fix across
    HIR, MIR, and codegen pull requests.* Rejected: MIR and codegen never run
    during `pycc check`, so there is no boundary-position notion to add to
    either of them for this diagnostic. Splitting would create two empty or
    near-empty pull requests with no code to justify them.
  - *Route the `str * int` repeat-count case through `pycc_types`, which
    already has the type information to detect a `str`-typed variable
    operand generally.* Rejected: `pycc_types` has no real span for an
    `HirExpr`-level diagnostic (158 pre-existing `Span::new(0, 0)` instances
    in `crates/pycc_types/src/expr.rs`); giving it one is a separate,
    unrelated architectural project this issue's completion criteria do not
    require. The narrowed literal-only check catches the common case
    (`"literal" * <oversized literal>`) and documents the gap explicitly.
  - *Give `HirExpr` real span information so `pycc_types` diagnostics can be
    spanned generally, then implement the repeat-count check there.*
    Rejected as scope: this would fix a much older, wider pre-existing
    limitation (every `pycc_types`-level diagnostic, not just this one) as a
    side effect of a narrowly-scoped issue, and is a large enough change to
    need its own issue and plan if pursued.

- Consequences:
  - `pycc check` now catches all 12 fully-resolvable boundary positions plus
    the string-literal case of the 13th at compile time with a spanned
    `T0051` diagnostic, restoring the pre-#148 catch point for the literal
    case specifically. An arithmetically promoted bigint reaching any of
    those 13 positions is completely unaffected and still hits the run-time
    `pycc_rt_int_untag_checked` abort D-178 describes, unchanged. `range()`
    stays entirely out of scope, per D-179.
  - The D-061 round-trip check now has a third independent copy
    (`pycc_hir::int_boundary::fits_tagged_smallint`), alongside
    `pycc_rt::fits_smallint` and `pycc_codegen::int_const::fits_tagged_smallint`.
    All three must stay in sync if D-061's encoding ever changes; no shared
    crate currently exists that all three compiler/runtime crates could
    depend on without introducing a cycle.
  - The `str`-typed-variable case of the repeat-count position
    (`s * 4611686018427387904` for a `str`-typed `s`) remains an accepted,
    documented gap: it still compiles and aborts at run time, exactly as
    D-178 left it. Closing it fully would require either giving `HirExpr`
    real spans or building a second span-tracking mechanism in `pycc_types`,
    either of which is future work with its own issue if it is ever pursued.
  - This is the second D-178 boundary-inventory update after D-179 (which
    removed the `range` operand): the inventory this decision covers is the
    13 non-`range` positions D-179 left, plus the narrowed repeat-count case.
