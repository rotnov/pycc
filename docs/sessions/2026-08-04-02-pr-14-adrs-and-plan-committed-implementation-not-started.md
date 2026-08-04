# 2026-08-04 checkpoint: PR-14 ADRs D-136–D-139 and implementation plan committed; implementation not started

## Status

Continuing from `docs/sessions/2026-08-04-01-pr-14-stdlib-imports-not-started-honest-status.md`.
Two real commits landed on `feat/v0-2-pr14-stdlib-imports` since that checkpoint. Task
implementation (crate code, tests, PR) has **not** started yet.

## What was actually done this session

1. Re-verified the worktree/branch from the prior checkpoint:
   `/Users/denis/projects/pycc-proto/.worktrees/pr14`, branch
   `feat/v0-2-pr14-stdlib-imports`. Discovered it was already 2 commits ahead
   of `origin/main` (`1016d03`, unchanged tip) from ADR work already done in
   this same branch before this turn started — not redone.
2. Confirmed commit `8cac479` ("PR-14 ADRs D-136..D-139: pycc_std shape,
   import binding, PEP-594 fixture, container corpus") already exists on the
   branch, adding four accepted ADRs to `docs/DECISIONS.md`:
   - **D-136**: `pycc_std` is a plain data crate; `math`/`sys` symbols are
     hand-recognized inside `pycc_types::infer_expr_in`, reusing the
     `print`/`len` pattern.
   - **D-137**: stdlib imports bind module-qualified names via a per-module
     HIR import table; every other import form stays a clean `C0001`.
   - **D-138**: the PEP-594 conformance fixture imports `cgi` and asserts its
     unchanged `C0001` rejection, contrasted with a passing `math`/`sys`
     fixture.
   - **D-139**: the container/generics differential corpus is one
     hand-authored, multi-feature fixture file, oracle-diffed like every
     other conformance fixture.
3. Wrote and committed (`385d9c3`)
   `docs/superpowers/plans/2026-08-04-v0-2-pr-14-stdlib-imports.md`: a
   7-task, zero-placeholder implementation plan matching PR-10 through
   PR-13's plan granularity, covering: `pycc_std` crate skeleton and
   registry; HIR `Stmt::Import`/`Stmt::ImportFrom` lowering bound to that
   registry with `C0001` as the sole rejection path for everything else;
   `pycc_types` binding of registry symbols to real `Ty`s (mirroring the
   `print`/`len` intrinsic pattern already in
   `crates/pycc_types/src/lib.rs`); `pycc_mir`/`pycc_codegen` lowering for
   one concrete thin slice (`math.sqrt` via libm, `math.pi` as a constant);
   PEP-594 + container/generics corpus fixtures; the full docs sweep; review
   and merge.

## What is NOT done (unchanged in kind from the prior checkpoint, now more precisely scoped)

- No `pycc_std` crate exists yet — not added to the workspace `Cargo.toml`
  members, no source file written.
- No HIR lowering for `Stmt::Import`/`Stmt::ImportFrom` — still absent
  exactly as documented at `crates/pycc_hir/src/lib.rs:541`.
- No `pycc_types` binding of stdlib symbols to `Ty`.
- No `pycc_mir`/`pycc_codegen` lowering, no libm linkage decision made.
- No PEP-594 fixture, no `math`/`sys` fixture, no D-139 container/generics
  corpus fixture.
- Zero new tests, zero coverage measured for this PR's scope, no PR opened,
  nothing merged.
- `docs/DELIVERY_PLAN.md` row 14, `docs/ROADMAP.md`'s v0.2 acceptance
  bullets, `docs/PYTHON_STANDARDS.md`, `docs/STDLIB_PLAN.md`,
  `docs/DIAGNOSTICS.md` are all still unchanged by this PR's actual work —
  only the plan file describes what they will need.
- v0.2 is **not** shippable as of this checkpoint. Do not report v0.2 as
  complete until PR-14's 7 tasks above actually land, are tested, and merge.

## Why this checkpoint stops here

The remaining scope (Tasks 1–7 of the committed plan) is a genuinely large,
multi-crate engineering effort — a new workspace crate, HIR/type-checker/MIR/
codegen changes across four existing crates, new conformance fixtures, a
100%-line/region coverage gate (D-014, no exemptions expected) across all of
it, a full documentation sweep, and a real PR driven to green CI — that does
not fit in this turn's remaining budget. Rather than fabricate partial
progress or claim tasks are done without real commits and passing tests
behind them, this checkpoint stops after landing the two real, verifiable
commits above (`8cac479`, `385d9c3`) and records the plan precisely enough
that a following session can execute Task 1 immediately without re-deriving
scope.

## Where a fresh session should resume

1. `cd /Users/denis/projects/pycc-proto/.worktrees/pr14` (branch
   `feat/v0-2-pr14-stdlib-imports`) — reuse this worktree/branch. Re-run the
   D-021 preflight fast-forward check against `origin/main` first, since
   this repo has repeated history of concurrent agent sessions advancing
   `origin/main`.
2. Read `docs/superpowers/plans/2026-08-04-v0-2-pr-14-stdlib-imports.md` in
   full, then execute it with `superpowers:subagent-driven-development`,
   Task 1 through Task 7 in order, dispatching each implementer
   **synchronously in the driving session's own turn** — this is the
   specific failure mode the prior checkpoint documented hitting twice.
3. Task 1 (`pycc_std` crate skeleton) has no dependency on anything else in
   the plan and is the correct starting point.
4. New diagnostic code (if Task 3 needs one) starts at **T0043**; re-verify
   that is still unclaimed before using it.
