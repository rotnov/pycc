# Session checkpoint: issue #385 — class model core (Part 1 of #375)

**Date:** 2026-08-06
**Status:** Implementation + 2 D-068 review rounds complete, all gates green, opening PR next.

## What happened

First real code PR of v0.3, following #374's decomposition. #375 (originally filed as PR-15,
"class model foundation") itself violated AGENTS.md's decomposition rule — bundled a `Ty`/HIR
representation change, a distinct class-body execution-order binding-scheme design problem, and
three additive PEP surfaces into one issue. Caught by `advisor` before implementation started;
split into #385 (Part 1, this issue), #386 (Part 2, execution order), #387 (Part 3, PEP surface).
#375 stays open as tracking parent.

**Plan** (dispatched agent running `issue-to-plan`, 3 review rounds): published at
https://github.com/rotnov/pycc/issues/385#issuecomment-5204594128. Key decisions: new `class.rs`
submodule per touched crate (mirroring the `stmt.rs`/`expr.rs` precedent from #141/#361); `Ty`
size verified against the existing D-109 `size_of::<Ty>()` tests as the primary local gate;
opaque-struct-with-FFI-accessors instance layout (matching `PyIntListObj`/`PyDictObj`'s existing
convention); confirmed PR #358's red CI is its own defect, not inherited.

**Implementation** (dispatched agent): `Ty::Instance(Box<String>)`, new
`crates/pycc_hir/src/class.rs` (HIR lowering), `crates/pycc_types/src/class.rs` (type resolution),
`crates/pycc_rt/src/instance.rs` (opaque runtime object + FFI accessors), new ADR
[D-154](../decisions/D-154-class-instance-runtime-layout-stays-opaque.md) (fresh-verified against
`docs/decisions/` and every open PR's live diff immediately before commit — no collision). All
gates green: `cargo test --workspace` (1745 passed), `cargo llvm-cov` genuinely 100% lines/regions,
clippy clean, `cargo doc` clean.

**D-068 review, round 1** (this session's own independent dispatch, not the implementer's internal
pre-check): 0 blockers, 2 warnings, 3 notes.
- Warning: a class method literally named `append`/`pop`/`get`/`add` was silently misrouted to the
  compiler's built-in container-method fast path (`crates/pycc_hir/src/expr.rs`), producing a
  confusing `C0001` naming the wrong type instead of resolving to the user's own method.
- Warning: a class name colliding with an existing top-level function/alias/import name produced
  no diagnostic and silently, permanently shadowed the function at every call site — the
  duplicate-name check only compared against other classes, and an inline comment in
  `pycc_types/src/lib.rs` asserted an invariant ("a class name can never collide with a function
  name") that nothing in the code actually enforced.
- 3 notes: doc drift (TYPE_SYSTEM.md's `__init__` pre-scan scope, a stale test-name
  cross-reference, an unreachable `T0044` explain-example using an out-of-scope class-typed
  annotation).

**Fix round** (implementer's own dispatched agent, resumed with full context): fixed all 5.
Finding 1: `CONTAINER_METHOD_NAMES` rejection at `lower_class` time with a clear `C0001`. Finding
2: extended the duplicate-name check to be fully bidirectional across class/function/alias/import
— caught mid-fix by the implementer's own internal D-068 re-review that the first attempt only
handled two of four direction combinations (`type Foo = int`/`import Foo` written *after*
`class Foo:` still slipped through), fixed completely on the second pass. Findings 3-5: doc/comment
fixes. All 5 regression tests independently confirmed to fail without their fix. Re-ran full gate
set: all green, still genuinely 100% coverage. Own internal D-068 round 2: 0 blockers, 0 warnings,
2 notes (both deliberately deferred: alias-vs-alias/import-vs-import self-collision — an
independent, narrower gap; a `CONTAINER_METHOD_NAMES`/`expr.rs` list-duplication maintenance
note).

## Current state

- Worktree: `.claude/worktrees/issue-385-class-model-core`, branch `task/issue-385-class-model-core`,
  5 commits on top of `origin/main @ a7c4cb1` (unchanged since preflight, re-verified immediately
  before this checkpoint).
- Issue #385 confirmed still open, no objecting comments.
- 17 files changed, +5601/-414. Zero actionable findings remain.

## Resume point

If resumed fresh: push the branch, open the PR (`Fixes #385`), monitor CI, merge. Then continue
`issue-select`'s loop — #386 (Part 2, class-body execution order) and #387 (Part 3, PEP surface)
both hard-depend on #385 and become selectable once this merges; #386 is the more central of the
two (blocks nothing else directly, but the pattern-reuse-not-mechanism design work it does is
architecturally load-bearing), while #387 is purely additive PEP polish — score accordingly rather
than assuming order.
