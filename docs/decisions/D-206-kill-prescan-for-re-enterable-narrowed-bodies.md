---
id: D-206
title: "Kill-prescan for re-enterable Optional[T] narrowed bodies (D-068 re-review of #780, third round)"
status: accepted
---

## D-206: Kill-prescan for re-enterable Optional[T] narrowed bodies (D-068 re-review of #780, third round)
- Status: accepted
- Context: D-205 (Part 2 of #747) implemented `Optional[T]` flow-sensitive
  narrowing as a single left-to-right source-order pass, reconciled only at
  control-flow *joins* (`join_if_branches`/`join_loop_body`/
  `join_match_branches`). Two earlier D-068 pinned-reviewer rounds against
  #780 already found and fixed soundness gaps in that join-reconciliation
  step itself (see D-205's own Alternatives section and
  `narrow::join_narrowed`'s doc comment). This third round found a
  different defect class in the same design: the source-order pass
  silently assumed a body's *execution* order always matches its *source*
  order. That assumption is false whenever a body can be re-entered such
  that a statement earlier in execution order runs after a kill that
  appears later in source order. Two concrete counterexamples, both
  accepted (wrongly) by the checker before this fix:

  ```python
  # 1. Loop re-entry: `print(x + 1)` precedes `x = None` in source order
  #    and was checked as narrowed against loop-entry state -- but the
  #    loop body re-executes, so on the second iteration `print(x + 1)`
  #    actually runs *after* the first iteration's own `x = None`.
  def f(x: int | None) -> int:
      if x is not None:
          i = 0
          while i < 2:
              print(x + 1)
              x = None
              i = i + 1
          return 0
      return -1

  # 2. Except-from-mid-try: the handler was checked against the pre-try
  #    narrowed state, but it is only ever entered after some prefix of
  #    the try body already executed -- here, after `x = None` already ran.
  def f(x: int | None) -> int:
      if x is not None:
          try:
              x = None
              raise ValueError("boom")
          except ValueError:
              return x + 1
      return -1
  ```

  Both were verified as real, exploitable unsoundness at the reviewed
  commit (`b7d03402`) before any fix: `check` exited 0 (accepted) and
  `run` executed the rejected-in-principle `int | None` arithmetic without
  a compile-time diagnostic. A third, related but non-soundness finding
  (a "warning" per the reviewer's severity classification) was that
  `check_match`'s case-body loop and `check_try_stmt`'s four body loops
  (`body`, each handler's `body`, `orelse`, `finalbody`) used a raw
  per-statement loop instead of `narrow::check_stmt_sequence[_in_function]`,
  so a nested early-return guard inside any of those bodies never narrowed
  the rest of that same body -- the identical fast-path-bypass defect the
  `if`/`while` fast-path helpers already had fixed in an earlier round, just
  never routed through `match`/`try` in the first place.

- Decision:
  1. **Kill-prescan, applied uniformly before checking or lowering any
     re-enterable body.** `pycc_hir::killed_names(body)` recursively
     collects every bare name `body` reassigns *anywhere* within it --
     the set of statement kinds that route a bare-name target through
     `check_assignment` (checker) or the equivalent MIR bind (`Assign`, a
     valued `AnnAssign`, a `ForRange`/`ForList` loop variable, and both
     `target`/`var` of `ListCompAssign`/`SetCompAssign`/
     `DictCompAssign`; `DictSet`/`AttrSet` write through a
     container/attribute slot, not a name binding, and are excluded),
     plus every `HirExpr::NamedExpr { name, .. }` (PEP 572 walrus, #774)
     found anywhere inside an `ExprStmt`'s expression or an `If`/`While`
     statement's own `test` -- the only two expression positions a walrus
     can appear in (`collect_named_expr_targets_in_expr`, a dedicated
     recursive expression walker `collect_killed_names` calls from those
     three statement arms, since a walrus target never has its own
     `HirStmt` variant to match on the way every other kill kind above
     does). A D-068 review of #780/#774's interaction (after this same PR
     merged main, which carries #774, into the branch this ADR already
     landed on) found the walrus half of this rule missing entirely --
     `collect_killed_names` put a bare `ExprStmt` in a no-op arm and never
     inspected `While`'s own `test` -- so a loop body whose only kill of a
     narrowed name was a bare walrus (`(x := None)`) was invisible to the
     prescan, corrected in place below per this same append-only
     exception (this entry is still unmerged into `main`, so it is not
     yet an "already-accepted" entry that rule protects).
     `pycc_types::narrow::apply_kill_prescan`/`pycc_mir`'s own
     `apply_kill_prescan` (crate-local, since `pycc_mir` cannot depend on
     `pycc_types`) each drop every killed name from the narrowing overlay
     for the *entire* body before the normal left-to-right pass begins --
     not from the kill's own source position onward. This is
     conservative (it can only ever drop a narrowing that was actually
     safe on some execution paths, e.g. a loop's first iteration before
     its own kill runs) rather than a precise fixpoint analysis, matching
     this repository's existing D-127 judgment call (recorded in D-205's
     Alternatives and re-applied here) that a narrower conservative rule
     which can only *drop* a valid narrowing is preferable to a more
     precise one that risks keeping an invalid one, at zero
     fixpoint-iteration cost.
  2. **Applied at every call site that checks/lowers a body capable of
     being entered such that execution order can precede source order for
     some statement in that body**: `while`/`for` loop bodies (including
     the `while`/`for` *test* expression itself, which also re-executes
     every iteration) and `try` `except` handler bodies (scanning the
     *try body's* kill set, not the handler's own -- a handler is entered
     only after some, possibly empty, prefix of the try body already ran).
     A straight-line body or an `if`/`else` with no enclosing loop or
     `try` needs no prescan: execution order there already equals source
     order, so the existing sequential pass is already sound, matching
     D-205 decision 2's scope (a `match` case body and a `try`'s `orelse`/
     `finalbody` are likewise not re-enterable in the sense this fix
     addresses, and need no prescan either -- `finalbody` in particular is
     checked (`check_try_stmt`, `crates/pycc_types/src/exception.rs`)
     against `joined`, an `Environment` built by folding `body_env` (the
     state after walking the *entire* try body top-to-bottom in source
     order, including any code after a `raise` -- still walked as dead
     code) through `join_loop_body`, then intersecting each handler's and
     the `orelse`'s end-state in turn via `join_if_branches`. Every one of
     those joins bottoms out in `narrow::join_narrowed`'s strict
     intersection (see its doc comment above), which already drops a name
     from the overlay unless *every* input map still narrows it -- so a
     kill anywhere in the try body, a handler, or the `orelse` already
     removes that name from `joined` before `finalbody` is ever checked,
     with no separate prescan needed).
  3. **Identical logic in both `pycc_types` and `pycc_mir`, sharing only
     the pure `killed_names` predicate via `pycc_hir`** -- the same
     cross-crate split D-205 decision 4 established for
     `optional_none_test`/`definitely_terminates`. `pycc_mir::expr.rs` has
     exactly one caller of `narrowed_ty` (`HirExpr::Name`'s lowering arm),
     reading the same `scopes` structure `apply_kill_prescan` mutates
     before any statement in a re-enterable body is lowered, so MIR
     eligibility tracks the checker's rule automatically with no
     additional per-call-site wiring once the prescan itself is correctly
     placed.
  4. **The warning finding (`match`/`try` under-narrowing) is fixed by
     routing those four now-identified raw per-statement loops through
     `narrow::check_stmt_sequence`/`check_stmt_sequence_in_function`**,
     the same helpers the `if`/`while` fast-path fix already introduced,
     rather than duplicating `apply_post_if_narrowing`'s call inline at
     each site.

- Alternatives:
  - *Scope-down instead of prescan*: restrict narrowing to bodies where
    execution order provably equals source order (straight-line and
    `if`/`else` with no enclosing loop or `try`) — rejected as the
    fallback per this task's own explicit ordering preference. The
    kill-prescan approach built cleanly, compiled without errors on the
    first attempt in both crates, and passed every existing test with
    zero regressions, so the fallback's added narrowing-capability loss
    (losing correct-and-safe narrowing for the common "read a narrowed
    value inside a loop that never touches it" shape, which the
    completeness-guard tests below exist specifically to keep working)
    was never actually needed.
  - *A true fixpoint / dataflow analysis over the loop's possible
    iteration states* — rejected as disproportionate to this codebase's
    current scope: the prescan already achieves soundness at zero
    iteration cost, and no existing narrowing consumer needs the
    additional precision a fixpoint analysis would recover (a loop body
    that reads a narrowed name and also kills it anywhere within the same
    body is already just as often *not* narrow-eligible on some real
    iteration as it is on others, so the extra precision's practical
    payoff is narrow).
  - *Treating a `while`/`for` test expression as exempt from the prescan
    (only prescanning the body)* — rejected: the test re-evaluates every
    iteration exactly like the body's own statements, so a read of a
    to-be-killed name inside the test is exactly as unsound as a read
    inside the body; `pycc_mir::stmt`'s `While` arm applies the prescan
    before lowering `test`, not only before `lower_loop_body` lowers
    `body` (the loop-body helper's own prescan call is a harmless,
    already-idempotent re-application for the body itself).

- Consequences:
  - **Narrows D-205's Consequences claim** that "a narrowing fact ...
    never survives a reassignment of the narrowed name" — that statement
    was true only for the constructs D-205 itself covered (straight-line
    bodies and `if`/`else` joins); it was never true in general once
    re-enterable bodies (`while`/`for` loops, `try`/`except` handlers)
    entered scope, as this entry's two counterexamples demonstrate. The
    accurate statement, going forward: a narrowing fact never survives a
    reassignment of the narrowed name *reachable from any point later in
    execution order than the read*, which for a re-enterable body means
    "anywhere in the body," not merely "earlier in source order."
  - `docs/decisions/D-205-optional-t-flow-sensitive-narrowing-part2.md`
    itself is left unedited per this repository's decision-log convention
    (an accepted entry is never hand-edited; a superseding entry narrows
    it instead) — a reader relying on D-205 alone should cross-reference
    this entry for the corrected scope of its narrowing-survival claim.
  - The kill-prescan is deliberately whole-body and non-positional: a read
    of a narrowed name that occurs *before* the body's own kill in both
    source and execution order (e.g. the loop counterexample's first
    iteration) is now rejected too, even though that specific read would
    have been sound on that one iteration. This trades a small amount of
    narrowing precision for a soundness guarantee that costs no
    additional analysis passes — consistent with D-205's own
    `join_narrowed` precedent of preferring "can only drop, never keep an
    invalid narrowing" over maximal precision.
  - `match` case bodies and `try`'s `body`/handler/`orelse`/`finalbody`
    bodies now all route through `narrow::check_stmt_sequence`/
    `check_stmt_sequence_in_function`, so a nested early-return guard
    inside any of them narrows the rest of that same body — closing the
    last remaining gap in the fast-path-bypass defect class two earlier
    D-068 rounds had already fixed for `if`/`while`.
  - `crates/pycc_hir/src/lib.rs`'s new `killed_names`/`collect_killed_names`
    functions are now the canonical way to answer "which names might a
    re-enterable body's execution kill," available to any future
    flow-sensitive analysis that needs the same soundness property this
    entry establishes for `Optional[T]` narrowing.
  - A second, narrower D-068 finding from the same #780/#774-interaction
    review round is not a `killed_names`/prescan gap at all, but a direct
    application of D-199's own original kill-on-assign rule that this PR's
    walrus merge missed: `pycc_mir::expr::pre_bind_named_expr_targets` (the
    pre-pass that performs the actual MIR-level `bind_variable` for every
    walrus target, mirroring `HirStmt::Assign`'s own paired
    `kill_narrowing`/`bind_variable` calls in `stmt.rs`) called
    `bind_variable` for a `NamedExpr` target but never `kill_narrowing` --
    so `(x := None)` inside a narrowed `if` branch left the stale
    `$narrowed:{name}` sentinel in place, and a read right after it kept
    unconditionally lowering to `MirExpr::OptionalUnwrap` for a value the
    walrus had just overwritten with `None`. Fixed by adding the same
    `kill_narrowing` call `pre_bind_named_expr_targets` was missing, no new
    decision needed since it is exactly D-199's existing rule applied to a
    binding shape (#774's walrus) that postdates D-199 itself.
