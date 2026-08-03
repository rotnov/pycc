# 2026-07-26 — PR #132 final-head performance-gate repair validated locally

**Remote evidence:** [PR #132](https://github.com/rotnov/pycc/pull/132) head
`5a9741e1b6761c58eefb7a85e1f7906a4dbdea19` passed workflow policy, agent
policy/assets, 100% coverage, Pages, all five native/cross target legs, and the
replicated measurement job. The predecessor-owned isolated gate then correctly
blocked the changed-input classification: predecessor replicate medians
`6201.39, 5973.44, 6078.99, 6044.88, 6399.96 ns` (aggregate `6078.99 ns`)
versus candidate `6179.26, 6119.92, 6209.22, 6263.44, 6250.65 ns` (aggregate
`6209.22 ns`), or `+2.1423%` against the unchanged hard `2.00%` threshold.
The run was not retried or waived. All fourteen prior review threads remain
resolved. The exact `@codex review` request for that head completed with two
new actionable findings: exceptional `float` powers could silently return
infinity/NaN, and the roadmap omitted reachable arithmetic failure boundaries.

**Local repair:** the measured parse/lower/check path had no PR-local parser,
HIR, types, or benchmark-fixture change, but the complete `src/`/`crates/`
classifier deliberately treated the backend diff as changed executable input.
Rather than weakening or re-running the gate, `pycc_types::check` now builds the
concrete function environment directly instead of creating and then cloning a
temporary signature table. Call validation retains the existing behavior of
inferring every argument before arity/type diagnostics while storing up to four
types inline and allocating only for wider calls. Focused regressions cover the
wide-call fallback, its error path, and diagnostic-order preservation.

The review repair now rejects zero-to-negative, negative-base/fractional, and
finite-overflow `float` powers explicitly until Python exceptions and complex
results exist. Floor-dividing the minimum tagged integer by `-1` promotes the
out-of-range quotient to bigint instead of aborting. Runtime and slice-level
regressions cover the successful promotion plus each reachable `float`-power
failure class and the supported non-finite real-result domains; `RUNTIME.md`
and the commit-relative roadmap enumerate the
remaining bigint-to-float, bigint-operand arithmetic, and negative-`int`-power
boundaries.

**Local evidence:** the same-tree Criterion baseline comparison moved the point
estimate from `6.0009 µs` to `5.7464 µs`; Criterion estimated `-3.2494%` with
`p = 0.00` and reported an improvement. The exact hard command
`cargo llvm-cov --workspace --fail-under-lines 100 --fail-under-regions 100`
passes with 16,038/16,038 regions and 11,501/11,501 lines. The coverage run
includes 159 `pycc_types` tests, 62 `pycc_rt` tests, and 28 slice-level tests.
Clippy with `-D warnings`, fresh workspace Rust documentation, formatting,
roadmap-evidence validation (99 runs / 432 assertions plus the production
checker), and diff checks are green after the final review repair.

**Review evidence at handoff:** independent iEvo reviews found that the initial
finite-domain guards over-rejected non-finite float-power operands and that the
diagnostic-order regression exercised an internal helper but not the changed
public `check` path. Both repairs are staged with focused assertions. The exact
final staged diff is reviewed again after this entry; the pull request and
commit history remain the authoritative outcome.

**Concurrent-head evidence:** while the reviewed repair was staged, the remote
PR head advanced through `f0226542e7601b3a82883ae82c74e45ed5fa3549`. The
reviewed local repair was first preserved as
`fcd865613c46b70bb7cbaf4b72b11929e897d5fb`, then the remote head was merged.
The resolution retains its exact-fibonacci oracle, Linux/libm end-to-end test,
and refreshed architecture/CLI text while preserving the independently
reviewed finite-only float-power guards and the now-implemented floor-division
promotion in the roadmap.

**Required next steps:** commit and push the reviewed repair, then request exact
`@codex review` once for that commit. Re-run every required gate and merge only
if the new fixed five-replicate performance result, coverage, aggregate
`ci-gate`, and review state are all green.
