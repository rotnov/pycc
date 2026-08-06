---
id: D-044
title: "Frontend performance is required by the aggregate merge gate"
status: accepted
---

## D-044: Frontend performance is required by the aggregate merge gate

- Status: accepted (PR-4 review correction; supersedes D-042's informational-only consequence)
- Context: ARCHITECTURE.md says a frontend regression over 2% blocks merge, and DELIVERY_PLAN.md repeats that the PR-4 baseline starts a per-PR check whose later regressions fail the pull request. D-042 correctly established a benchmark and an executable JSON comparison because Criterion does not fail on regressions by itself, but then made the job informational until a later live cache round trip. That consequence weakened an existing hard contract inside the implementation pull request instead of implementing it. A temporarily absent previous baseline cannot justify making the whole job optional: the job already handles the bootstrap run explicitly by recording the first baseline and succeeding.
- Decision: add `frontend-perf-gate` to `ci-gate.needs` and require its result to equal `success`, exactly like every other job in the aggregate. A restored baseline that measures more than 2% regression makes `scripts/check_perf_regression.rb` exit non-zero, fails `frontend-perf-gate`, and therefore fails the branch-protected `ci-gate`. When no previous baseline exists, the job records one and succeeds; that is the only bootstrap behavior, not an exemption from the required job.
- Alternatives: leave the job informational until two production runs prove cache restoration (rejected because it directly contradicts the already-accepted merge invariant); require `frontend-perf-gate` as a separate branch-protection context (rejected because D-032 deliberately centralizes `ci.yml` jobs behind one stable aggregate); fail whenever no previous baseline is restored (rejected because the first legitimate run has nothing to compare).
- Consequences: PR-4 cannot merge while the performance job is missing, skipped, cancelled, pending, or failing. Cache lifecycle defects now fail closed at the aggregate boundary instead of silently removing the performance invariant. D-042 remains unchanged as historical rationale; this entry is the explicit superseding decision.

