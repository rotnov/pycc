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
  this fan-in. D-051/D-053 supersede D-048's cross-run artifact transport with
  exact predecessor and candidate timings measured sequentially on one hosted
  runner. The predecessor timing is sealed before candidate code runs; the
  isolated gate consumes both artifacts by distinct numeric IDs, flattens each
  into an exact destination, verifies the predecessor-owned median comparator,
  and fails closed on revision, benchmark-contract, artifact-identity, file-set,
  or comparison drift. Only the reviewed D-051 paired-workflow digest remains
  authorized. The standalone `agent-policy` job provides faster
  feedback until its exact context has run successfully on `main` and is
  added to branch protection.
- Zero approving reviews are required while this is a solo-maintainer repository.
  Requiring the author's own approval would deadlock every pull request. Enable one
  independent approval, stale-approval dismissal, and last-push approval when a
  second human maintainer is available.
- All review conversations must be resolved.
- Administrators are included; force pushes and branch deletion are disabled.

### Retired cross-run baseline provenance (D-048)

The historical D-048 activation completed through the successful
[`main` CI run](https://github.com/rotnov/pycc/actions/runs/30168696265) for
merge commit `9bed86027e3efe0e0ab9dd906457953d8ba09956` published the non-expired
90-day `frontend-perf-current` artifact `8622316274`; `frontend-perf-gate` and
the aggregate `ci-gate` both succeeded. The deletion readback for
`PERF_ACTIVATION_HEAD` returned `404` at 2026-07-25T17:59:51Z, eight seconds
before the artifact was created, because stale-attempt cleanup raced the
activation merge. This deviated from the planned post-run deletion ordering,
but did not weaken the executed boundary: the activation push bootstrap did
not read the variable, remained bound to the exact event and reviewed
predecessor, and `frontend-perf-gate` started after deletion and succeeded.
The repository Actions API confirmed continued absence after the full run at
2026-07-25T18:13:08Z. The
[post-merge audit](https://github.com/rotnov/pycc/pull/103#issuecomment-5079757567)
records the timestamps and exact artifacts. D-051/D-053 later retired this
cross-run transport, its workflow digest, and its fixture. The activation
variable, bootstrap branches, pre-split fixture, activation fixture, and every
D-048 authorization remain absent.

D-048 and D-050 preserve the historical activation lifecycle and the reason it
was bounded; they no longer describe live performance transport.

### Active paired-runner gate (D-051/D-053)

Three complete CI attempts for PR #107 measured a documentation-and-test-only
candidate at `+80.82%`, `+12.82%`, and `+7.30%` against the same exact
predecessor. The third candidate estimate had a narrow 95% confidence interval,
so longer sampling on that host would not remove the observed cross-host
offset. Re-running until a convenient hosted machine passes is not acceptable
merge evidence.

The reviewed `tests/fixtures/d51-paired-ci.yml` successor therefore checks out
the exact predecessor and candidate, verifies their revisions and the complete
bound benchmark-definition and build-configuration contract, and measures both
sequentially on one runner with separate target state. It seals the predecessor
artifact before candidate code runs, so a lingering same-user process cannot
rewrite the trusted side of the pair. The gate binds both downloads to the
distinct numeric artifact IDs emitted by the trusted upload steps, so deleting
and replacing an artifact under the same name fails closed. Each single-ID
download is flattened into its own exact comparison directory, so the pinned
action cannot add an artifact-name path component. Its isolated gate
keeps the 2% threshold and hash-verified review boundary while using an
exact-predecessor median comparator that is robust to isolated high outliers
and accepts exactly two regular-file estimates. The active
`.github/workflows/ci.yml` is byte-identical to this fixture, the allowlist
contains only its reviewed digest, and the D-048 fixture and digest are absent.
Every pull request and `main` push measures both exact revisions inside its own
run, so no successful external baseline artifact or administrative bootstrap
state is required.

### Staged source-aware successor (D-056)

The active D-051/D-053 workflow remains unchanged in this commit. D-056 adds a
reviewed inert fixture for the residual false-positive class demonstrated by
main run 30198852753: a `+3.14%` delta with identical executable inputs. The
prospective measurement job classifies the complete `src/` and `crates/` trees
before candidate execution, while retaining every existing benchmark/build
contract check. Its gate accepts only an exact boolean identity, always
downloads and validates both timing artifacts, and treats the delta as
non-blocking environment telemetry only when all executable inputs are proven
identical. Changed source keeps the existing `>2%` block.

Activation requires a separate branch from the staging merge, a byte-exact
replacement of `.github/workflows/ci.yml` with
[`d56-source-aware-ci.yml`](../tests/fixtures/d56-source-aware-ci.yml), the
trusted-base `audit`, and normal `ci-gate`. The activation then retires D-051's
active digest; this staged paragraph grants no bootstrap or bypass.

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

D-054 records the only use of this path to date. Public
[incident #125](https://github.com/rotnov/pycc/issues/125) documented the D-048
exact-base deadlock, and explicit administrator authorization allowed
only `ci-gate` to be removed from the required-check set for at most ten minutes
and exactly one staging merge. Strict up-to-date protection, `audit`, administrator
enforcement, review and conversation rules, and the force-push/deletion prohibitions
remained enabled. [PR #119](https://github.com/rotnov/pycc/pull/119) merged as
`416f626fcd8406cc60781d9415589367d4d9c18a`; an unconditional exit trap restored
the app-bound `audit` plus `ci-gate` set within seconds, and the full settings
readback is attached to the incident. The exception is closed and grants no
permission for any future bypass.
