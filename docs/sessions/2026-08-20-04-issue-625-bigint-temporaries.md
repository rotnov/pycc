# 2026-08-20-04 — issue #625: release a heap bigint's birth reference

## Overall status

Issue #625 ("Part 2" of the D-180 heap-bigint refcounting work) is
implemented and committed on the task branch `claude/issue-625` as a
single commit, `00b5100d`, on top of `origin/main` = `c43e5558`. No pull
request has been opened; the orchestrating session opens it after review.
At the time of writing there are no other open pull requests on the
repository.

## What landed in `00b5100d`

D-180 (#624) refcounted `BigIntObj` and released heap bigints at named
storage slots but left the birth reference of every freshly built
arithmetic temporary unbalanced. D-181 closes that: a `Ty::Int`
expression is treated as *owned* — its birth reference is retired after
the consumer has read it — unless it is one of three *borrowed* shapes
(`MirExpr::Name`, `MirExpr::AttrGet`, or a `MirExpr::Subscript` whose
base is a tuple).

- `crates/pycc_codegen/src/bigint_rc.rs` (new) — the guarded
  retain/release emitter, both ownership classifications, and the
  helpers that apply them. This is the AGENTS.md decomposability carve
  out of `lib.rs`, scoped to exactly the unit #625 touches.
- `crates/pycc_codegen/src/lib.rs` — module wiring, `Scalar` made
  `Copy`, and the release sites: integer `BinOp` and `Compare` operands,
  `ExprStmt`, `if`/`while` conditions, `print` arguments, f-string
  interpolations, the three comprehension `if`-filter conditions, and
  the `ForRange` / comprehension-`range` bounds. Every release is
  emitted *after* the consuming call, because consumers such as `truthy`
  and `to_str` read the bigint's limbs.
- `crates/pycc_rt/src/lib.rs` — documents and tests the no-aliasing
  invariant the release sites depend on: no `int` operation may return an
  operand's own encoded word.
- `tests/issue_146_bigint_release.rs` — 10 behavioural fixtures plus 3
  peak-RSS ratio gates (nested, aliased, and literal operands).
- `docs/decisions/D-181-*.md`, `docs/decisions/README.md`,
  `docs/RUNTIME.md`, `docs/ROADMAP.md` — the decision record, the
  regenerated index, and the narrowed residual-leak list.

This change also fixes issue #633 Direction A: a bigint read out of a
tuple used to be freed by overwriting the reading local, because the read
never retained. Verified empirically against the pre-change compiler.

## Deviation from the published plan

The plan called for widening `CompLoopTail::Range` to carry `stop_v`.
Instead the `match source` tuple in all three comprehension emitters
gained a fourth element, `owned_range_operands: Vec<IntValue<'ctx>>`.
This is equivalent, less invasive, and keeps a compile-time ownership
decision out of a runtime tail enum.

## Known follow-ups (deliberately out of scope)

- Issue #633 **Direction B** — the owner overwritten *before* the read —
  remains an unfixed use-after-free, documented in D-181's consequences.
  Do not close #633 on the strength of this change alone.
- Two residual leaks remain, both enumerated in D-181 and
  `docs/RUNTIME.md`: a fresh `int` flowing into a `TupleLiteral` element
  (D-141 container ingress), and an operand skipped by a D-173
  `guard_statement_effects` exception edge.
- `require_inline_int` aborts on bigint operands, so `int_mul`,
  `int_floordiv`, `int_floormod`, `int_pow` and `int_cmp` are not
  behaviourally testable with bigints. In-crate IR-observer tests stand
  in for those paths.

## Gate status on this host

Green locally: clippy, the frontend-throughput gate, `check_ci_permissions`,
`check_roadmap_evidence` and its own test, the decisions-index `--check`,
the `scripts/` unittest suite, `validate_agent_policies.py`, and
`check_conformance_breadth.py`.

Two gates cannot be judged on this host and must be judged by CI:

- `cargo test --workspace -- --include-ignored` fails on this machine
  because the pinned conformance/nbody oracle must be exactly CPython
  3.14.7 and only 3.14.6 is installed (3.14.7 is not obtainable through
  `uv` here). The identical two targets — `conformance` and
  `nbody_bench` — fail the same way on the unmodified base commit, so the
  failure is environmental, not introduced by this change.
- The nbody runtime-speedup gate depends on the same missing oracle and
  was therefore not run at all rather than run in a weakened form.

## Where a fresh session should resume

`git log -1 claude/issue-625` for the change itself; the published plan
at `https://github.com/rotnov/pycc/issues/625#issuecomment-5351408402`
for the work-item breakdown; `docs/decisions/D-181-*.md` for why each
site is classified the way it is.
