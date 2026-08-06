---
id: D-126
title: "Gather exact-child CPU-time evidence before changing the nbody gate"
status: accepted
---

## D-126: Gather exact-child CPU-time evidence before changing the nbody gate

- Status: accepted (Phase A; issue [#226](https://github.com/rotnov/pycc/issues/226))
- Context: the nbody gate compares median wall-clock time from five `--release`
  pycc launches with five pinned-CPython launches on the same runner. Its
  platform floors were derived from wall-clock observations, but unchanged
  benchmark/compiler source can still flip the 20x legs: post-merge run
  [30812646930](https://github.com/rotnov/pycc/actions/runs/30812646930)
  measured 19.64x on `ubuntu-latest` immediately after the byte-identical PR
  tree passed, while every source change in that merge was confined to Python
  governance tooling and its documentation. Wall time includes time a child is
  descheduled on a shared hosted runner, so that result is not evidence of a
  generated-code regression. The existing CPU-time floors cannot be inferred
  from the wall-clock floors, however, and three Tier-1 classes are unavailable
  as local hardware. Changing the gate and guessing replacement values in one
  PR would trade a known noisy measurement for an uncalibrated one.
- Decision: split the methodology change. Phase A records both wall-clock and
  CPU time for the same existing `K = 5` launch blocks while the wall-clock
  ratio and the existing 20x/12x/15x/18x per-target floors remain the only
  gate. Unix spawns the child and uses `wait4(pid, ...)` to reap that exact PID
  and obtain its user plus system time; it does not use process-global
  `getrusage(RUSAGE_CHILDREN)`, whose delta can include another concurrently
  reaped test child. The raw wait status must represent a successful normal
  exit. Windows waits through `std::process::Child`, then calls
  `GetProcessTimes` while the retained process handle remains valid and sums
  its kernel plus user `FILETIME`. `libc` and `windows-sys` are target-gated
  dev-dependencies already present in `Cargo.lock`; no production dependency
  or workflow step is added. The exact summary record is frozen as
  `nbody speedup: wall_ratio=...x cpu_ratio=...x cpython_wall_median=...s
  pycc_wall_median=...s cpython_cpu_median=...s pycc_cpu_median=...s
  required_wall=...x wall_pass=...`, emitted on passes and failures. Phase B
  may switch the assertion only after at least five independent CPU-time
  observations exist for each of the five Tier-1 legs; it must derive every
  applicable floor from those observations and record the evidence in a new
  decision. If CPU-time variance is not better, keeping wall clock and recording
  that negative result is an acceptable Phase B outcome.
- Alternatives: switch directly to CPU time while retaining the wall-clock
  numbers (rejected because the numeric floors have no CPU-time evidence yet);
  use `getrusage(RUSAGE_CHILDREN)` on Unix (rejected because its accounting is
  process-global within a concurrently executing integration-test binary);
  lower the 20x floor or accept reruns as the remedy (rejected because that
  hides measurement instability without improving the method); modify CI to
  collect separate data (rejected because the existing ignored benchmark
  already runs on all five Tier-1 legs and can report both clocks itself).
- Consequences: `tests/nbody_bench.rs` gains a non-ignored subprocess test for
  the platform measurement path and pure unit tests for the platform time-unit
  conversions. Phase A changes diagnostic telemetry, not compiler behavior,
  benchmark workload, run count/order, correctness checking, acceptance floor,
  or CI trust boundaries. Ordinary CI merges now accumulate the evidence needed
  for Phase B without allowing the new metric to select results or unblock a
  failure prematurely.

