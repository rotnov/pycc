# 2026-07-26 — PR #132 merged: PR-5 (Codegen depth) complete, v0.1 delivery moves to PR-6

**Merged:** [PR #132](https://github.com/rotnov/pycc/pull/132) merged into
`main` as merge commit `03c6472362d1d6d2211b7cf4e7bb132ffe86295f` (parents
`78f5dcc0c3fd7c88fdc87e716e294fb0fc5cdb53` and
`d30e6a6c787de39e7e761d44d44cbf3e6cad3353`, the branch's final head). All
required checks passed on that head (`ci-gate`, `audit`, five native-build-test
legs, cross-compile build/verify, `build-test-coverage` at 100%
lines/regions, `frontend-perf-measure`/`frontend-perf-gate`, `agent-assets`,
`agent-policy`); `mergeStateStatus` was `CLEAN` and no review thread was
unresolved at merge time. `main-history-audit` passed post-merge.

**What this session added on top of the prior entry's state:** this branch
was independently, concurrently worked by two agent lineages pushing
directly to `feat/v0-1-pr5-codegen-depth` (this session's own, and a
`codex/fix-pr132-review-0764`-derived one whose merges landed as
`0f19f22`…`5a9741e` and later `fcd8656`/`50e36e8`) — see
[AGENT_RETROSPECTIVE.md](./AGENT_RETROSPECTIVE.md)'s newest entry for the
process lesson. Rather than fight over authorship, this session's later
pushes adopted the more-complete remote lineage as base each time and
carried forward only genuinely unique value: stale `ARCHITECTURE.md`/
`CLI_SPEC.md` prose, a hardened exact-value `fib(100)` bigint assertion, a
real linker-exercising `**` e2e test, and (after the pinned local reviewer
skill remained uninvokable this session) a substitute 17-agent workflow
review (5 dimensions × adversarial 2-vote verify) over the complete
`main...HEAD` diff. All 6 of its findings were independently reproduced
against the live CLI before fixing: `return helper()` inside a `-> None`
function built invalid `ret i8 0` IR (fixed to a clean void return,
mirroring `print()`'s own `None`-handling); a `bool` widened to tagged
`int` at any boundary permanently loses its identity so `print`/`str`
renders CPython's `"True"`/`"False"` as `"1"`/`"0"` (documented as an
accepted gap in `ROADMAP.md`, not architecturally reworked under merge
pressure — see that entry's own reasoning); plus two cheap test-rigor
additions (an executed, not just compiled, `>=`/`!=` check, and a
hand-built-MIR NaN test for the deliberate `FloatPredicate::UNE` choice).
Missing long-form `docs/DECISIONS.md` sections for six PR-5 decisions
(D-057–060, D-070, D-071) and for D-001/D-007 were deferred as a follow-up
rather than backfilled under time pressure — see Known follow-ups below.
Two more live Codex findings landed on the concurrent lineage's own later
commits and were fixed the same way: `pycc run`'s exit-code mapping
(`status.code().unwrap_or(1)`) silently returned `1` instead of the
`101` CLI_SPEC.md promises when the compiled program aborted via `SIGABRT`
crossing a plain `extern "C"` boundary (fixed to `unwrap_or(101)`, verified
live against `print(1.0 / 0.0)`); a `None`-typed parameter type-checks but
has no codegen ABI representation (already an honest panic, now also
listed in `ROADMAP.md`'s known-gaps).

**Local evidence (final head `d30e6a6`, prior to the merge commit):** the
exact hard command
`cargo llvm-cov --workspace --fail-under-lines 100 --fail-under-regions 100`
passed at 100.00% across all metrics (16,207 regions, 11,626 lines, 781
functions). Full workspace build, test suite, `cargo clippy --workspace
--all-targets -- -D warnings`, and `cargo doc --workspace --no-deps` were
all clean, along with the roadmap-evidence, ci-permissions, agent-assets,
and agent-policies checkers.

**Known follow-ups (not blocking, tracked here rather than in an issue
yet):** (1) missing long-form `docs/DECISIONS.md` sections for D-057,
D-058, D-059, D-060, D-070, D-071, and for D-001/D-007's own graduation to
`accepted` — pure docs-completeness, safer to write carefully in a
dedicated pass than to backfill six-plus historical rationales under merge
pressure; (2) the documented `bool`-identity and `None`-parameter gaps
above are real v0.1 scope boundaries, not defects, but would need a
representation-level design (a runtime type tag, or a real `None` ABI
representation) to close for a future milestone.

**Next:** PR-6 per `docs/DELIVERY_PLAN.md` row 6 — `pycc_testkit`
(fib + mandelbrot-ascii vs. pinned CPython 3.14.6 on all 5 Tier-1 targets,
`--debug` profile), the `pycc check` <50ms/1k LOC benchmark, and byte-for-byte
CLI_SPEC.md diagnostic-output conformance. `docs/DELIVERY_PLAN.md` itself
notes the CPython oracle needs upgrading from the currently-pinned 3.14.3
to the 3.14.6 patch target before this PR starts.
