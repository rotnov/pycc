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
  merge-blocking without executing PR-head comparator code, join this fan-in.
  D-048 supersedes D-047's temporary PR-6 deferral and replaces D-046's
  ref-scoped cache transport with exact-predecessor artifacts from successful
  `main` runs. D-051 completes the activation: the live `ci.yml` is
  byte-identical to the reviewed steady-state fixture, only that digest remains
  trusted, and no missing-baseline bootstrap or activation variable exists. The
  standalone `agent-policy` job provides faster feedback until its exact
  context has run successfully on `main` and is added to branch protection.
- Zero approving reviews are required while this is a solo-maintainer repository.
  Requiring the author's own approval would deadlock every pull request. Enable one
  independent approval, stale-approval dismissal, and last-push approval when a
  second human maintainer is available.
- All review conversations must be resolved.
- Administrators are included; force pushes and branch deletion are disabled.

### Frontend performance baseline (D-048/D-051)

Every pull request resolves the exact `pull_request.base.sha`; every `main`
push resolves the exact event `before` SHA. The isolated gate accepts only a
successful `push` run of `ci.yml` on `main` for that exact predecessor and
downloads its non-expired `frontend-perf-current` artifact by explicit run ID.
It never falls back to an older run or a ref-scoped cache. A missing, expired,
cancelled, malformed, or non-exact artifact fails the required `ci-gate`, and
the comparison has no skip expression.

The checked-in `tests/fixtures/d48-steady-ci.yml` is the byte-exact workflow
trust anchor. The roadmap checker retains only its SHA-256 digest. Changes to
this workflow follow the staged prospective-digest procedure in
[TESTING.md](./TESTING.md); editing the workflow and its sole trusted digest in
one unreviewed step is forbidden.

The one-time activation is complete and preserved as audit evidence:

- PR #103 merged activation commit
  `9bed86027e3efe0e0ab9dd906457953d8ba09956`.
- Exact-head [CI run 30168696265](https://github.com/rotnov/pycc/actions/runs/30168696265)
  completed successfully at 2026-07-25T18:09:59Z with every required job
  green and published non-expired 90-day artifact `8622316274`.
- `PERF_ACTIVATION_HEAD` deletion was confirmed by a 404 at
  2026-07-25T17:59:51Z and remained absent after the run.
- The deletion confirmation preceded artifact creation by eight seconds
  because attempted stale-attempt cleanup raced the merge. This differed from
  D-048's prescribed ordering but did not relax the executed push boundary:
  that bootstrap never read the variable, remained bound to the exact event,
  checkout, live-main, and pre-split workflow identities, and its gate started
  after deletion and succeeded.

The activation variable, predecessor and activation fixtures, their digests,
and activation-only tests are retired under D-051. Do not recreate or reuse
them. A future transition requires a new staged decision and trust anchor.

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
