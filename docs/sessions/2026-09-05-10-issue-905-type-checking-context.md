# 2026-09-05 — #905: validate `if TYPE_CHECKING:` bodies for context violations

## Previous checkpoint's outcome

Iteration 12 delivered [#934](https://github.com/rotnov/pycc/issues/934)
(reject a protocol class in return-annotation position with `C0001`): PR
[#947](https://github.com/rotnov/pycc/pull/947) merged by squash as
`ee979196ce49314949dc1ac6ed2f0d9eadc4132c` at 2026-09-05T19:38:39Z and #934
is CLOSED. Follow-ups [#948](https://github.com/rotnov/pycc/issues/948) and
[#949](https://github.com/rotnov/pycc/issues/949) were filed from that
iteration's "to file" list. A fourth data point for the nbody performance
flake was recorded on [#641](https://github.com/rotnov/pycc/issues/641)
([issuecomment-5554230191](https://github.com/rotnov/pycc/issues/641#issuecomment-5554230191):
`main` `f37ca75f`, run
[33982547932](https://github.com/rotnov/pycc/actions/runs/33982547932), job
[101350330425](https://github.com/rotnov/pycc/actions/runs/33982547932/job/101350330425),
18.49x, passed on rerun).

Post-merge `main` runs for `ee979196` were all `success`: CI
[33987730414](https://github.com/rotnov/pycc/actions/runs/33987730414), Main
history audit
[33987730417](https://github.com/rotnov/pycc/actions/runs/33987730417),
Status page freshness
[33987730420](https://github.com/rotnov/pycc/actions/runs/33987730420), and
Pages [33987730430](https://github.com/rotnov/pycc/actions/runs/33987730430).

## Overall status

Implemented [#905](https://github.com/rotnov/pycc/issues/905) on
`autopilot/iter-2026-09-05-13`, cut from `ee979196`, which was still
`origin/main` at push time (fetched immediately before the push; no merge
was needed and no other pull request was open). One pull request carrying
`Fixes #905`; the orchestrating session watches CI and merges. The issue was
reconfirmed at `ee979196` before the first edit — a guarded `finally`-
`return` and a guarded `break` both exited 0 under `pycc check` — and the
issue and open-PR list were re-checked before the first edit, at the first
commit, and before the push (state OPEN throughout, comments only the
`issue-to-plan` plan and this session's own claim, no open PR referencing
905). The plan is the `issue-to-plan` comment on #905
([issuecomment-5554542166](https://github.com/rotnov/pycc/issues/905#issuecomment-5554542166),
published against `ee979196`); this snapshot records where the
implementation followed it and where it deviated.

## What the change is

#790 constant-folds an `if TYPE_CHECKING:` / `elif TYPE_CHECKING:` body away
in `crates/pycc_hir/src/stmt.rs` before `lower_expr` or `lower_body` ever
sees it, which is what lets a guarded body contain constructs pycc does not
implement. The fold also swallowed every *context* check `lower_stmt`
performs, so a `return` in a `finally`, a `break`/`continue` with no
enclosing loop, a `yield`/`yield from` outside a function or an `async for`
outside an `async def` compiled silently when hidden behind the guard —
CPython rejects all of them at compile time whether or not the branch runs.

The new `crates/pycc_hir/src/stmt/type_checking.rs` is a total syntactic
walker over the guarded body, called at **both** fold sites (`lower_stmt`'s
`Stmt::If` arm and the `elif` arm inside `lower_elif_else_clauses`) with the
fold site's own incoming `(in_loop, in_function, in_finally, except_star)`
passed through unchanged. It reports only `L0001` `context_invalid`
diagnostics and stays silent wherever `lower_stmt` would have produced a
`C0001`. Two mechanisms hold that contract against drift:

- **Shared predicates.** `return_context_violation`,
  `break_context_violation` and `continue_context_violation` live in the new
  module and are called by `lower_stmt`'s own `Stmt::Return`/`Stmt::Break`/
  `Stmt::Continue` arms as well as by the walker, so each rule and each
  message exists once. `lower_stmt` falls through to its pre-existing
  `C0001` "`break`/`continue` inside a loop" message exactly when the
  predicate returns `None`.
- **Delegation and a recursion gate.** An expression statement is handed to
  the real `lower_expr` and only an `L0001` is forwarded (`expr.rs` raises
  `context_invalid` in exactly the two `yield` places, so this can neither
  miss a violation nor over-report a capability gap). A nested body is
  walked only after the real lowering helpers accept the non-body parts
  around it — a `while`/`if` test through `lower_expr`, a `for`'s
  `else`/target/iterable through `lower_for`'s own shape checks (including
  `lower_range_call`), an `except` handler's type through
  `lower_except_handler`'s — so the walker can never report a violation
  unguarded code would not have reached.

Residual gaps, deliberate and recorded in both
`docs/decisions/D-223-*.md` and `docs/RUNTIME.md`: a module-scope `return`
under the guard (CPython's fatal error there is `'return' outside function`,
pycc's separate `T0024` pass); a `yield` that is not the whole of an
expression statement (`x = (yield 3)` is a `Stmt::Assign`, which the
statement-level walker does not visit); the body of a nested `def`/`class`;
a `from __future__ import ...` (#919's own `L0001`s, out of scope);
and every body whose enclosing statement's non-body parts do not lower —
`match` cases, a `while`/`for` with an `else`, a non-lowering `while`/`if`
test, a non-bare-name `for` target or unsupported iterable, and an `except`
handler whose type is neither a bare name nor a non-empty tuple of bare
names.

Tests: 51 in-crate `#[cfg(test)]` unit tests in the new module (integration
tests contribute nothing to this crate's own region coverage under D-014),
10 end-to-end CLI tests in `tests/issue_905_type_checking_context.rs`
including the #790 regression guard that a guarded body of unimplemented
constructs still checks cleanly, and three byte-exact fixtures
`tests/diagnostics/l0001_{return_inside_finally,break_outside_loop,yield_outside_function}_type_checking.py`
registered in `tests/diagnostics_test.rs`. Docs: `D-223`'s consequences
(closure plus the five residual gaps), `docs/RUNTIME.md`'s `except*`
section, `docs/DIAGNOSTICS.md`'s `L0001` row, `docs/STDLIB_PLAN.md`'s
`typing` row, and the `#382` exception-handling paragraph in
`docs/ROADMAP.md` (prose-only — no new feature paragraph, so the status-page
four-pin rotation is not triggered; `check_status_page_freshness.rb
origin/main` reports no signal).

## Gates

All run from the worktree, all exit 0: `cargo fmt --all -- --check`;
`cargo clippy --workspace --all-targets -- -D warnings`;
`cargo test --workspace` (4696 passed, 0 failed, 58 ignored, across 86 test
binaries); the CI coverage sequence
(`cargo build --target x86_64-apple-darwin -p pycc_rt`,
`cargo build --workspace`, `cargo build --release -p pycc_rt`,
`cargo llvm-cov --workspace --fail-under-lines 100 --fail-under-regions
100`: TOTAL regions 53409/0 missed = 100.00%, lines 35112/0 missed =
100.00%, and `crates/pycc_hir/src/stmt/type_checking.rs` itself 471
regions / 395 lines, both 100.00%);
`python3 -m unittest discover -s scripts -p 'test_*.py'` (996 tests, OK,
6 skipped); `check_roadmap_evidence.rb`;
`check_status_page_freshness.rb origin/main` (no signal); `check-site.sh`;
`check_conformance_breadth.py` (39 evidence-backed rows);
`check_readme_milestone_projection.rb`;
`generate_decisions_index.py --check`; `check_ci_permissions.rb` (10 files);
and `cargo doc --workspace --no-deps`. Nothing touched appears in
`tests/fixtures/policy-successor-manifest.json`.

The pinned D-068 reviewer (`ievo:deep-reviewer`) ran on the staged diff and
returned one finding, a documentation-completeness gap: the new module's
"Deliberate silences" list claimed parity with `D-223`/`RUNTIME.md` while
naming four of the five residual gaps. Fixed before the first commit by
adding the `from __future__ import ...` bullet. No correctness finding.

## Deviations from the plan

- The plan described the entry point as returning `Option<Diagnostic>`; it
  returns `Result<(), Diagnostic>` so both fold sites read as a plain `?`.
  Behaviourally identical.
- The plan said the walker "selects the byte-identical message itself" for
  the `yield`/`yield from` rules. It instead delegates a bare expression
  statement to the real `lower_expr` and forwards only `code == "L0001"`,
  which is literally what `lower_stmt`'s own `Stmt::Expr` arm does, so the
  message and the span are exact by construction rather than by convention.
  This narrows the plan's residual gap #2: a `yield` *is* now caught when it
  is the whole of an expression statement, and the surviving gap is a
  `yield` in a compound expression (`x = (yield 3)`). The gap wording in
  `D-223` and `docs/RUNTIME.md` was written to the narrowed shape.
- No new ADR was written: the shared predicates landed as planned, so the
  plan's own condition for `D-231` was not met.
- `cargo test --workspace` was run without `--include-ignored`: the local
  oracle is CPython 3.14.6 and the ignored conformance tests require the
  pinned 3.14.7. CI runs them.

## Known follow-ups

- [#798](https://github.com/rotnov/pycc/issues/798) — the same fold site:
  `is_type_checking_guard` is not import-gated and not shadow-aware, so a
  module defining its own truthy `TYPE_CHECKING` still diverges from
  CPython. Untouched here and unaffected by this change.
- [#948](https://github.com/rotnov/pycc/issues/948),
  [#949](https://github.com/rotnov/pycc/issues/949) — filed from iteration
  12 (spurious `T0046` for a self-referential protocol member; `T0022`
  wording on a public protocol-parameter function).
- [#944](https://github.com/rotnov/pycc/issues/944) — the enum-call `C0001`
  (#921) renders at `1:1`; carry the call expression's span.
- [#932](https://github.com/rotnov/pycc/issues/932) and the rest of the open
  `v0.4` set for `issue-select` to weigh.

## Paused autopilot

- Directive scope: open-ended (`/goal fix all opened issues`).
- Active milestone: `v0.4` (Accept unmet).
- Last iteration outcome: #934 closed by PR #947 (`ee979196`).
- This iteration: #905 implemented; PR open, awaiting CI and merge by the
  orchestrating session.
- Next step: re-enter `issue-select` for `v0.4` after this PR lands.
- Denylist: empty.

## Where to resume

`crates/pycc_hir/src/stmt/type_checking.rs` is the whole of the new logic;
its module doc comment carries the contract and the full list of deliberate
silences, and its in-crate tests enumerate every branch. The two call sites
are `lower_stmt`'s `Stmt::If` arm and the `elif` arm of
`lower_elif_else_clauses`, both in `crates/pycc_hir/src/stmt.rs`. Closing
any residual gap means extending the walker there: the module-scope `return`
gap belongs to `T0024`'s pass rather than to this walker, the compound-
expression `yield` gap needs an expression-level walk (explicitly out of
scope in the plan), and the recursion-gate group closes on its own as the
underlying constructs gain real lowering support. If the shared predicates
are ever changed, both `lower_stmt`'s arms and the walker change with them
by construction — that is why they were extracted.
