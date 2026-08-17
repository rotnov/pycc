# Non-blocking CI policy audit design

**Date:** 2026-08-17
**Status:** Approved direction; implementation pending
**Issue:** [#558](https://github.com/rotnov/pycc/issues/558)

## Summary

Keep the required, base-owned `audit` check, but make it validate security
properties of pull-request workflows instead of requiring general CI and
policy files to equal pre-staged successor bytes.

The recovery lands in one pull request. That pull request activates the
already-reviewed D-171 CI workflow, fixes the five stale roadmap-policy
self-test assertions exposed by activation, and retires the steady-state
exact-byte transition that caused the repository-wide deadlock. Because the
current base checker rejects that repair before it can evaluate the repaired
state, the repository owner explicitly authorized the agent on 2026-08-17 to
perform one D-024-style emergency relaxation of `audit` for this recovery only.
`ci-gate` is never relaxed.

After recovery, an ordinary CI change needs one pull request. The trusted
`audit` still runs code from the current base revision, downloads the proposed
workflow files only as non-executable data, parses them fail-closed, and rejects
violations of explicit security and routing invariants.

This specification supersedes only the D-103 staging/activation sequence and
the no-bypass recovery claim in
`2026-08-15-ci-feedback-routing-design.md`. That specification's D-171 routing,
coverage, platform, cancellation, and measurement requirements remain in
force.

## Problem

PR #562 merged a D-103 manifest state in which the next candidate was required
to replace `.github/workflows/ci.yml` with the exact staged D-171 workflow.
That workflow correctly moved policy tests into the new `governance` job, but
the staged and activated `scripts/test_check_roadmap_evidence.rb` retained five
assertions for the old workflow topology.

The resulting base state is contradictory:

- leaving the active CI unchanged fails `audit` because the manifest demands
  activation;
- activating the exact staged CI passes the base-owned transition check but
  fails five policy self-test assertions in `ci-gate`;
- fixing those assertions in the same candidate fails `audit` because their
  replacement bytes were not staged by an earlier merge.

This is a P1 governance deadlock. It blocks unrelated development even though
the candidate workflow itself satisfies the structural workflow-permission
policy.

The two-merge protocol does not remove the underlying trust risk when the same
maintainers can approve both merges: the first merge can stage a weakened
checker and the second can activate it. Its concrete benefit is only temporal
separation. That benefit does not justify a mechanism that can make all normal
pull requests unmergeable.

## Goals

- Preserve the required `audit` and `ci-gate` branch-protection contexts.
- Ensure the pull request cannot change the validator used for its own
  `audit` result.
- Permit safe `.github/workflows/ci.yml` changes in one pull request.
- Validate explicit security, provenance, and routing properties rather than
  an exact whole-file digest.
- Eliminate successor state that forces an unrelated pull request to activate
  a pending policy bundle.
- Activate D-171 while preserving Linux, macOS Apple Silicon, and macOS Intel
  Tier-1 coverage.
- Keep D-125 unchanged as the narrow mechanism for failures caused by external
  repository state, not by the current pull-request diff.

## Non-goals

- Removing `audit` or making it advisory.
- Relaxing `ci-gate`, strict up-to-date protection, administrator enforcement,
  or conversation-resolution requirements.
- Executing pull-request scripts from `pull_request_target`.
- Removing macOS Intel coverage.
- Claiming CI-time improvement before the fixed 20-merged-PR measurement window
  completes.
- Protecting against a repository owner who deliberately approves and merges a
  weakened policy. Repository-local two-merge staging does not solve that
  threat either.

## Trust model

The existing `.github/workflows/workflow-policy.yml` architecture remains the
trust boundary:

1. GitHub starts the required `audit` job on `pull_request_target`.
2. The job checks out the current base revision, not the pull-request head.
3. The job obtains candidate workflow and evidence blobs through the GitHub API
   and writes them beneath `/tmp/pr-policy-input` as data.
4. It rejects checkout-affecting `.gitattributes`, non-regular entries,
   executable entries, oversized blobs, unsafe paths, and truncated trees.
5. It runs the base revision's workflow-permission, roadmap-evidence, and
   search-evidence checkers against the downloaded candidate data.
6. It never checks out or executes candidate repository scripts.

Therefore a pull request that edits `scripts/check_ci_permissions.rb` cannot
change the validator deciding that pull request's `audit` result. Once merged,
the edited checker becomes the trusted base checker for later pull requests,
which is the ordinary protected-branch review model. Mutation tests, the
repository's mandatory local pinned-review loop, resolved review conversations,
protected main, and the post-merge history audit mitigate accidental or
unreviewed weakening. GitHub approving reviews remain at zero while this is a
solo-maintainer repository, as required by the repository governance policy.

## Policy checked by `audit`

The base-owned checker parses every candidate workflow with Psych's AST rather
than permissive object deserialization. It rejects duplicate keys, YAML merge
keys, unsafe aliases, ambiguous structures, and unsupported scalar shapes.

The structural policy must enforce at least these invariants:

### Permissions and credentials

- Workflow- and job-level permissions are read-only or `none` by default.
- Any write-capable job is explicitly recognized, runs only on `push` to
  `refs/heads/main`, establishes a trusted commit source, and does not execute
  untrusted code or consume unverified untrusted state.
- Every third-party action is pinned to a full reviewed commit SHA.
- Every `actions/checkout` step sets `persist-credentials: false`.
- Pull-request jobs cannot expose repository secrets or write-capable tokens to
  candidate code.

### Required CI topology

- The change classifier is mandatory and fails closed: missing, malformed, or
  unexpected outputs cannot turn required work into a successful skip.
- The `governance` job runs whenever the workflow has not been cancelled and
  propagates policy failures.
- Compiler-relevant changes retain Linux coverage, macOS Apple Silicon native
  coverage, and macOS Intel cross/verification coverage.
- Documentation-only and other classified changes may skip unrelated heavy
  work only through reviewed classifier outputs.
- `ci-gate` is present, depends on every required producer, and accepts a skip
  only where routing explicitly permits it. A failure, cancellation, missing
  result, or malformed result cannot produce success.
- Coverage, Tier-1, performance, Pages/accessibility, and policy requirements
  already accepted by repository decisions remain represented in the gate.

The checker may enforce exact values for individual security-critical fields,
commands, action SHAs, or gate relationships. It must not require the complete
general CI workflow or its ordinary policy tests to match a pre-staged file
digest.

The active D-171 validation in `scripts/check_roadmap_evidence.rb` must follow
the same rule. Historical fixtures may retain their reviewed full-file and
job-body digests, but the steady-state acceptance path for the active
`.github/workflows/ci.yml` must validate named routing and gate properties. It
must not require the whole active workflow or whole ordinary job bodies to
retain the original D-171 digest.

## Successor-manifest scope

`validate_policy_successor_transition` must no longer run as a steady-state
merge gate for general CI, routing, roadmap-policy, or workflow-permission
files. In particular, `.github/workflows/ci.yml`,
`scripts/check_ci_permissions.rb`, their tests, and the D-171 roadmap checker
and self-test must not require a previous merge containing exact replacement
bytes.

The implementation may retain historical successor fixtures and manifest data
needed to explain or reproduce earlier decisions. Historical evidence must not
force the next unrelated candidate to activate a pending target. Any remaining
search-specific activation protocol must be scoped to that activation and must
not gate unrelated CI changes after activation is complete.

The `workflow-policy.yml` trust-anchor workflow itself is outside the ordinary
CI-edit path because it defines how base-owned code and candidate data are
separated. It may retain a dedicated exact authorization rule. Any future
trust-anchor update must have an explicit recovery path and must never encode a
pending state that forces unrelated pull requests to activate it.

## Validation layers

Each pull request is accepted only when both independent required contexts are
green:

- `audit`: trusted base code performs static, fail-closed inspection of the
  candidate workflows and evidence as data.
- `ci-gate`: the candidate revision performs its builds, tests, routing, and
  aggregate result checks.

The repository's local pinned-review loop remains mandatory for policy-checker
changes, and all GitHub review conversations must be resolved. The audit's base
ownership prevents same-PR self-validation; review and mutation coverage
protect the effect that a checker change will have after merge. Branch
protection continues to require zero approving reviews while there is only one
human maintainer.

## Required tests

The implementation must add focused positive and negative tests for:

- a normal non-workflow pull request while historical successor fixtures exist;
- the exact D-171 candidate workflow;
- changing `ci.yml` without a previously staged whole-file copy;
- changing an ordinary D-171 job body while preserving every required security,
  routing, coverage, and gate property;
- a candidate edit to the checker not affecting the base-owned validator used
  for that candidate;
- workflow- or job-level permission widening;
- mutable or non-SHA action references;
- missing `persist-credentials: false` on every checkout;
- privileged jobs reachable from pull-request or non-main events;
- secrets or privileged state crossing into untrusted execution;
- classifier failure, missing output, malformed output, and misdirected output;
- removal or weakening of required Linux, macOS ARM, or macOS Intel paths;
- removal, partial dependency, or truth-table weakening of `ci-gate`;
- removal or failure suppression of governance checks;
- duplicate YAML keys, aliases, merge keys, unsafe paths, non-regular Git
  entries, and checkout-affecting `.gitattributes`;
- the five stale roadmap-policy assertions observed during D-171 activation;
- the former no-op-candidate deadlock: leaving active CI unchanged must not
  require activation of an inert successor.

The existing live policy suites, D-171 staged suites, classifier tests,
actionlint, safe YAML parse, and manifest/evidence checks remain required where
applicable. The recovery must include a direct base-owned audit simulation and
the full candidate `ci-gate` result from GitHub.

## One-PR recovery

The recovery pull request will:

1. start from the current protected `main` after PR #562;
2. activate `tests/fixtures/policy-successors/ci-d171.yml` byte-for-byte as
   `.github/workflows/ci.yml`;
3. fix the five stale assertions in
   `scripts/test_check_roadmap_evidence.rb`;
4. remove the steady-state exact-byte transition from
   `scripts/check_ci_permissions.rb`, replace the active whole-workflow and
   whole-job digest checks in `scripts/check_roadmap_evidence.rb` with explicit
   property checks, and update both mutation suites;
5. add a successor decision that explicitly narrows/supersedes D-103, then
   update governance/testing/roadmap documentation and the generated decision
   index so they describe the property-based audit while preserving D-103 as
   historical evidence;
6. retain the already-reviewed D-171 routing, checkout hardening, and macOS
   Intel path;
7. pass all local policy, routing, YAML, actionlint, build, and test gates that
   are available on the development host.

The current base `audit` cannot authorize steps 3 and 4 because those exact
bytes were not staged in an earlier merge. This failure is partly attributable
to the recovery diff, so D-125 is deliberately ineligible and remains
unchanged. On 2026-08-17 the repository owner explicitly authorized the agent
to operate the broader D-024 emergency path for this one recovery merge. This
is a one-use exception to D-024's normal personally-operated rule and grants no
reusable bypass authority. The recovery decision and public incident must quote
that scope before protection changes. The agent will:

1. publicly track the bypass incident;
2. capture the complete protection snapshot, inventory open pull requests, and
   confirm no other bypass incident or merge operation is active;
3. relax only `audit` for the recovery pull request, at most one merge, and at
   most ten minutes;
4. prevent any other pull request from merging during that window and verify
   the resulting merge SHA is the reviewed recovery head;
5. never expose authentication material in the incident or command output;
6. never relax `ci-gate`;
7. merge only after all other required and review gates pass;
8. restore the exact branch-protection settings immediately on success or any
   failure;
9. complete an independent post-restore adversarial verification equivalent to
   D-125's Gate 2 and publish the restoration evidence.

## Rollback and failure handling

- Before merge, any structural-policy or `ci-gate` failure blocks the recovery;
  the D-024 exception is not used to hide a candidate-caused failure other than
  the old exact-byte authorization rule it is explicitly replacing.
- If branch-protection restoration or the independent post-restore verification
  fails, treat it as a release-blocking governance incident and stop further
  merges.
- If the activated CI fails after merge, revert the routing activation with a
  normal reviewed pull request while keeping the property-based audit. Do not
  restore the forced successor transition.
- Keep issue #558 open and marked in progress until activation, protection
  restoration, and post-merge validation are complete.

## Measurement

The historical baseline recorded on #558 remains fixed: 18 merged PRs and two
empty closed PRs from #519 through #557, 1,064 runner-minutes total, 518
unsuccessful runner-minutes, 18-minute median lifetime, and 7-minute median
final CI.

After the recovery merge, record its exact merge SHA and start the comparison
window. Do not claim improvement until 20 subsequent merged pull requests have
completed. Report the same lifetime, final-CI, total runner-minute,
unsuccessful-minute, compiler/non-compiler, and macOS Intel measures.

## Rejected alternatives

### Keep mandatory two-merge exact staging

Rejected because it caused the current P1 deadlock, blocks unrelated pull
requests, and does not prevent the same maintainers from staging and later
activating a weakened checker.

### Remove `audit`

Rejected because the base-owned `pull_request_target` check usefully validates
candidate workflows without executing candidate code and is an independent
required control alongside `ci-gate`.

### Run the candidate checker during `audit`

Rejected because a pull request could then weaken the validator used to approve
its own workflow changes.
