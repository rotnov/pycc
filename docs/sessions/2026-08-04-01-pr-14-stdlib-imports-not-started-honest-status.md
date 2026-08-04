# 2026-08-04 checkpoint: PR-14 (stdlib imports, v0.2 final PR) — orientation done, implementation not started

## Status

PR-14 ("pycc_std crate + math/sys stdlib-intrinsic imports + PEP-594 dead-battery
fixture + hand-authored container/generics corpus") is **not implemented**. This
entry exists so a following session does not have to re-derive orientation from
scratch, and so nobody mistakes silence for progress.

## What was actually done this session

1. D-021 preflight: fetched `origin`, confirmed `origin/main` tip is
   `1016d03a6c612fe202c3c5fe0902390c21d0a9fd` ("v0.2 PR-13: PEP 695 generic
   functions, type statement, legacy TypeAlias (#321)"), the same commit
   recorded as PR-13's merge in `docs/DELIVERY_PLAN.md`.
2. Created a fresh worktree/branch from that exact tip:
   `/Users/denis/projects/pycc-proto/.worktrees/pr14`,
   branch `feat/v0-2-pr14-stdlib-imports`, tracking `origin/main`. No commits
   on it yet — it is at `origin/main`'s tip, clean.
3. Verified the current highest ADR/diagnostic numbers fresh against this tip:
   highest `D-` entry in `docs/DECISIONS.md` is **D-135**, highest `T00xx`
   diagnostic code in the tree is **T0042**. A new PR-14 ADR should start at
   **D-136**; no code has been produced yet, so nothing has actually claimed
   D-136 or a new T-code.
4. Re-read D-088 (`docs/DECISIONS.md`) in full — it is the governing decision
   for this PR's scope, already accepted:
   - Replaces the unreachable "corpus Tier-1 (tomli/packaging/more-itertools)
     compiles" criterion with a **hand-authored container/generics differential
     corpus** (list/dict/set/tuple literals, comprehensions, slicing, methods,
     generic functions), diffed against pinned CPython 3.14.6 on all 5 Tier-1
     targets, same pattern as D-085's v0.1 corpus.
   - Scopes stdlib imports narrowly: `import math`/`import sys` and
     `from math import ...`/`from sys import ...` resolve as compile-time
     intrinsic bindings against `pycc_std`'s own registry (same category as
     the existing special-cased `print` handling in `infer_expr_in`), **not**
     general filesystem/package resolution. Everything else (relative imports,
     multi-file user modules, PEP 420 namespace packages) stays out of scope,
     unchanged v0.4 boundary.
   - PEP 594 (dead-battery removals) becomes meaningfully testable only once
     real stdlib-module resolution exists to distinguish "this module was
     removed" from "imports aren't supported yet" — that is exactly this PR's
     job; the fixture should import something real but genuinely absent from
     `pycc_std`'s registry (e.g. `cgi`) and assert a distinct, clean rejection
     from the current blanket `C0001`.
   - Conformance target is ≥15 rows green (not ≥25), backed by an itemized
     16-row list in D-088 itself.
5. Confirmed empirically (not assumed) that import statements are currently
   unhandled all the way back at the HIR boundary, not merely at type-check
   time: `crates/pycc_hir/src/lib.rs:541` documents "`pycc_hir` has no
   `Stmt::Import`/`Stmt::ImportFrom`" handling at all today. `pycc_ast` already
   carries `Stmt::Import`/`Stmt::ImportFrom` AST variants (inherited from the
   vendored general-purpose parser), but nothing downstream consumes them —
   confirming D-088's own claim that today's behavior is a clean, generic
   `C0001` rejection, not a partial implementation.
6. Located the precedent this PR's dispatch mechanism should follow: `print`'s
   existing special-cased handling in `infer_expr_in` (`pycc_types`) is the
   established "hand-recognized intrinsic, not general dispatch" pattern this
   whole PR-9..PR-13 series has used; `pycc_std`'s `math`/`sys` registry should
   extend that same mechanism rather than build a new one.

## What is NOT done (the actual remaining scope)

This is the real size of what's left, so it isn't underestimated by a future
session:

- No `pycc_std` crate exists yet (workspace member, `Cargo.toml`, module
  registry data structure for `math`/`sys` symbols).
- No HIR lowering for `Stmt::Import`/`Stmt::ImportFrom` at all (today: no
  variant, not even a rejected one — needs a deliberate new HIR node or an
  explicit clean-rejection path per D-088's "everything outside the registry
  must still cleanly reject, not silently no-op" requirement).
- No type-check-time binding of `math.sqrt`/`sys.argv`-shaped names to
  `pycc_std`'s registry in `pycc_types`.
- No codegen path for at least one concrete intrinsic call reaching real
  generated code (this series' established "thin slice: one concrete case to
  real codegen, clean diagnostics for the rest" pattern — D-105 through D-135
  all did this; PR-14 has not yet picked and implemented its one case).
- No PEP-594 fixture, no hand-authored container/generics corpus per D-088
  item 1.
- No `docs/superpowers/plans/2026-08-04-v0-2-pr-14-stdlib-imports.md` plan
  file has been written yet (the `writing-plans` skill was not run this
  session).
- No new ADR has been added (the design forks in the task prompt — exact
  `math`/`sys` symbol subset, the HIR node shape, the PEP-594 fixture's exact
  target module, the corpus's exact file layout — are still open judgment
  calls for whoever picks this up).
- Zero tests, zero coverage, no PR opened, nothing merged.
- `docs/ROADMAP.md`'s v0.2 acceptance bullets are therefore **not** all green
  yet; v0.2 is **not** shippable as of this checkpoint. Do not report v0.2 as
  complete until PR-14 above actually lands.

## Why this checkpoint stops here instead of continuing

Earlier in this same session, dispatching PR-14 as a single long-running
background agent produced no real work (the agent's own worktree returned
zero commits and an empty plan directory when checked directly, and the
agent had already exited after one message — background dispatch does not
reliably keep making progress while the parent session is not actively
polling it, matching this project's own documented PR-13 lesson about that
pattern). This checkpoint intentionally favors an honest, verifiable "not
done yet, here is exactly where things stand" over either continuing to
trust an unverified background agent or fabricating a completed PR.

## Where a fresh session should resume

1. `cd /Users/denis/projects/pycc-proto/.worktrees/pr14` (branch
   `feat/v0-2-pr14-stdlib-imports`, currently at `origin/main`'s tip,
   clean) — reuse this worktree/branch rather than creating another one,
   unless `origin/main` has moved, in which case re-run the D-021 preflight
   fast-forward check first.
2. Run `superpowers:writing-plans` against D-088's scope (section above) to
   produce the zero-placeholder file-level plan at
   `docs/superpowers/plans/YYYY-MM-DD-v0-2-pr-14-stdlib-imports.md`, using
   `print`'s existing `infer_expr_in` special-case as the concrete model for
   `pycc_std` dispatch, and D-089's boxed recursive `Ty` (already merged) for
   any container types the corpus needs.
3. Execute the plan with `superpowers:subagent-driven-development`,
   dispatching each implementer task **synchronously in the driving
   session's own turn** (wait for and read the actual tool result before
   ending the turn) rather than as a detached background agent — this is the
   specific failure mode this checkpoint hit twice in one session.
4. New ADR(s) start at **D-136**; verify that number is still unclaimed
   against the then-current `origin/main` before using it, since this repo
   has repeatedly collided on ADR/diagnostic numbering across concurrent
   agent sessions.
