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
- `9b1615d2` docs(sessions): record the D-068 review round for #944 (also lands `.harden/findings/issue-944.jsonl`)
- `3416241c` fix(hir): fold TYPE_CHECKING bodies out of the enum-call scan (#944)
- `2bc3164c` docs(sessions): record the PR #971 review round for #944
- `a5a6fb5b` perf(hir): skip the enum-call scan when no enum class is known (#944)

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

## PR #971 review round

PR #971 (`Fixes #944`, head `9b1615d2` at open) came back CI-green but
`BLOCKED` on one Codex review thread (P1, `crates/pycc_hir/src/module.rs`):
the scan walked the original AST, so a call inside an `if`/`elif
TYPE_CHECKING:` body that `lower_stmt` constant-folds away (#790, D-223)
was reported as an enum-call `C0001` and rejected a module the lowering
accepts. Confirmed against `stmt.rs`'s fold and fixed in `3416241c`: both
the call scan and the frame binder route every `if` through
`walk_if_as_lowered`, which skips a guarded body (calls and bindings) with
the same `is_type_checking_guard` predicate and the same import view the
fold used for that item; the module frame is computed before the loop and
so keeps an aliased `t.TYPE_CHECKING` module-level residual (limit (vi),
over-suppression only). Nine unit tests pin the folded, live, and residual
shapes; D-233 decision 5, `docs/DIAGNOSTICS.md`, and the scan's module doc
record the fold. Round 2 of `.harden/findings/issue-944.jsonl` records
the finding. After that commit `cargo fmt --check`, `cargo clippy
--workspace --all-targets -D warnings`, `cargo test --workspace`, the
decisions-index `--check`, and `cargo llvm-cov --workspace
--fail-under-lines 100 --fail-under-regions 100` (run alone: TOTAL 100.00%
lines / functions / regions, 55173 regions, 36331 lines, 0 missed) all
exited 0. `origin/main` was still `50a0dc2a` and #971 the only open pull
request when this entry was committed.

## PR #971 perf round

Head `2bc3164c` failed `frontend-perf-gate`: the paired `pycc check`
frontend median regressed 9.9% against the exact predecessor (threshold
7%); the first run at `9b1615d2` had passed at 6.9%. Reproduced locally
with `cargo bench --locked --bench check_bench`: `origin/main`
(`50a0dc2a`) 8.11 us median versus 8.73 us on the branch, +7.6% -- a real
cost, not runner noise. The cause was the unconditional per-item walk
(the whole item plus each `def` body again for its frame) on a bench
fixture with no enum class. `lower_module` now builds the name set as
borrowed `&str`s and calls the scan only when it is non-empty; the same
local bench then reads 8.28 us, +1.8% against `main`. D-233 decision 1
and the scan's doc record the rule. Round 3 of
`.harden/findings/issue-944.jsonl` records the finding.

## Where to resume

`docs/decisions/D-233-...md` for the design, `crates/pycc_hir/src/class/enum_call.rs`
for the scan and its residual-limit tests, `tests/issue_921_enum_call.rs` and
`tests/diagnostics/c0001_enum_class_call_*.py` for the end-to-end span pins.

## Second rebase, onto `5f942512`

While the perf round's gates ran, the concurrent actor merged PR #972
(`site/issue-569-human-first`: `site/` files plus `docs/ROADMAP.md`) and
`origin/main` moved from `50a0dc2a` to `5f942512`. The branch was rebased onto
`5f942512` without conflicts (`git merge-tree` reported a clean merge before
the rebase). The rebase rewrote every branch commit, so the short SHAs quoted
above and the `fix_commit` values in `.harden/findings/issue-944.jsonl` name
the pre-rebase commits; the delivered squash commit supersedes all of them.
No Rust source changed between `50a0dc2a` and `5f942512`, so the perf-round
gate run (fmt, clippy, test, `llvm-cov` at 100.00% lines/regions) stands;
`check_roadmap_evidence.rb`, the decisions-index freshness check, the findings
checker, and `cargo check --workspace` were re-run after the rebase and
passed. PR #973 (`autopilot/iter-2026-09-06-22`, #969) is open and overlaps
this branch on `crates/pycc_hir/src/class.rs`, `crates/pycc_hir/src/lib.rs`,
`docs/ROADMAP.md`, `docs/TYPE_SYSTEM.md`, and `docs/decisions/README.md`;
whichever merges second rebases.

## PR #971 second Codex round (def-bound names)

A Codex P2 thread on `crates/pycc_hir/src/module.rs` (posted against head
`2bc3164c`, found after the perf push) showed a real false-kind report:
`def Color() -> int` / `Color()` / `class Color(Enum)` reported the enum-call
`C0001` at the call first and the class/function collision second, because
the syntactic pre-collection knew `Color` before its class item was reached
and the frame binder records no `def` name. Reproduced with `pycc check`
on all three orderings (def-call-class, class-def-call, and a `def`-body
call). Fix: `class::enum_call::module_function_names` lists the module-level
`def` names and `lower_module` drops them from the scan's name set for the
whole module (limit (vii) in the module doc; D-233 decision 5 and
`docs/DIAGNOSTICS.md` gained the sentence), so such a program carries the
collision diagnostic alone. Four unit tests and one end-to-end test pin it.
Round 4 of `.harden/findings/issue-944.jsonl` records the finding.

## Third rebase, onto `00b0f5a0`

PR #973 (#969, D-234) merged while the def-bound-name round's CI ran and
the watcher reported #971 as CONFLICTING. Rebased onto `00b0f5a0`:
`crates/pycc_hir/src/lib.rs` (both sides extended the `pub use class::{..}`
re-export; both kept), `docs/ROADMAP.md` and `docs/TYPE_SYSTEM.md` (#969
rewrote the same paragraphs; resolved by taking `main`'s text and
re-applying #944's four sentence-level edits, each base sentence verified
to survive verbatim in `main`), and `docs/decisions/README.md` (regenerated
with `generate_decisions_index.py`; D-233 and D-234 both listed). #969
changed `pycc_hir` source, so the full gate set was re-run from a
single-writer baseline after the rebase: fmt, clippy, `cargo test
--workspace`, `llvm-cov` at 100.00% lines/regions, `cargo doc`,
`check_roadmap_evidence.rb`, the decisions-index freshness check, and the
findings checker all passed. The commit SHAs quoted in earlier sections
are pre-rebase identifiers.

## PR #971 third Codex round (rebound names, span wording)

Head `93db52e1` went fully green (the `frontend-perf-gate` included) but
Codex opened two more P2 threads. (1) `crates/pycc_hir/src/module.rs`: the
def-only filter left an ordinary `class Color`, a `from m import Color`,
or a `type Color = int` preceding `Color()` and `class Color(Enum)` with
the same false-kind report. Reproduced with `pycc check` for the ordinary
class and the project import (the `type` alias was already frame-suppressed
and the aliased `from ... import ... as` import fails and poisons the name).
Fix: `module_function_names` became `module_rebound_names` -- every name
two or more module-level `def`/`class`/`import`/`type` statements bind --
and `lower_module` drops those from the scan set; limit (vii), D-233
decision 5, and `docs/DIAGNOSTICS.md` restated accordingly; two more unit
tests (ordinary class, `type` alias), one more end-to-end test (project
import), and a helper test over the import shapes. (2) `docs/DIAGNOSTICS.md`
promised the call-expression span categorically; the sentence now names the
residual fallback to the span-less guard at `1:1` (module-level rebinding
anywhere in the module, sibling-comprehension rebinding). Round 5 of
`.harden/findings/issue-944.jsonl` records both.

## PR #971 fourth Codex round (failed same-named def)

Codex P2 (thread `PRRT_kwDOTiOo7s6fr7RV`): a same-named `def` that fails to
lower (unannotated `def Color()`), followed by `class Color(Enum)` and
`Color()`, still counts for `module_rebound_names`, so the call is never
scanned and the collision never fires (the def bound nothing); Codex
proposed suppressing only names whose declarations actually bind or
collide. Reproduced both orders with `pycc check`: def-first reports
`T0001` at the def, class-first reports the collision at the def. Judgment
fork resolved by an independent advisor round (D-127), verdict refute: the
program carries two independent errors and the root-cause `T0001` is always
reported, after which the collision surfaces on the next run -- the same
under-reporting downstream of a failed same-named item that D-219's
`class A:` / `def A()` / `def g(a: A)` example documents as intended.
Outcome-based suppression would emit a false-kind `C0001` at a `Color()`
that lexically resolves to the def, which is the wrong-kind report limit
(vii) was pinned to prevent in the two earlier rounds; the limit stays
syntactic so collision ownership never depends on lowering outcomes.
Replied and resolved; round 6 of `.harden/findings/issue-944.jsonl` records
the refutation. The one-region coverage gap in `module_rebound_names`
(two unreachable `Option` branches) closed with `.expect` in `f7d2de0e`.

## PR #971 fifth Codex round (repeated import, file size)

Two more findings on `4d7216c7`, both fixed in `c7c77dde`. (1) P2: an
identical repeated import (`from colors import Color` twice, then
`Color()`) counted as a rebinding for limit (vii), so the call was never
scanned and fell to the span-less backstop at `1:1` -- reproduced with
`pycc check` -- although `lower_module` accepts the repeat as binding the
same class twice. `module_rebound_names` now keys each import binding on
its imported definition and treats a repeat of the same (local, source)
pair as no rebinding; `from other import Color` or `import pkg.tone as
Shade` over an earlier import still counts. Limit (vii), D-233 decision 5,
and `docs/DIAGNOSTICS.md` say so; one unit test and one end-to-end test
added. (2) P1: `enum_call.rs` had reached 1,013 lines; its inline test
module is now the sibling `enum_call_tests.rs` (a `#[path]` child module,
so `super::` access is unchanged), leaving the scanner at 488 lines. Round
7 of `.harden/findings/issue-944.jsonl` records both.

## PR #971 sixth Codex round (re-exported enum under two paths)

Codex P2 on `15efde6e`: `from colors import Color` beside `from palette
import Color` (with `palette` re-exporting it) puts `Color` in
`module_rebound_names`, so `Color()` falls to the span-less guard at `1:1`
-- reproduced with `pycc check` (a single re-exported import keeps the
call span). The proposed fix, comparing the resolved
`ImportBinding::Project` origin, has no mechanism behind it:
`bind_project_name` records the module the name was imported *from*
(`palette`), and `copy_class_with_ancestors` clones a re-exported class
with no defining-module tag, so HIR carries no provenance to compare.
Recorded as one more D-233 residual shape the backstop reports (limit
(vii), decision 5, `docs/DIAGNOSTICS.md`); tracking re-export provenance is
a separate feature outside #944. Replied and resolved; round 8 of
`.harden/findings/issue-944.jsonl` records the refutation.

## PR #971 seventh Codex round (diagnostic-count contracts)

Codex P2 on `fb4caa18`: `docs/DIAGNOSTICS.md`'s quality-bar bullet,
`docs/CLI_SPEC.md`'s `check` contract, and `.claude/skills/pycc/SKILL.md`
still stated that HIR lowering reports one diagnostic per failing
top-level item, although the D-233 scan appends one `C0001` per enum-class
call after the item's own diagnostic (an unsupported `with` item holding two
`Color()` calls yields three). Fixed in `8dfe739c`: all three describe the
two per-item sources; the `.agents/skills/pycc` Codex entrypoint is a pointer
to the Claude skill and needed no change. Round 9 of
`.harden/findings/issue-944.jsonl` records it.

## PR #971 eighth Codex round (ordering contracts)

Three P2 doc-drift findings on `80b23ca9`, fixed in `78d8a626`:
`docs/CLI_SPEC.md` no longer promises a release-stable first diagnostic
(it is the completed #864 transition invariant, D-217 rule 2, as
`docs/DIAGNOSTICS.md` already said); its report-order section and
`src/frontend.rs`'s `Frontend` error doc now name the per-item pair (the
item's own diagnostic, then its enum-call `C0001`s); and the D-219 cascade
sentence in `docs/DIAGNOSTICS.md` scopes the silence to the lowering
diagnostic, since the scan still runs over a cascade-silenced item (D-233
decision 6) -- a new unit test pins that report. Round 10 of
`.harden/findings/issue-944.jsonl` records the three.

## PR #971 ninth Codex round (attributable calls)

One P2 on `6c52790c`: the per-item count contract said one enum-call
`C0001` per call unconditionally, while a scan-suppressed name is reported
by the span-less guard at `1:1` instead. `5b12e7b8` qualifies the count to
calls the scan can attribute in `docs/CLI_SPEC.md`, `docs/DIAGNOSTICS.md`,
`src/frontend.rs` and the `pycc` skill; round 11 of the findings pile
records it.

## PR #971 tenth Codex round (future imports, lazy frame, two-path test)

Three P2s on `13e7725b`, fixed in `f674edce`: a `from __future__` import no
longer counts as a rebinding in `module_rebound_names` (it binds nothing),
so `class annotations(Enum)` keeps its call span; the module frame is
built lazily on the first scanned item, against the pre-loop import slice,
so a module without an enum class pays no walk (D-233 decision 1); and the
two-path re-export import residual now has a CLI test at `1:1`. Round 12
of the findings pile records the three.

## PR #971 eleventh Codex round (definition-time bindings)

One P2 on `0e88a674`, fixed in `a093c901`: the scope binder skipped a
nested `def`/`class` whole, so a walrus in a decorator, type parameter,
default value, annotation, return annotation, or class base did not bind
the enclosing frame, and a later module-level `Color()` was a false-kind
enum-call `C0001` outside a class body. The binder now walks those
definition-time expressions into the enclosing frame and still skips the
body; limit (iii) and D-233 record the harmless over-suppression under the
`def`'s own frame. Round 13 of the findings pile records it.

## PR #971 twelfth Codex round (rustdoc contracts)

Two P2s on `95d7aa2a`, fixed in `7cb92306`: `lower_all`'s rustdoc now
counts only the enum calls the scan can attribute, and `lower_module`'s
contract states the pre-#867 byte-identity for the first *lowering*
diagnostic, since D-233 decision 4 lets an earlier item's scan diagnostic
come first. Round 14 of the findings pile records both.
