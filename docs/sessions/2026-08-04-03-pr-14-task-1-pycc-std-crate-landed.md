# 2026-08-04 checkpoint: PR-14 Task 1 (`pycc_std` crate) landed; Tasks 2-7 not started

## Status

Continuing from `docs/sessions/2026-08-04-02-pr-14-adrs-and-plan-committed-implementation-not-started.md`.
One real commit landed this session on `feat/v0-2-pr14-stdlib-imports`
(worktree `/Users/denis/projects/pycc-proto/.worktrees/pr14`):

- `7aeae69` — Task 1: `pycc_std` crate skeleton.

Branch is now 4 commits ahead of `origin/main` (`1016d03`, re-verified
unchanged at the start of this session via `git fetch origin --prune`).

## What was actually done this session

1. Re-verified worktree/branch state and confirmed `origin/main` is still at
   `1016d03` (unchanged since the prior checkpoint).
2. Read the full 7-task implementation plan
   (`docs/superpowers/plans/2026-08-04-v0-2-pr-14-stdlib-imports.md`) and the
   prior session log.
3. Resolved the plan's own flagged open question (Task 4's registry/Task 1's
   registry circular dependency) *before* writing any code, per advisor
   guidance: grepped `crates/pycc_codegen/src/lib.rs` and confirmed (a) an
   extern-call mechanism already exists (`add_function` +
   `Linkage::External`, used for every `pycc_rt` runtime function today) and
   (b) the linker driver already passes `-lm`. This means `math.sqrt` via a
   libm call needs no new codegen mechanism, only a new match arm — de-risking
   Task 4 significantly. Also decided to drop `sys` entirely from this PR
   (no `NoReturn` precedent for `sys.exit`, no `list[str]`-from-argv path for
   `sys.argv`) rather than half-implement it.
4. Implemented Task 1 for real: new workspace crate `pycc_std`
   (`crates/pycc_std/Cargo.toml`, `crates/pycc_std/src/lib.rs`), added to the
   root `Cargo.toml` `members` array ahead of `pycc_types`. Dependency-free
   per D-136 (local `ScalarKind` enum, not `pycc_hir::Ty` — conversion
   happens at the `pycc_types` call site in a later task). Registry contains
   exactly `math.sqrt` (`Function`) and `math.pi` (`Constant`) — narrower
   than D-136 point 2's original symbol list (which also named
   `math.floor`/`math.pow`/`math.e`/`sys.exit`), because those are the only
   two symbols with a concrete lowering slice landing in *this* PR.
5. Recorded the narrowing as an explicit addendum on D-136 in
   `docs/DECISIONS.md` (not a silent deviation from the ADR's point 2 list),
   per the plan's own instruction to record any such narrowing.
6. Verified: `cargo test -p pycc_std` (7 tests, all passing);
   `cargo llvm-cov -p pycc_std --fail-under-lines 100 --fail-under-regions
   100` — **100% lines, 100% regions, 100% functions** on the new crate;
   `cargo build --workspace` — whole workspace still builds clean with the
   new member added.
7. Committed as `7aeae69`.

## What is NOT done

- **Task 2** (HIR `Stmt::Import`/`Stmt::ImportFrom` lowering, `crates/pycc_hir/src/lib.rs`):
  not started. `pycc_hir::lower_stmt` still has no arm for either variant;
  `import math` still falls through to the generic `C0001` catch-all exactly
  as documented at the comment near line 541 in that file.
- **Task 3** (`pycc_types` binding of registry symbols to `Ty`s, including the
  `C0002`-vs-`C0001` diagnostic split D-136's own decision text specifies):
  not started.
- **Task 4** (`pycc_mir`/`pycc_codegen` lowering of `math.sqrt`/`math.pi`):
  not started, though the pre-implementation grep in this session confirmed
  the mechanism it needs (`add_function`/`Linkage::External`, existing `-lm`
  link flag) already exists and does not need to be built from scratch.
- **Task 5** (PEP-594 `cgi` rejection fixture, passing `math`/`sys` — now just
  `math` — fixture, D-139 container/generics corpus fixture): not started.
- **Task 6** (docs sweep: `STDLIB_PLAN.md`, `PYTHON_STANDARDS.md`,
  `DELIVERY_PLAN.md` row 14, `ROADMAP.md` v0.2 bullets, graduating
  D-136–D-139, `DIAGNOSTICS.md`): not started beyond the D-136 addendum note
  already folded into Task 1's commit. In particular, note for the next
  session: flipping any `docs/ROADMAP.md` `[x]` box requires an inline
  `roadmap-evidence` identifier recognized by
  `scripts/check_roadmap_evidence.rb`, and teaching that checker a *new*
  identifier requires a failing public-CLI mutation test added first (see
  `AGENTS.md`'s testing-and-hard-coverage-gate section) — budget for this
  explicitly in Task 6, it is not optional polish.
- **Task 7** (100% workspace coverage re-verification including all new
  Tasks 2-4 code, pinned `ievo:deep-reviewer` pass, PR opened, CI green,
  merge): not started. No PR opened yet.
- Zero HIR/type-checker/MIR/codegen changes exist yet — only the new leaf
  crate and its own tests. `import math` in a `.py` file still produces
  `C0001` exactly as before this session; nothing user-visible has changed.
- v0.2 is **not** shippable as of this checkpoint.

## Why this checkpoint stops here

Task 1 was a genuinely self-contained, independently testable unit (a new
leaf crate with zero existing-crate edits) that could be implemented,
verified to 100% coverage, and committed as one clean, reviewable step.
Tasks 2-4 are materially larger and touch four existing crates each with
their own 100%-region coverage obligation across a compiler-wide gate
(D-014) — HIR statement lowering plus a new side-table threaded through
`HirModule`, type-checker intrinsic dispatch mirroring the existing
`print`/`len` special case, and MIR/codegen lowering to a real libm call.
Attempting to rush all of Tasks 2-7 in the remaining budget of this turn
risked exactly the failure this project's own D-021/AGENTS.md rules warn
against: fabricated or shallow progress that claims more than what was
actually built and verified. One real, coverage-verified, committed task is
preferred over an unverifiable multi-task claim.

## Where a fresh session should resume

1. `cd /Users/denis/projects/pycc-proto/.worktrees/pr14` (branch
   `feat/v0-2-pr14-stdlib-imports`, currently at `7aeae69`). Re-run the D-021
   preflight fast-forward check against `origin/main` first (expected still
   `1016d03`, but re-verify — do not assume).
2. Start Task 2 directly: `crates/pycc_hir/src/lib.rs`, the `Stmt::Import`/
   `Stmt::ImportFrom` arms in `lower_stmt`/`lower_checked`, following the
   `type_aliases: Vec<(String, Ty)>` side-table precedent PR-13 established
   (see `HirModule` around line 430-440 in that file for the exact shape to
   mirror). Remember: the registry is now `pycc_std`'s actual shipped
   `math`-only, `sqrt`/`pi`-only registry from Task 1's commit — do not
   re-derive a broader scope from the plan's original (now-narrowed) text.
   D-136's own decision text (not just the plan) specifies the `C0002`
   diagnostic for "recognized module, unregistered symbol" distinct from
   `C0001` for "unrecognized module/shape" — implement both codes, not just
   `C0001`, in both Task 2 (module-level) and Task 3 (symbol-level).
3. Keep working synchronously task-by-task in one continuous run per the
   `autopilot-async-monitoring` skill: no background/async `Agent`
   dispatches that end the turn to "wait." Commit after each task passes
   local review (tests + `cargo llvm-cov` on the touched crates), so any
   future stopping point stays durable.
