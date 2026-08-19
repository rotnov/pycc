# 2026-08-19-03 — issue #147, loop-control fast path and final gate run

Continues [2026-08-19-02](2026-08-19-02-issue-147-bigint-range.md). Same task
branch `claude/issue-147-bigint-range`, still based on default-branch tip
`0ffd7aad` (re-fetched and unchanged at the time this entry was committed).
Still unpushed, still no pull request — that remains the calling session's
work.

## What changed since the previous checkpoint

A third commit, `bc9108d9`. Verifying the previous checkpoint's work surfaced a
runtime performance regression that no CI gate would have caught: routing all
three `range_continue` operands through `encoded_int_cmp` unconditionally cost
about 22% on an ordinary smallint loop. Five paired release rounds of
`for i in range(50000000): total = total + 1`, built with the pre-change driver
from a temporary `0ffd7aad` worktree and with the branch driver, measured a warm
0.32s before and 0.39s after.

The gap matters because `scripts/check_replicated_paired_perf_regression.rb`
times `pycc check` — frontend compilation — not generated-program runtime, and
`tests/nbody_bench.rs::nbody_release_binary_meets_required_speedup_over_cpython`
is `#[ignore]`d and is not invoked by any workflow. Nothing in CI measures the
runtime of a compiled program, so this had to be measured by hand.

`range_continue` now carries an explicit three-inline-operand fast path that
orders plain `i64`s; re-measuring gives a warm 0.30s against the same 0.32s
baseline. It is a performance shortcut only — `inline_int_value` returns `None`
for a bigint so every promoted operand still takes the general path, and a
malformed word still fails closed in `classify_encoded_int` either way. The
before/after numbers and the absent-gate reasoning are recorded in D-179's
Consequences.

The same commit corrects three documentation inaccuracies found while
verifying: a `docs/TYPE_SYSTEM.md` cross-reference to a rule-4 `range` bullet
that does not exist, and `docs/RUNTIME.md` plus `docs/ROADMAP.md` claims that
every int-to-int comparison funnels through the D-141 runtime boundary.

## Gate results at `bc9108d9`

All captured as the gate's own exit status, not through a pipeline.

- `cargo llvm-cov --workspace --fail-under-lines 100 --fail-under-regions 100`
  — exit 0; TOTAL 100.00% regions (83293/0 missed), 100.00% functions
  (3690/0), 100.00% lines (62074/0). `crates/pycc_rt/src/int_encoding.rs` and
  `crates/pycc_rt/src/lib.rs` are both 100.00% on all three.
- `cargo test --workspace` — exit 0, no failures.
- `cargo clippy --workspace --all-targets -- -D warnings` — exit 0.
- `cargo fmt --all --check` — exit 0.
- `python3 -m unittest discover -s scripts -p 'test_*.py'` — exit 0.
- `python3 scripts/validate_agent_policies.py` — exit 0.
- `ruby scripts/check_ci_permissions.rb` — exit 0.
- `ruby scripts/check_roadmap_evidence.rb` and its test — exit 0 each, with
  `RUBYOPT=-EUTF-8`. Without it both fail on this machine at the unmodified
  baseline too; it is a local ruby default-encoding artifact, not a defect in
  the change.
- `python3 scripts/generate_decisions_index.py --check docs/decisions
  docs/decisions/README.md` — exit 0.

## Known gap in local verification

`tests/conformance.rs::oracle_python_bin` requires exactly CPython 3.14.7; this
machine has 3.14.6, so all 50 conformance tests abort at that guard regardless
of fixture correctness. This is pre-existing and unrelated to #147. The new
`bigint_range` fixture was therefore verified manually instead: built with
`pycc build` in both the debug and release profiles, run, and `cmp`-compared
against `python3.14 tests/fixtures/bigint_range.py` — byte-identical in both
profiles.

## Where a fresh session should look

The branch is committed and coherent at `bc9108d9`. Remaining work is the
calling session's: run the pinned deep reviewer over the full range
`0ffd7aad..HEAD`, then push and open the pull request. The diff touches no
`.github/workflows/` path and no path listed in
`tests/fixtures/policy-successor-manifest.json`, so neither the D-080 staged
CI-digest pattern nor the D-103 policy-successor pattern applies.
