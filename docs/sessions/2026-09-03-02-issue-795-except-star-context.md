# 2026-09-03-02 -- Issue #795: `except*` rejects `return`/`break`/`continue` and group types

## Status: implemented, pull request open, not merged

Worktree `.claude/worktrees/autopilot-2026-09-02-08`, branch
`autopilot/iter-2026-09-03-02`, started from `origin/main` at `96c63a82`
and merged up to `origin/main` at `256fb7c8` after the peer session's #906
landed (that merge is what renumbered this change's ADR from D-222, which
#906 took first, to D-223).
[#795](https://github.com/rotnov/pycc/issues/795) (v0.4) is implemented on
that branch and a pull request is open against `main`; nothing is merged as
of this snapshot, and CI is watched by the dispatching session rather than
by the implementing agent. The plan this followed is
<https://github.com/rotnov/pycc/issues/795#issuecomment-5524700517>. The new
decision record is
[D-223](../decisions/D-223-reject-except-star-exceptiongroup-at-compile-time.md),
which narrows -- without editing -- the accepted
[D-202](../decisions/D-202-pep-654-except-star-and-exceptiongroup.md).

## What changed

Two independent PEP 654 divergences from CPython, both previously accepted
silently:

- **Gap 1 -- control flow out of an `except*` clause.** CPython makes
  `return`, `break` and `continue` inside an `except*` clause a
  `SyntaxError`; pycc compiled them. `crates/pycc_hir/src/stmt/exception.rs` now
  carries a three-state `pub(crate) enum ExceptStarCtx { Outside,
  InsideUnshielded, InsideLoopShielded }` (re-exported from `stmt.rs`, which
  keeps `stmt.rs` itself from growing past what AGENTS.md's ~1,000-line
  decomposability rule tolerates) threaded positionally exactly like
  the D-193 `in_finally` flag, through `lower_stmt`, `lower_body`,
  `lower_elif_else_clauses`, `lower_match`, `lower_except_handler` and the
  loop lowerings. A `return` anywhere inside an `except*` clause is rejected
  with `L0001`; `break`/`continue` are rejected only while unshielded,
  because a loop opened *inside* the clause makes them legal again
  (`ExceptStarCtx::shielded_by_loop`). At module scope the pre-existing
  `T0024` "'return' outside a function" still wins, which the
  `d0024_return_in_except_star_at_module_level` fixture pins.
  The three lowering entry points (`func.rs`, `class.rs`, `module.rs`) reset
  the context by passing the constant `ExceptStarCtx::Outside` -- a
  conditional reset there would add an unreachable branch that the D-014
  region gate cannot cover.
- **Gap 2 -- `except* ExceptionGroup`.** CPython raises `TypeError` at
  runtime for `except* ExceptionGroup` / `except* BaseExceptionGroup`; pycc
  has no mechanism to raise it. `check_try_star_stmt` in
  `crates/pycc_types/src/exception.rs` now rejects both names -- and any
  user class whose MRO reaches either of them -- at compile time with
  `C0001` and a message that says why. D-223 records that choice
  and why a compile-time `C0001` is the honest form of the divergence.

Ten new `tests/diagnostics/` fixture pairs cover both gaps plus the
shielded-`break`, `break`-inside-`finally` and module-scope precedence
paths; `crates/pycc_hir/src/stmt/exception.rs` hosts fifteen inline
`except_star_context_tests` for the context-threading states that have no
public-CLI surface (the shielded direction simply compiles).

Documentation updated in the same change: `docs/PYTHON_STANDARDS.md` rule 12
gains the `out-of-scope` carve-out for this gap (edited line-count-neutrally
because the conformance-breadth manifest fail-closed-checks
`matrix_line: 348`), and `docs/ROADMAP.md`'s exception-handling section
gains a dated `2026-09-03 / #795 / D-223` paragraph. `docs/RUNTIME.md` and
the `L0001`/`C0001` explain text in `crates/pycc_diag/src/explain.rs` (with
its `docs/DIAGNOSTICS.md` counterpart) describe the two new rejections. No
conformance matrix row moves: PEP 654 stays `◐`.

## How this task was run

One dispatched implementation agent under the standing `fix all opened issues`
directive (D-127), following `issue-implement` from its implementation step
onward: implementation, the full local gate set, then five D-068
`ievo:deep-reviewer` rounds. Rounds 1-4 each returned exactly one doc-drift
finding and each was fixed in its own commit; round 5 returned none. The
`/harden batch` pass over `.harden/findings/issue-795.jsonl` clustered those
four findings into three classes and shipped no artefact -- two classes are
recurrences of topics whose escalation ladder was already walked to
`build nothing`, and the third is a new singleton counter seed
(`comment-cites-a-sibling-that-was-never-created`). `docs/AGENT_RETROSPECTIVE.md`
gains one entry, for an unrelated locale mistake that did cost real time: a
Ruby checker's test suite failing with `invalid byte sequence in US-ASCII` is
an ambient-locale artefact, not a diff defect, and re-running it under
`LC_ALL=en_US.UTF-8` is the first thing to try.

## Deviations from the published plan

- The plan's §5 table had a `Try` node's `finalbody` reset `except_star` to
  `Outside`. It propagates the incoming value instead. A `finally` block is
  not inside the `except*` clause, but it *is* still inside whatever
  enclosing `except*` clause the `try` itself sits in, so resetting would
  have wrongly accepted `return` there; the
  `l0001_break_in_except_star_in_finally` fixture pins the propagating
  behavior.

## Follow-ups

- [#903](https://github.com/rotnov/pycc/issues/903) (v0.4) tracks raising a
  real runtime `TypeError` for `except* ExceptionGroup`, which is what would
  let D-223 be superseded rather than narrowed further.
- [#905](https://github.com/rotnov/pycc/issues/905) tracks the review finding
  this change declined: a body folded away by the `TYPE_CHECKING` constant
  fold is never lowered, so none of the new `except*` context checks (nor the
  pre-existing PEP 765 ones) see it. That is a property of the fold, not of
  this change, and fixing it belongs with #798's own rework of the fold.
- `crates/pycc_hir/src/stmt.rs` is still above the ~1,000-line threshold
  (1,148 lines after the extraction above, against 1,055 on the merge base).
  AGENTS.md requires decomposing the part a task touches, which this change
  did; the residual size has no dedicated D-185 tracker, and this run filed
  none because D-192's non-milestone ceiling is in force.

## Where a fresh session should resume

`git log origin/main..autopilot/iter-2026-09-03-02` is the whole change.
The open pull request for this branch is the live object -- check its
required checks and review threads before doing anything else; if it merged
while this snapshot sat, #795 is done and the next thing is #903.
