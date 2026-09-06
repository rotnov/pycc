# 2026-09-06 — #944: the enum-call `C0001` is reported at the call expression

## Previous checkpoint's outcome

`docs/sessions/2026-09-06-07-issue-966-inherited-init-rank.md` delivered
#966 as PR #970, since squash-merged to `main` as
`50a0dc2a` (verified with `git fetch --prune origin` immediately before this
entry was committed), taking decision number **D-232** and session file
`-07-`. This branch therefore takes **D-233** and `-08-`.

## Overall status

`feat/issue-944` (worktree `/Users/denis/projects/pycc-worktrees/issue-944`)
implements #944 on top of `89501bbb` (PR #968). At the time of writing it is
four commits ahead of that base and one merge behind `origin/main` (#970's
merge); it is **not pushed and has no pull request** — the dispatching
session owns push, review, and merge. Nothing is merged by this branch yet.

## What this change is

`HirExpr` carries no spans, so the enum-call `C0001` that #921 added to
`pycc_types::class::resolve_instantiation` rendered at `1:1`. Unlike the
abstract-class, protocol-class, and builtin-exception siblings in that
ladder, the enum case is decidable syntactically, so the reporting site moves
into `pycc_hir` where the call's range still exists:

- `crates/pycc_hir/src/class/enum_call.rs` (new) collects the module's
  syntactic enum classes (`class X(Enum)` / `class X(StrEnum)`, a sole bare
  base) and, per top-level item, reports one `C0001` per bare-name,
  keyword-free call to an enum class at the call's own span. A frame stack
  (module body, each `def` with its parameters, each `lambda` with its
  parameters; `Store` names, `except ... as`, `match` captures) suppresses the
  report when any enclosing frame rebinds the callee, so every shadowing
  shape keeps its accurate `T0021` from `pycc_types`. The residual limits are
  enumerated in the module doc and each is pinned by a unit test.
- `crates/pycc_hir/src/module.rs` runs the scan after each item's own
  lowering outcome (`Ok` or `Err` alike), filtering the D-219 poisoned names
  out of the class set, and appends its diagnostics right after the item's
  own one — loop order (D-217 rule 3) is preserved.
- `pycc_hir::enum_class_call_message` is the single owner of the #942
  message; `resolve_instantiation` now calls it, so the spanned and the
  span-less rejection render byte-identically. That guard stays as defense
  in depth (reachable through import ordering and the scan's documented
  over-suppression cases) and keeps a direct-HIR unit test for the D-014
  region gate.
- `docs/decisions/D-233-hir-lowering-gains-a-second-per-item-diagnostic-source.md`
  amends D-219 decision 1 ("one diagnostic per failing item" now describes
  the lowering source only); `docs/DIAGNOSTICS.md`, `docs/TYPE_SYSTEM.md`,
  and the existing #379/#432 paragraphs of `docs/ROADMAP.md` are updated
  in place (no new landing-paragraph marker, so `site/status/` is untouched
  and the freshness check reports no signal).

Smoke on the issue's program: `pycc check` and `pycc build` both moved the
`C0001` from `issue.py:1:1` to `issue.py:10:9`; every shadowing shape
(parameter, local, `except ... as`, `match` capture/star/mapping-rest,
module-level assignment and `for` target) still yields `T0021`.

## Commits on the branch

- `38673195` feat(hir): report the enum-call C0001 at the call expression (#944)
- `64f20979` test: pin the enum-call C0001 span end to end (#944)
- `02dd2b79` docs: D-233 -- HIR lowering gains a second per-item diagnostic source (#944)
- `c83902f3` test(hir): keep the enum-call test helpers fully covered (#944)

## Gate results (single writer, run sequentially, exit codes captured to files)

`cargo fmt --check`, `cargo clippy --workspace --all-targets -D warnings`,
`cargo test --workspace`, `cargo llvm-cov --workspace --fail-under-lines 100
--fail-under-regions 100` (TOTAL 100.00% lines / functions / regions after
`c83902f3`; the first run found two uncovered test-scaffolding regions in
`enum_call.rs`), the `scripts/` unittest suite, the decisions-index
`--check`, roadmap-evidence checker and its test, CI-permissions checker and
its test, `check-site.sh`, agent-policy validation, scratch-dir usage,
conformance breadth, status-page freshness (checker and test), README
coverage badge (checker and test), the replicated-perf checker's own test,
frontend throughput (39.74 ms against a 75 ms threshold), `classify_ci_changes`
(`compiler=true`, `agent=true`, `pages=false`), both alpha skill-eval
clients, and `cargo doc --workspace --no-deps` all exited 0. The
`--include-ignored` conformance oracle and the replicated paired-perf
regression checker were not run: the first needs CPython 3.14.7 (the machine
has 3.14.6) and the second consumes CI-produced criterion artifacts.
`cargo doc` prints seven pre-existing intra-doc-link warnings in files this
branch does not touch.

## Known follow-ups

- #877 (thread spans through `pycc_types`) still owns the abstract-class,
  protocol-class, and builtin-exception `1:1` renderings.
- The branch must be updated over `origin/main` (`50a0dc2a`, #970) before
  its pull request; the only expected overlap is the generated
  `docs/decisions/README.md` table (D-232 row versus D-233 row).

## Where to resume

`docs/decisions/D-233-...md` for the design, `crates/pycc_hir/src/class/enum_call.rs`
for the scan and its residual-limit tests, `tests/issue_921_enum_call.rs` and
`tests/diagnostics/c0001_enum_class_call_*.py` for the end-to-end span pins.
