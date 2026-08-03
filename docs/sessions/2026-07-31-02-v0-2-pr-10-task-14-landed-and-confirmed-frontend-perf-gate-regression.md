# 2026-07-31 — v0.2 PR-10: Task 14 landed and confirmed; `frontend-perf-gate` regression (D-109) resolved

**Authoritative checkpoint:** same branch/PR as the entry directly below
(`feat/v0-2-pr10-ty-representation-migration` → [PR #236](https://github.com/rotnov/pycc/pull/236)).
Commit `a6d35c8` lands Task 14 on top of the plan commit (`2a8879a`) the
entry below left as the checkpoint. Since then: `2123ec6` (Task 14's own
pinned-review follow-up), `c276262` (an independent task review's Dict
doc-comment fix), `1420d91` (the D-109 CI-confirmation record), and
`f4b01c6` (a merge of `origin/main`, which had advanced with an unrelated
merged PR #238 in the meantime — auto-merged cleanly, no conflicts).

**Two attempts, one dead end recorded, not silently discarded:** the
plan's own first-drafted shape, `Ty::Tuple(Box<[Ty]>)` (a boxed slice),
was executed exactly as planned and measured `size_of::<Ty>() == 24` —
no reduction at all from the pre-fix size, confirmed independently
in-crate and via a standalone `rustc` reproduction. Per the task's own
explicit contingency for this outcome, that attempt was reverted in full
(nothing committed) and reported BLOCKED. A corrected shape,
`Ty::Tuple(Box<Vec<Ty>>)` (a second pointer indirection instead of a fat
pointer), measured `size_of::<Ty>() == 16` (`align_of::<Ty>()` unchanged
at 8) — a real reduction — and is what actually landed. `Ty::Dict` is
`Dict(Box<(Ty, Ty)>)` as originally planned; it needed only the one
attempt. `docs/DECISIONS.md`'s D-109 entry, this plan's own Task 14
section, and `docs/ROADMAP.md`'s Task 14 follow-up paragraph all carry
this correction, including an explicit caution that the niche-filling
"why" is a plausible hypothesis, not an independently re-derived proof
(the pre-fix shape's own 24-byte measurement doesn't fit a naive
"tag plus largest payload" rule either, so a future session shouldn't
cite that mechanism as settled fact without checking rustc's actual
layout algorithm).

An independent task review (a fresh subagent, not the implementer's own
self-dispatched pass) then found one further Important finding — the
`Ty::Dict` doc comment overclaimed that boxing `Dict` itself "closed" the
regression and mis-attributed the original 24-byte size to `Dict`'s
shape, when the brief's own analysis already established `Tuple(Vec<Ty>)`
(24 bytes) was the actual size-dominating variant, not `Dict`'s two boxes
(16 bytes) — fixed directly in commit `c276262` (a one-line reword).

**Status: confirmed resolved.** Two independent, genuinely fresh full CI
reruns of commit `c276262` ([run 30613065177](https://github.com/rotnov/pycc/actions/runs/30613065177))
both passed `frontend-perf-gate` — 1.8430% and then **-0.4454%** (current
measurement slightly *faster* than previous), both comfortably under the
2.00% threshold, each with its own fresh `frontend-perf-measure`
timestamps and distinct replicate medians (ruling out the cached-artifact
false-positive this same investigation hit earlier with a `--failed`-only
rerun). `docs/DECISIONS.md`'s D-109 entry and `docs/ROADMAP.md`'s Task 14
follow-up both carry the confirmation numbers. PR #236 is no longer
merge-blocked by D-109.

`origin/main` advanced (PR #238, `version --verbose`) while this was in
flight; merged cleanly (`f4b01c6`, `docs/ROADMAP.md` auto-merged, no
conflicts) to satisfy branch protection's strict up-to-date requirement.

The pinned `ievo:deep-reviewer` (D-068) then ran against the full
`merge-base(origin/main)..HEAD` diff (44 commits, ~8000 insertions) —
this plan's own single highest-risk area (non-exhaustive `Ty`/`Scalar`
dispatch in `pycc_codegen`) came back clean. 8 findings total, all
doc-currency/cross-file-consistency defects at task boundaries plus one
real code asymmetry: `.append(True)` was rejected while `xs[True]` (this
same branch's own earlier fix) was accepted, for the identical D-086
bool-is-int-subtype reason — confirmed via a real build, fixed to use
`is_assignable` for symmetry. All fixed directly: `TESTING.md`/
`DELIVERY_PLAN.md`'s stale "not yet pushed for CI"/"pending CI" wording
(the PEP-585 row has in fact been flipped since Task 13), `SPEC.md`'s
DECISIONS.md range (stopped at D-108, missing this PR's own D-109), the
HIR `ListAppend` doc comment (claimed `pycc_types` rejects value-position
`.append()` — nothing does; it surfaces as a D-072-shaped codegen panic
instead), two stale `pycc_rt` doc comments (referenced the private
`untag_smallint` instead of the actual public boundary helper
`pycc_rt_int_untag_checked`, and described a bigint-corruption gap that
helper already closes), and `DIAGNOSTICS.md`'s `T0033` row (didn't
mention the `len()`-arity failure shape). One `note` deliberately left
open: `PYTHON_STANDARDS.md`'s other nine `✅` rows still carry stale
`pyXX/` fixture-path prefixes this PR's own new row doesn't — pre-existing
drift, not introduced here; the reviewer itself called deferring it
defensible.

**Status: ready to merge.** All required checks green, all review threads
resolved, pinned review clean. What a fresh session should pick up next
if this session ends before merging: confirm CI is green on the latest
pushed commit, then squash-merge PR #236 per this project's established
convention (matching PR-6 through PR-9's own precedent).
