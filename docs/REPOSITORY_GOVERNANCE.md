# Repository governance

`main` is a protected integration branch. GitHub settings, not contributor intent,
enforce the normal delivery path.

## Required main-branch controls

- Pull request required; direct pushes are rejected.
- The branch must be current with `main` before merge.
- Required status checks: `ci-gate` and the trusted `audit` context. `ci-gate`
  (D-032) is a single stable-named job that fans in every job in `ci.yml`,
  including `build-test-coverage` (which runs the agent-policy tests and
  clean-clone validator) and the four-target `native-build-test`/
  `cross-compile-build`/`cross-compile-verify` Tier-1 matrix -- named directly
  rather than each matrix leg, since a matrix job's GitHub-generated check
  name bakes in its matrix values and would go stale the moment an
  `os`/`target` entry changes. D-044's untrusted `frontend-perf-measure` job
  and isolated `frontend-perf-gate`, which make a measured >2% regression
  merge-blocking without executing PR-head comparator code, are required by
  this fan-in. D-048 supersedes D-047's temporary PR-6 deferral and replaces
  D-046's ref-scoped cache transport with exact-predecessor artifacts from
  successful `main` runs. The gate is now bootstrap-free: it requires a
  non-expired `frontend-perf-current` artifact from the exact successful
  predecessor, compares untrusted PR timing through the hash-verified
  main-owned checker, and fails closed when that artifact is unavailable.
  Only the reviewed steady-state workflow digest remains authorized. The
  standalone `agent-policy` job provides faster feedback until its exact
  context has run successfully on `main` and is added to branch protection.
- Zero approving reviews are required while this is a solo-maintainer repository.
  Requiring the author's own approval would deadlock every pull request. Enable one
  independent approval, stale-approval dismissal, and last-push approval when a
  second human maintainer is available.
- All review conversations must be resolved.
- Administrators are included; force pushes and branch deletion are disabled.

### Frontend performance baseline provenance (D-048)

The one-time activation is complete. The successful
[`main` CI run](https://github.com/rotnov/pycc/actions/runs/30168696265) for
merge commit `9bed86027e3efe0e0ab9dd906457953d8ba09956` published the non-expired
90-day `frontend-perf-current` artifact `8622316274`; `frontend-perf-gate` and
the aggregate `ci-gate` both succeeded. The repository Actions API confirmed
that `PERF_ACTIVATION_HEAD` was absent after that run at
2026-07-25T18:13:08Z. The active workflow is byte-identical to the reviewed
steady-state fixture, and the activation variable, bootstrap branches,
pre-split fixture, activation fixture, and retired digests are absent.

Every later pull request must locate the non-expired artifact from the exact
successful `main` run at its base SHA. Every later `main` push must use the
exact `before` SHA. Missing, expired, cancelled, or non-exact predecessor
evidence is a hard failure; there is no reusable bootstrap or administrative
feature flag. D-048 and D-050 preserve the historical activation lifecycle and
the reason it was bounded.

When a new check is introduced, first merge and observe it successfully on `main`,
then add its exact reported context to branch protection. Never require a guessed or
not-yet-emitted context, because that creates an unfulfillable merge gate.

## Direct-commit audit

`.github/workflows/main-history-audit.yml` checks out the pre-push `main` revision of
`scripts/check_main_history.py` after every push to `main`. When that parent predates
the checker, the workflow uses immutable reviewed bootstrap commit
`2d9fcd1b4135caef19b6ebad7bf96f7111f2258d`; the pushed repository is checked
out separately and its checker is never executed. The script enumerates every commit
introduced by the push and queries GitHub's commit-to-pull-request association. Each
commit must correlate with a merged PR targeting `main` whose `merge_commit_sha` is
also in that same push. A historical association from an earlier squash or merge
therefore cannot launder a later direct push of an old source commit. The audit also
fails closed when GitHub reports branch creation, a zero `before` SHA, a
forced/non-fast-forward update, an API failure, or malformed data. Its revision
enumeration, event-shape, API-failure, malformed-response, current-merge,
historical-association, and uncorrelated-commit paths are covered by
`scripts/test_check_main_history.py`.

This is an alert and forensic control; it does not replace preventive branch
protection. GitHub loads a push workflow definition from the pushed revision, so the
job cannot be its own trust anchor: a bypassing push could change or remove the
workflow, its checkout ref, or its command. The external repository monitor therefore
compares the audit workflow with the last reviewed state and verifies that the
expected run exists and succeeds. A changed workflow, missing run, forged job shape,
or unavailable monitor is release-blocking even if another run reports success.

Treat a failed audit as a release-blocking governance incident:

1. Open an issue linking the run, commit, actor, and reason.
2. Confirm whether protection was changed or bypassed and preserve the settings
   before/after evidence.
3. Reconcile any ambiguous PR state without rewriting published history.
4. Restore the required controls and obtain review/CI for the carried-forward change.
5. Close the incident only after a later audit proves the protected path again.

## Emergency path

Only a repository administrator may temporarily edit protection when GitHub itself
prevents recovery and waiting would cause greater harm. Before doing so, create a
public incident issue that states the exact control being relaxed, justification,
owner, expiry, and rollback command/settings. Keep every unaffected control enabled.

The administrator restores the original settings immediately after recovery and
attaches the settings diff plus audit result to the incident. An agent, bot, or normal
maintenance task never receives standing bypass permission.
