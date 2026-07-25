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
  merge-blocking without executing PR-head comparator code, join this fan-in
  through D-048's staged exact-head activation. D-048 supersedes D-047's
  temporary PR-6 deferral and replaces D-046's ref-scoped cache transport with
  exact-predecessor artifacts from successful `main` runs. The staging change
  authorizes both the activation and bootstrap-free steady-state workflow
  digests but does not activate those jobs; the following activation must use
  the final reviewed head recorded in `PERF_ACTIVATION_HEAD`, and the
  variable/bootstrap/activation digest must be removed after the first main
  artifact while the steady-state digest remains. The
  standalone `agent-policy` job provides faster feedback until its exact
  context has run successfully on `main` and is added to branch protection.
- Zero approving reviews are required while this is a solo-maintainer repository.
  Requiring the author's own approval would deadlock every pull request. Enable one
  independent approval, stale-approval dismissal, and last-push approval when a
  second human maintainer is available.
- All review conversations must be resolved.
- Administrators are included; force pushes and branch deletion are disabled.

### Performance activation trust anchor (D-048)

`PERF_ACTIVATION_HEAD` is a one-shot repository Actions variable, not standing
configuration. Only a repository administrator may set or delete it. The
activation sequence is fail-closed:

1. Merge the staging-policy PR while `ci.yml` is still the pre-split workflow,
   fetch the resulting `main`, and record that exact base commit.
2. From that base, create and review the workflow-only activation commit. Run
   the exact checker, its mutation tests, `actionlint`, and independent deep
   review before treating the head as final. Its `ci.yml` must be byte-identical
   to `tests/fixtures/d48-activation-ci.yml`.
3. Confirm the variable is absent, set it to the final 40-hex activation head,
   read it back through the Actions Variables API, and record the base SHA,
   head SHA, setter, timestamp, and read-back value in the activation PR
   description. Do not push another commit after this mutation.
4. If `main` advances or the activation head must change, close the activation
   attempt, delete and verify removal of the variable, refresh from the new
   `main`, re-review the new final head, and repeat the set/read-back audit.
   Merely changing the variable underneath an open reviewed head is forbidden.
5. After the activation merge, require the successful `main` `ci.yml` run and
   its non-expired `frontend-perf-current` artifact for that exact merge
   commit. A failed or cancelled attempt may be rerun while that exact commit
   remains the live `main` head; the event `after`, checkout, live-main, and
   pre-split predecessor checks still bind every retry to the same activation
   transition. Then delete the variable, verify that it is absent, and record
   the deletion evidence in the cleanup PR that replaces `ci.yml`
   byte-for-byte from `tests/fixtures/d48-steady-ci.yml` and removes every
   bootstrap path.

The SHA is public and is not a credential. Its administrative mutability is the
reason the lifecycle requires explicit before/set/read-back/delete evidence and
immediate cleanup.

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
