# Repository governance

`main` is a protected integration branch. GitHub settings, not contributor intent,
enforce the normal delivery path.

## Required main-branch controls

- Pull request required; direct pushes are rejected.
- The branch must be current with `main` before merge.
- Required status checks: `build-test-coverage` and the trusted `audit` context.
  `build-test-coverage` runs the agent-policy tests and clean-clone validator;
  the standalone `agent-policy` job provides faster feedback until its exact
  context has run successfully on `main` and is added to branch protection.
- Zero approving reviews are required while this is a solo-maintainer repository.
  Requiring the author's own approval would deadlock every pull request. Enable one
  independent approval, stale-approval dismissal, and last-push approval when a
  second human maintainer is available.
- All review conversations must be resolved.
- Administrators are included; force pushes and branch deletion are disabled.

When a new check is introduced, first merge and observe it successfully on `main`,
then add its exact reported context to branch protection. Never require a guessed or
not-yet-emitted context, because that creates an unfulfillable merge gate.

## Direct-commit audit

`.github/workflows/main-history-audit.yml` runs
`scripts/check_main_history.py` after every push to `main`. The script queries GitHub's
commit-to-pull-request association for every commit in the push and fails when no
merged PR targeting `main` exists. It also fails closed when GitHub reports branch
creation, a zero `before` SHA, or a forced/non-fast-forward update, because PR
association alone cannot prove that an old associated commit arrived through its
original merge path. Its revision enumeration, event-shape, API-failure, malformed
response, associated-commit, and unassociated-commit paths are covered by
`scripts/test_check_main_history.py`. This is an alert and forensic control; it does
not replace preventive branch protection.

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
