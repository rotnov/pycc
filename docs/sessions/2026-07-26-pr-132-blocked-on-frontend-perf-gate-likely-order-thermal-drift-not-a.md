# 2026-07-26 — PR #132 blocked on `frontend-perf-gate`; likely order/thermal drift, not a real regression; escalated to the user

**Snapshot evidence:** head `1ae1b3c` (fifth merge round). CI run
[30205958740](https://github.com/rotnov/pycc/actions/runs/30205958740):
every job passed except `frontend-perf-gate`, which failed with
`FAIL: pycc check replicated frontend median regressed 6.1931% (threshold: 2.00%)` —
previous replicate medians `6686.28, 6777.54, 7088.44, 7228.73, 7185.40 ns`,
current replicate medians `8498.72, 7405.25, 7527.44, 8023.83, 7353.21 ns`.
(A first attempt on this same head also failed
`frontend-perf-measure` on a plain DNS lookup failure fetching the Rust
toolchain — pure infra, correctly retried via `gh run rerun --failed`,
unrelated to the finding below.)

**Why this looks like the gate's own known order/thermal-drift gap, not
a PR-5 regression:**
1. `git diff 45545bb...HEAD -- crates/pycc_parser crates/pycc_hir crates/pycc_types benches/ Cargo.toml Cargo.lock rust-toolchain.toml`
   is empty, and neither `pycc_hir` nor `pycc_types`'s `Cargo.toml`
   depends on `pycc_mir`/`pycc_codegen`/`pycc_rt`. The exact code path
   this benchmark measures (`pycc_parser::parse` → `pycc_hir::lower_checked`
   → `pycc_types::check` over a fixed fib/print fixture, all in-process,
   no CLI subprocess spawn) is byte-for-byte identical to the predecessor.
   A real algorithmic regression in the measured path is not possible.
2. The two five-value sets show complete separation: every one of the 5
   current replicates (min `7353.21`) is slower than every one of the 5
   previous replicates (max `7228.73`). Under random per-round noise that
   has roughly a 1-in-252 (~0.4%) chance; a full separation like this
   points to a systematic effect, not noise scattered around a stable
   mean. The measurement order is fixed (all 5 predecessor rounds run
   first, then all 5 candidate rounds) — D-062's own text still runs
   sequentially, and D-056's own text already names "order and thermal
   drift inside one hosted runner" as a gap neither D-051 nor D-062
   removes (interleaving was explicitly rejected on trust-boundary
   grounds: candidate code must not run before the predecessor upload is
   sealed).

**Why not just retry:** D-051/D-056/D-062 all explicitly reject
"rerun until one pair passes" as selection bias, and this session already
burned five merge-conflict rounds while `main` kept advancing during
each CI wait — a retry is also a bet that the same host-level order
effect doesn't recur, not a fix for anything PR-5 controls. Widening the
gate's classifier or changing its measurement order is the concurrent
actor's byte-exact-reviewed CI-workflow domain, not something this PR
can or should patch as a side effect of shipping compiler work.

**Escalated to the user** with the two facts above, and three options:
one documented probe re-run, a D-054-style audited exception for this
one merge, or pausing/adjusting the gate's classifier so this stops
recurring. Full CI check list at
[the failed run](https://github.com/rotnov/pycc/actions/runs/30205958740).
No action taken on the gate itself pending that decision; the fifth
merge round's local verification (tests/clippy/doc/100% coverage/evals)
already passed before this push.
