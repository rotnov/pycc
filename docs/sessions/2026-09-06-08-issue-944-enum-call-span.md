# 2026-09-06 — #944: the enum-call `C0001` is reported at the call expression

## Previous checkpoint's outcome

`docs/sessions/2026-09-06-07-issue-966-inherited-init-rank.md` delivered
#966 as PR #970, since squash-merged to `main` as
`50a0dc2a` (verified with `git fetch --prune origin` immediately before this
entry was committed), taking decision number **D-232** and session file
`-07-`. This branch therefore takes **D-233** and `-08-`.

## Overall status

`feat/issue-944` (worktree `/Users/denis/projects/pycc-worktrees/issue-944`)
implements #944. It was authored on `89501bbb` (PR #968) and rebased onto
`50a0dc2a` (PR #970, #966) before review: three prose paragraphs
(`docs/ROADMAP.md` line 200, `docs/TYPE_SYSTEM.md` line 234, and the
generated `docs/decisions/README.md`) were merged by hand, keeping both
sides, and one fixup mirrors #970's new `HirClassDef.implicit_object_init`
field into the direct-HIR guard test. The pull request that delivers this
snapshot carries `Fixes #944`; nothing is merged by this branch until it
lands.

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

## Commits on the branch (after the rebase onto `50a0dc2a`)

- `e8e758fc` feat(hir): report the enum-call C0001 at the call expression (#944)
- `6870f4b0` test: pin the enum-call C0001 span end to end (#944)
- `d70d4b94` docs: D-233 -- HIR lowering gains a second per-item diagnostic source (#944)
- `4017a4c0` test(hir): keep the enum-call test helpers fully covered (#944)
- `570ab52a` docs(sessions): snapshot the #944 enum-call span delivery
- `2d4df343` fixup(rebase): mirror #970's implicit_object_init field in the direct-HIR enum guard test
- `38982a62` docs(sessions): record the #944 rebase onto 50a0dc2a and the post-rebase gate run
- `d7d18c48` docs/test: address the D-068 review round for #944
- (this commit) docs(sessions): record the D-068 review round; followed by `chore(harden): record #944 review findings`

## Gate results (single writer, run sequentially, exit codes captured to files)

Re-run in full after the rebase: the first post-rebase run failed
`clippy`/`test`/`llvm-cov` on the missing `implicit_object_init` field,
fixed in `2d4df343`; the re-run reports TOTAL 100.00% lines / functions /
regions (55031 regions, 36251 lines, 0 missed) and frontend throughput at
41.93 ms against the 75 ms threshold. The pre-rebase run, for the record:
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
- The branch is already rebased onto `origin/main` (`50a0dc2a`, #970,
  re-verified with `git fetch --prune` before this entry was committed);
  the generated `docs/decisions/README.md` table carries both the D-232 and
  the D-233 rows.

## D-068 review round

The pinned iEvo `deep-reviewer` (fresh context, full merge-base..HEAD range
after the rebase) reported no behavioural defect and three documentation
findings, all fixed in `d7d18c48` and recorded in
`.harden/findings/issue-944.jsonl`: (1) the `module.rs` doc and D-233
decision 6 overclaimed that a cascade-silenced item emits no diagnostic at
all, while the per-item scan runs after every item whatever its outcome --
both texts now scope the suppression to the lowering source, and the scan
deliberately stays unfiltered on the cascade outcome; (2) the `pycc_types`
guard's reachability comment now names only the scan's position-insensitive
over-suppression cases (limit (i)), since binding-form shadows yield `T0021`
instead; (3) D-233 decision 5's "every residual is pinned by a test" is now
literally true -- two unit tests pin the call-before-a-later-module-rebinding
and the comprehension-target-suppresses-a-sibling-call residuals. After that
commit, `cargo fmt --check`, `cargo clippy --workspace --all-targets -D
warnings`, `cargo test --workspace`, the decisions-index `--check`, and
`cargo llvm-cov --workspace --fail-under-lines 100 --fail-under-regions 100`
(run alone: TOTAL 100.00% lines / functions / regions, 55039 regions, 36259
lines, 0 missed) all exited 0.

## Where to resume

`docs/decisions/D-233-...md` for the design, `crates/pycc_hir/src/class/enum_call.rs`
for the scan and its residual-limit tests, `tests/issue_921_enum_call.rs` and
`tests/diagnostics/c0001_enum_class_call_*.py` for the end-to-end span pins.
