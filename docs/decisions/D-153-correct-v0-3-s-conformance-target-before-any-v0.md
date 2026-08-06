---
id: D-153
title: "Correct v0.3's conformance target before any v0.3 PR starts"
status: accepted
---

## D-153: Correct v0.3's conformance target before any v0.3 PR starts

- Status: accepted
- Context: `docs/ROADMAP.md`'s v0.3 ("classes & pattern matching") accept bullet
  reads "conformance ≥ 45 PEPs," inherited unverified from before
  `docs/PYTHON_STANDARDS.md`'s per-PEP matrix and #374's own decomposition
  plan existed in their current form — the same root cause D-088 already
  found and fixed once for v0.2's own three accept bullets. #374's
  decomposition plan (issue comment, 2026-08-06) re-derived the arithmetic
  and found an 11-row gap; this decision is the itemized, per-PEP
  feasibility pass Correction 3 of that plan called for before either
  closing the gap with a breadth PR or revising the target, mirroring
  D-088's own "attempt the real fix first, fall back to a corrected number
  only for the honest residual" discipline.

  **Two ways to count "45 PEPs," and both matter.** `PYTHON_STANDARDS.md` is
  operationally checked by row (`grep -c "✅"`), the same unit D-088 used
  ("≥15 conformance-matrix rows... encompassing 17 distinct PEP numbers");
  ROADMAP.md's bare "45 PEPs" wording does not itself disambiguate rows from
  PEP numbers, and ROADMAP.md's v0.3 section does not add any qualifier.
  Two of PYTHON_STANDARDS.md's rows are one-row/multi-PEP: `634–636`
  (`match`, PEP 634 plus 635/636) and `649/749` (deferred annotations).
  Following D-088's own convention, this decision counts primarily by row
  (the CI-checkable unit) and reports the distinct-PEP-number figure
  alongside it at every step, so neither reading is left silently unstated.

  **Current state (verified, `grep -cE "✅" docs/PYTHON_STANDARDS.md`,
  excluding the legend line): 15 rows checked, encompassing 14 distinct PEP
  numbers** (one checked row, `dict` insertion order, is unnumbered).

  **The v0.3-implied surface (#374's Correction 3 itemization, re-verified
  row-by-row against `PYTHON_STANDARDS.md`'s actual current rows): 19 rows,
  encompassing 22 distinct PEP numbers** — 3135, 3119, 3129, 409, 3151, 435,
  487, 557, 560, 544, 634–636 (one row, 3 numbers), 654, 673, 681, 695's
  separate generic-classes row, 698, 649/749 (one row, 2 numbers), 758, 765.

  15 + 19 = **34 rows against the ≥45 target: an 11-row gap** (row basis).
  14 + 22 = **36 PEP numbers against the ≥45 target: a 9-PEP-number gap**
  (number basis). Either reading falls well short of 45.

  **Per-PEP feasibility pass on the plan's own PR-23 candidate list**
  (3102, 3104, 3132, 448, 570, 572, 604, 586, 591, 593, 589), performed by
  reading `crates/pycc_hir/src/{lib,stmt,expr}.rs` and
  `crates/pycc_mir/src/lib.rs` directly rather than assuming "no class
  model needed" is the same thing as "cheap":

  - **Genuinely reachable without new subsystems (3 of 11):**
    - **PEP 570 (positional-only params, `/`)** — `lower_params`
      (`crates/pycc_hir/src/lib.rs`) already special-case-rejects
      `parameters.posonlyargs` with a dedicated message; the parser already
      populates it. Keyword call arguments are globally unsupported today
      (`expr.rs`'s `Expr::Call` arm rejects `!call.arguments.keywords.is_empty()`
      unconditionally), so positional-only parameters are already the *only*
      calling convention this compiler has — accepting `posonlyargs` the
      same way `args` is accepted today is a narrow, self-contained change.
    - **PEP 593 (`Annotated[X, ...]`)** — needs one new `annotation_to_ty`
      arm recognizing `Expr::Subscript` for the `Annotated` name and
      unwrapping to `X`'s own `Ty`, discarding the metadata arguments. This
      is not a shortcut: PEP 593's own spec says a type checker that does
      not understand a piece of `Annotated` metadata must treat
      `Annotated[X, ...]` exactly as `X` — unwrapping *is* the correct
      behavior, not an approximation of it.
    - **PEP 591 (`Final[X]`)** — same bounded `annotation_to_ty` unwrap as
      593, plus a real reassignment diagnostic (a new `T0xxx` code) so the
      row tests actual PEP 591 semantics (a `Final`-annotated name may not
      be reassigned) rather than only its syntax.
  - **Considered and rejected for this PR (2 of 11), not for missing
    prerequisite machinery but because implementing them honestly right now
    would repeat a mistake already logged twice in this project's own
    history:**
    - **PEP 586 (`Literal[...]`)** — the only bounded implementation
      available without new `Ty` machinery is unwrapping `Literal[3]` to
      plain `int`, which drops literal-value narrowing entirely. That is
      exactly the "accept the syntax, drop the semantics, and let a
      differential fixture pass anyway" failure D-088's own context section
      caught for PEP 526 and PEP 594, and the v0.2 design doc's Update note
      caught a third time for PEP 649/749. Not repeated a fourth time to
      manufacture a row this decision does not need (see Decision below).
    - **PEP 572 (walrus `:=`)** — `crates/pycc_mir/src/lib.rs`'s `MirExpr`
      has no side-effecting variant at all (no expression form that can
      perform a binding as part of producing a value); assignment-expression
      semantics need new MIR/codegen infrastructure for expression-position
      side effects, not a bounded annotation-only change like 591/593.
  - **Not reachable without a genuinely new prerequisite subsystem, class
    model unrelated (6 of 11):**
    - **PEP 3102 (keyword-only params)** — meaningless without keyword call
      arguments, which are globally rejected today (see PEP 570 above); a
      kwonly parameter can only ever be *called* by keyword.
    - **PEP 3104 (`nonlocal`)** — meaningless without nested function
      definitions, which `crates/pycc_hir/src/stmt.rs`'s `lower_stmt` does
      not handle at all inside a function body (no `Stmt::FunctionDef` arm
      exists there; only `lib.rs`'s module-level dispatch handles function
      defs).
    - **PEP 3132 (extended unpacking, `a, *b = ...`)** — needs basic
      multi-target tuple/list destructuring assignment first, which does not
      exist in any form: `Stmt::Assign` lowering
      (`crates/pycc_hir/src/stmt.rs`) accepts only a bare name or a
      bare-name subscript as an assignment target today.
    - **PEP 448 (unpacking generalizations, `*a`/`**b` in calls and
      literals)** — needs `Expr::Starred`/double-starred handling in call
      arguments and container literal construction, neither of which exists;
      `expr.rs`'s `Expr::Call` arm has no starred-argument handling.
    - **PEP 604 (union syntax, `int | str`)** — needs a new `Ty::Union`
      variant plus real union assignability/narrowing across
      `pycc_types`/`pycc_mir`/`pycc_codegen`, not an annotation-parsing
      change; `pycc_hir::Ty` (`crates/pycc_hir/src/lib.rs`) has no union
      variant today.
    - **PEP 589 (`TypedDict`)** — needs a new structural, per-key-typed dict
      representation distinct from `dict[K, V]`'s existing homogeneous
      `Ty::Dict(Box<(Ty, Ty)>)` shape; real new type-system work, not a
      bounded annotation unwrap.

  PR-23 (breadth PEP sweep) therefore closes **3 of the 11 candidate rows**
  (570, 591, 593), not the full 11 the plan's candidate list made available
  in principle. New reachable total: 34 + 3 = **37 rows**, encompassing
  36 + 3 = **39 distinct PEP numbers**. Both readings still fall short of
  45 — the residual 8-row / 6-PEP-number gap is exactly Correction 3's
  anticipated fallback case: "falling back to (b) only for whatever residual
  gap remains after an honest per-PEP feasibility pass."

- Decision:
  1. Revise `docs/ROADMAP.md`'s v0.3 accept bullet from "conformance ≥ 45
     PEPs" to "conformance ≥ 37 `PYTHON_STANDARDS.md` matrix rows green,
     encompassing 39 distinct PEP numbers (D-153 — revised down from 45 with
     itemized justification, mirroring D-088's precedent for v0.2)."
  2. The itemized 37-row target is: the 15 rows already checked today, plus
     the 19-row v0.3-implied surface from this decision's Context section
     (owning PRs recorded in `docs/DELIVERY_PLAN.md`'s new "v0.3 execution
     strategy" section and the design doc's own PEP table), plus PR-23's 3
     newly reachable rows (570, 591, 593). Reaching more than 37 during v0.3
     is welcome (any of PEP 3102/3104/3132/448/604/589/586/572 could close
     partially or fully if its prerequisite subsystem lands as a side effect
     of other v0.3 work, or in a later milestone's own breadth sweep) but is
     not required; the gate is set to what the itemized, feasibility-checked
     scope can honestly deliver, not backfilled to justify an unverified
     number.
  3. PEP 586 and PEP 572 are recorded as deliberately not attempted in v0.3
     for the reasons in Context above (paragraph 7, "Considered and
     rejected"), not silently dropped — a future revision of this target may
     reconsider either once their prerequisite gap (a real literal-narrowing
     `Ty` shape; a side-effecting MIR expression form) is closed for
     unrelated reasons.
- Alternatives: keep "≥45 PEPs" as aspirational rather than binary (rejected
  — same reasoning as D-088: ROADMAP.md's own milestone definition requires
  binary acceptance criteria, and an unreachable "binary" criterion is worse
  than a corrected one). Expand PR-15..23's scope to add the extra grammar
  features needed to reach 45 literally — nested functions/closures for
  `nonlocal`, destructuring assignment for extended unpacking, starred call
  arguments for PEP 448, a real `Ty::Union` for PEP 604, a structural
  per-key-typed dict for `TypedDict` (rejected — each is a materially new
  subsystem unrelated to v0.3's own named surface (classes, `match`,
  exceptions); pulling all of them forward to hit an unverified number is
  the same undocumented scope creep D-088 already rejected for v0.2, now at
  a larger scale). Implement PEP 586/572 anyway via the bounded
  unwrap-and-drop-semantics or partial approach (rejected — see Context;
  this project has already caught and logged that exact mistake twice, and
  D-153 exists specifically to not make it a third and fourth time).
- Consequences: `docs/ROADMAP.md`'s v0.3 accept line is rewritten to cite
  this decision. `docs/DELIVERY_PLAN.md`'s new "v0.3 execution strategy"
  section and the `docs/superpowers/specs/2026-08-06-v0-3-classes-pattern-matching-design.md`
  design doc are both scoped to this decision's itemized 37-row target, not
  the original unverified 45. A future milestone (or a later v0.3 PR, if one
  of PR-15..23's own work happens to unblock a deferred PEP as a side
  effect) may close PEP 3102/3104/3132/448/604/589/586/572 without
  needing a further ADR, as long as the closing evidence is itemized the
  same way this one is — only *lowering* an already-corrected target again
  needs a new decision.
