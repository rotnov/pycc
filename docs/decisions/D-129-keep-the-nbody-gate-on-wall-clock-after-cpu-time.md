---
id: D-129
title: "Keep the nbody gate on wall clock after CPU time fails to reduce variance across Tier-1"
status: accepted
---

## D-129: Keep the nbody gate on wall clock after CPU time fails to reduce variance across Tier-1

- Status: accepted (Phase B; closes issue
  [#226](https://github.com/rotnov/pycc/issues/226))
- Context: D-126 required at least five independent exact-child CPU-time
  observations from every Tier-1 leg before changing the nbody assertion or its
  floors, and explicitly allowed a negative Phase B result. Five ordinary CI
  observations now exist from runs
  [30814975808](https://github.com/rotnov/pycc/actions/runs/30814975808),
  [30815296286](https://github.com/rotnov/pycc/actions/runs/30815296286),
  [30816067817](https://github.com/rotnov/pycc/actions/runs/30816067817),
  [30817223376](https://github.com/rotnov/pycc/actions/runs/30817223376), and
  [30817715798](https://github.com/rotnov/pycc/actions/runs/30817715798).
  The first run used the final benchmark implementation before a manifest-only
  relocation of unchanged target-specific dev-dependency tables; the other four
  ran the reviewed Phase A tree or descendants containing it. The full
  observation table is recorded in
  [#226](https://github.com/rotnov/pycc/issues/226#issuecomment-5166985278).
  Across the five ratio observations, the sample coefficient of variation was:

  | Tier-1 leg | Wall-clock CV | CPU-time CV | CPU-time change |
  |---|---:|---:|---:|
  | macOS aarch64 coverage | 2.069% | 1.548% | 25.1% lower |
  | Ubuntu x86_64 | 13.979% | 13.985% | 0.05% higher (effectively identical) |
  | Ubuntu aarch64 | 0.995% | 0.990% | 0.5% lower (effectively identical) |
  | macOS x86_64 | 10.636% | 9.230% | 13.2% lower |
  | Windows x86_64 | 6.055% | 8.729% | 44.2% higher |

  CPU time therefore improves two macOS legs, is effectively unchanged on both
  Linux legs, and is materially worse on Windows. Most importantly, it does not
  improve the motivating Ubuntu x86_64 instability: the first observation was
  19.7282x wall and 19.7377x CPU, so a 20x CPU gate would have failed the same
  byte-identical tree.
- Decision: retain the wall-clock ratio as the sole nbody assertion and retain
  D-093/D-095/D-096/D-101's existing 20x/12x/15x/18x target-specific floors.
  Continue recording exact-child CPU time, both ratios, all four medians, the
  required wall floor, and `wall_pass` through D-126's frozen summary schema.
  CPU time remains diagnostic telemetry, not a result selector or fallback
  gate. No CPU-time floors are introduced because the evidence does not support
  a cross-platform methodology switch.
- Alternatives: switch every leg to CPU time and derive margins below each
  observed minimum (rejected because the new clock leaves Ubuntu x86_64's
  variance unchanged and makes Windows variance worse); use CPU time only on the
  two macOS legs (rejected because a per-platform mixture of clock semantics
  would complicate the acceptance contract while leaving the known Linux
  failure mode untouched); gather more observations before deciding (rejected
  as a blocker because D-126 deliberately set five per leg as the decision bar
  and the negative result is already clear on the motivating and worst-regressed
  legs; retained telemetry can still support a later superseding decision);
  lower the wall floors or accept reruns (rejected for D-126's original reason:
  either would hide instability without improving the measurement method).
- Consequences: Phase B changes documentation, not benchmark code, workflow
  trust boundaries, launch count/order, correctness checking, assertion metric,
  or floors. The dual-clock telemetry remains useful for diagnosing future
  failures, but issue #226 is complete with an evidence-backed negative result
  rather than an unsupported CPU-time gate.

