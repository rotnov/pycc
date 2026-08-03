# 2026-07-31 — v0.2 PR-10: confirmed self-inflicted `frontend-perf-gate` regression (D-109), fixing as Task 14 on this same branch

**Authoritative checkpoint:** [PR #236](https://github.com/rotnov/pycc/pull/236)
(`feat/v0-2-pr10-ty-representation-migration` → `main`) is open, head
`38c34ed` (the origin/main merge resolving the D-103 ID collision), with
one further docs-only commit about to be pushed on top recording this
entry's own findings. Tasks 1-12 (the `Ty` representation migration,
D-103/D-104's ADRs, the `list[int]` end-to-end thin slice, and the
PEP-585 fixture) are complete, individually task-reviewed, and merged
into this branch's own history — none of that work is in question.

**CI run [30608030517](https://github.com/rotnov/pycc/actions/runs/30608030517)
is green on every required check except `frontend-perf-gate` (and
consequently `ci-gate`).** All 4 `native-build-test` legs,
`build-test-coverage`, `cross-compile-build`, and `cross-compile-verify`
passed, including the new `pep_0585_builtin_generics` conformance test on
all 5 Tier-1 targets in both profiles — `docs/PYTHON_STANDARDS.md`'s
PEP-585 row is flipped to `✅` on this real evidence, per D-102's manual-
flip policy. `frontend-perf-gate` failed twice on independent, genuinely
fresh measurements: 4.7008% on the original run, 5.1960% on a full rerun
(not the `--failed`-only rerun, whose "second data point" turned out to
be misread self-test-fixture output interleaved in the same log stream —
caught by reading the raw job log rather than trusting a grep hit).
Both real measurements exceed the 2.00% threshold in the same direction,
which reads as a real, reproducible regression rather than noise.

**Root cause identified, recorded as [D-109](../DECISIONS.md#d-109-pr-10s-frontend-perf-gate-regression-is-real-and-self-inflicted-fix-deferred-to-its-own-follow-up-pr-merge-stays-blocked):**
`benches/check_bench.rs`'s `pycc_check_frontend_fixture` is scalar-only
(no `list[T]` value ever constructed), ruling out the new list codegen.
D-089/D-104's own `Ty` migration (Tasks 2-5) grew `crates/pycc_hir::Ty`
from a flat, 1-byte, `Copy` enum to a recursive, 24-byte, non-`Copy`
enum (`size_of::<Ty>() == 24`, `align_of::<Ty>() == 8`, confirmed with a
throwaway example deleted after use) — a highly plausible, mechanical
cause on a type-checking-heavy hot path that clones many `Ty` values.

**Decision (called `advisor()` before acting): do not fix this inside
Task 13.** The identified remedy — box `Ty::Dict`'s two fields and
`Ty::Tuple`'s `Vec` down to single-pointer payloads — would change a
representation Tasks 2-5 already migrated ~857 call sites against, after
every task review already closed; it needs its own task brief,
implementer, and independent review, not a late unreviewed amendment.
Per D-024, `ci-gate` red blocks merge with no exception for this being
self-inflicted or well-understood. Per this project's own D-095/D-096/
D-101 precedent, a self-inflicted, root-caused regression is not
grounds for relaxing the gate — that precedent only ever relaxed floors
after confirming an *external*, unfixable hardware constraint.

**Correction to this entry's own first draft:** an earlier version of this
paragraph reported the fix as needing "its own follow-up PR" and stopped
to escalate that choice to the user before proceeding. That framing was
mechanically wrong: the regressed `Ty` shape exists only on this branch,
never having reached `main`, so no PR opened against `main` could carry a
fix for it — there was nothing to defer to. The actual review-discipline
concern (no unreviewed representation change slipped in without its own
brief, implementer, and independent review) is satisfied instead by
adding **Task 14** to this same plan and branch, executed through the
identical task-review loop every other PR-10 task already passed
through — not by pausing for a user decision this repository's own
standing autonomous-delivery directive does not call for on a
well-scoped, already-diagnosed technical fix. `docs/DECISIONS.md`'s
D-109 carries the same correction as its own cross-reference note.

**Status: Task 14 in progress** (box `Ty::Dict`/`Ty::Tuple` down to
single-pointer payloads, re-measure `size_of::<Ty>()`, verify the blast
radius — confirmed small by direct grep: only `crates/pycc_hir/src/lib.rs`
and `crates/pycc_codegen/src/lib.rs` reference `Dict`/`Tuple` at all, zero
occurrences in `pycc_types`/`pycc_mir`/`pycc_rt` — then a fresh full CI
rerun to confirm `frontend-perf-gate` closes). PR #236 stays open and
unmerged until Task 14 lands green; Task 13 (pinned `ievo:deep-reviewer`
pass, final merge) resumes once it does. `docs/ROADMAP.md` carries this
same follow-up inline, alongside the pre-existing D-107 leak-only-
refcounting one.
