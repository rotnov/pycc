# 2026-08-22-07 — #558's 20-merge CI comparison published

## Baseline

`origin/main` at `eeeea4abe8c3f1ee00f7b1190e795d386999ae64` (PR #724, issue #275).
No open pull requests at checkpoint time; 115 open issues.

## What this checkpoint delivers

Issue #558's implementation merged as #570 / `163bf49f` on 2026-08-17. The issue stayed
open for one remaining deliverable, fixed precisely by the maintainer: publish a 20-merge
comparison against the frozen #519–#557 baseline, using the same definitions. That
comparison is now published as
https://github.com/rotnov/pycc/issues/558#issuecomment-5378870545. The issue is **not** closed — the savings
verdict was explicitly reserved by the maintainer and is not a computation.

### Cohort

The first 20 pull requests merged strictly after `163bf49f` (2026-08-17T16:17:49Z):
#560, #581, #582, #583, #584, #588, #589, #590, #592, #596, #597, #598, #599, #600, #601,
#605, #607, #609, #612, #613 — spanning 2026-08-18T08:46:06Z to 2026-08-19T08:50:26Z.

### The convention problem, and how it was settled

The published baseline quotes runner-minutes, but `/actions/runs/{id}/timing` returns an
empty `billable` block for public repositories — it cannot have been the source. Without
knowing the real convention the whole comparison would have been apples-to-oranges while
looking rigorous.

Runner-minutes were therefore defined as the sum of per-job `completed_at − started_at`
over non-skipped jobs of the `CI` workflow only, and that convention was calibrated by
recomputing the baseline cohort once. It reproduces the published snapshot: unsuccessful
runner-minutes **518 exactly**, unsuccessful attempts **15 exactly**, aggregate 1,074 vs
1,064 (+0.9%), median lifetime 17.6 vs 18, median final CI 6.8 vs 7. The 30-vs-32 attempt
difference resolves cleanly: two of the 32 runs belong to the empty closed PRs #527/#531
and contribute zero minutes.

This was a deliberate judgment call, and it went against explicit advice. The independent
advisor consulted before the work started (the D-127 fork-in-judgment consultation for this
task) said not to recompute the baseline, precisely to avoid a reconciliation rabbit hole;
the plan itself directs that the baseline be preserved and not restated. The call taken was
narrower than what was advised against: calibrate the convention once, keep the published
figures authoritative, and reconcile nothing. Had the recomputation disagreed, the published
figures would have stood and the disagreement been reported as a limitation. It agreed, so
the comparison rests on a convention shown to reproduce the baseline rather than on an
assumption about it.

### The finding

**The workload composition inverted.** The baseline held 2 compiler-touching pull requests
out of 20; the measurement window holds 18 out of 20. Routing can only remove work from
pull requests that do not need it, so aggregate runner-minutes over this window measure the
window's contents, not the change.

The like-for-like figures do carry signal, and they are favorable:

- per-attempt cost of a full-topology run unchanged — baseline 1,074 min / 30 attempts =
  35.8 min, window 1,042.0 min / 29 attempts = 35.9 min, both denominators restricted to
  attempts that actually ran the full topology (the baseline excludes the two zero-duration
  runs on empty closed PRs #527/#531; the window excludes the outlier below and the two
  routed-away attempts), so routing adds no overhead;
- on the two pull requests routing targets (#581, #590), cost fell to 1.2 runner-minutes
  each against ~34 for a full topology — a ~96% reduction;
- partial routing works — 64 of 440 jobs skipped against 0 of 360 in the baseline; 16 of
  the 18 compiler pull requests skipped both Pages jobs while running every compiler gate;
- unsuccessful runner-minutes rose 518 → 552 (+6.6%), or fell to 376.4 (−27.3%) once the
  outlier below is removed; unsuccessful attempts fell 15 → 11. Neither is claimed as a
  routing effect — failure counts are as composition-sensitive as aggregate minutes.

Aggregate runner-minutes rose 1,064 → 1,220 and median lifetime 18 → 23.2 min. Neither is
attributed to routing, and the comment says so rather than burying it.

### The outlier, reported separately

Run 32210643308 on #605: `frontend-perf-measure` ran 151 minutes before cancellation,
consuming 175.6 runner-minutes — 14% of the window's entire aggregate. Folding it in
silently would have made the window look uniformly expensive. Excluding it, the window is
1,044 runner-minutes across 31 attempts. It is a concrete instance of the untimed-job
hazard in #614 and the perf instability in #414/#641.

## Planning gap recorded

Task 8 of #558's implementation plan required opening a bounded follow-up issue,
`P2: Re-measure PR CI after 20 post-activation merges`. No such issue exists; the
obligation was carried on #558 itself. Task 8's Step 2 verification is likewise unsatisfiable
as literally written — it requires confirming that "#558 is closed, and the follow-up issue is
open", and neither holds: the follow-up issue was never opened, and #558 stays open by
deliberate maintainer reservation of the savings verdict. Nothing was lost — this checkpoint
and the published comment are that measurement — but the plan item was not executed as
written, an instance of the rule in `AGENTS.md`'s completion check that a plan's non-code
deliverables are items to track rather than prose accompanying the code.

## Paused autopilot

The standing `/next-milestone` directive (no arguments) remains in effect and is **paused**
at this checkpoint, not terminated.

- Active milestone: **v0.3**. Accept criterion — at least 37 `docs/PYTHON_STANDARDS.md`
  matrix rows at `◐` or better. `python3 scripts/check_conformance_breadth.py` reports 32
  evidence-backed rows at this baseline, so **v0.3 is not met** and the `issue-select` loop
  continues.
- Last iteration's outcome: #558 selected, its measurement computed and published; the
  issue deliberately stays open for the maintainer-held savings verdict.
- Next step: merge this checkpoint, then re-enter `issue-select` at step 1 with a fresh
  baseline. The strongest remaining P1 candidates are the security gaps #44, #45, and #82,
  whose premises were re-read this session and found still current.
- In-run denylist carried forward: **#20**, **#631**, **#604**. #604's original stop reason
  was lost across a context boundary and is recorded as unrecovered rather than
  reconstructed.
