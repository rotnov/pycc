# Repository governance

`main` is a protected integration branch. GitHub settings, not contributor intent,
enforce the normal delivery path.

## Required main-branch controls

- Pull request required; direct pushes are rejected.
- The branch must be current with `main` before merge.
- Required status checks: `ci-gate` and the trusted `audit` context. `ci-gate`
  (D-032/D-171) is a single stable-named, fail-closed job: it always requires
  `classify-changes` and `governance` to succeed, requires every selected
  compiler or Pages job to succeed, runs agent-dependent checks inside
  `governance`, and requires every unselected conditional job to be skipped.
  `governance` is where unconditional repository-policy gates live, so a new
  one is added as a step there rather than as its own required context --
  issue #595 wired `scripts/check_conformance_breadth.py` in that way, and the
  required-check list above is unchanged as a result. Prefer that placement:
  a separate required context for a gate `ci-gate` already fans in is a second
  control over the same contract, editable independently of the first.
  Compiler routing fans in
  `build-test-coverage` and the four-target `native-build-test`/
  `cross-compile-build`/`cross-compile-verify` Tier-1 matrix -- named directly
  rather than each matrix leg, since a matrix job's GitHub-generated check
  name bakes in its matrix values and would go stale the moment an
  `os`/`target` entry changes. D-044's untrusted `frontend-perf-measure` job
  and isolated `frontend-perf-gate`, which make a measured >7% regression
  merge-blocking (D-114; originally >2%, see below) without executing
  PR-head comparator code, are required by
  this fan-in. D-051/D-053 supersede D-048's cross-run artifact transport with
  exact predecessor and candidate timings measured sequentially on one hosted
  runner, and D-056 adds trusted pre-execution source identity. The predecessor
  timing is sealed before candidate code runs; the isolated gate consumes both
  artifacts by distinct numeric IDs, flattens each into an exact destination,
  verifies the predecessor-owned source-aware comparator, and fails closed on
  revision, benchmark-contract, executable-input identity, artifact-identity,
  file-set, or comparison drift. D-062's fixed-replicate contract addresses the
  residual changed-source single-observation variance tracked in open issue
  #109 with five fixed runs per revision and a median-of-five aggregate. D-171
  keeps that reviewed performance boundary inside change-aware routing, while
  the base-owned D-172 audit validates named security, coverage, Tier-1,
  performance, Pages/accessibility, and aggregate-gate properties instead of a
  complete `ci.yml` digest. Historical whole-workflow digests remain audit
  evidence only. The reviewed search-ledger trust anchor remains active. It
  fetches protected head inputs as regular, non-executable Git data, rejects
  checkout-affecting attributes, and audits the query registry, ledger, and
  checkpoints without executing candidate code. The successor manifest is a
  bounded historical input inventory, not authority that can force a later PR
  to activate exact bytes. The active D-171 matrix preserves macOS Intel as a
  Tier-1 native leg. The standalone `agent-policy` job provides faster
  feedback until its exact context has run successfully on `main` and is
  added to branch protection.
- Zero approving reviews are required while this is a solo-maintainer repository.
  Requiring the author's own approval would deadlock every pull request. Enable one
  independent approval, stale-approval dismissal, and last-push approval when a
  second human maintainer is available. Significant changes still require the
  repository's pinned local review loop before publication.
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

### Active fixed-replicate source-aware paired-runner gate (D-051/D-053/D-056/D-062)

Three complete CI attempts for PR #107 measured a documentation-and-test-only
candidate at `+80.82%`, `+12.82%`, and `+7.30%` against the same exact
predecessor. The third candidate estimate had a narrow 95% confidence interval,
so longer sampling on that host would not remove the observed cross-host
offset. Re-running until a convenient hosted machine passes is not acceptable
merge evidence.

The reviewed D-051 design checks out
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
and accepts exactly two regular-file estimates. D-056 retained this entire
boundary and addresses the residual false-positive class demonstrated by main
run 30198852753: a `+3.14%` delta with identical executable inputs. Before
candidate execution, the active measurement job classifies the complete `src/`
and `crates/` trees while the existing contract independently binds every
benchmark and build input. The gate accepts only an exact boolean identity,
always downloads and validates both timing artifacts, and treats the delta as
non-blocking environment telemetry only when all executable inputs are proven
identical. D-062 retains that classifier and the existing `>2%` block for
changed source, but fixes the sample plan at five complete runs per revision.

**Update (2026-08-03, D-114):** every `2%`/`>2%` figure in this subsection
(the reviewed D-051 design, D-056's residual-noise fix, and D-062's
five-replicate sample plan above, and the PR #131 incident narrative below)
accurately describes what that specific decision set or retained at the
time. D-114 later raised the live regression threshold to `7%` to
accommodate v0.2 PR-10's real, one-time `Ty`-migration cost -- see the
"active `.github/workflows/ci.yml`" paragraph immediately below for the
current governance contract, and issue #296 for the plan to lower it back
toward 2% once that one-time cost is absorbed into every future baseline.

The active `.github/workflows/ci.yml` uses D-171 change-aware routing. For a
compiler-relevant change, its `frontend-perf-measure` and isolated
`frontend-perf-gate` retain D-112's Ubuntu runner, D-062's five-replicate
sample plan, and D-114's `>7%` regression threshold; per D-203 the D-091
bench-manifest tail check tolerates exactly one reviewed line -- the
`pycc_scratch` root dev-dependency -- and hard-aborts on every other tail
difference. The D-172 audit validates
the exact predecessor/candidate bindings, artifact identities, reviewed
comparator, threshold command, routing dependency, and `ci-gate` result
branches as named properties; it does not authorize the active workflow by a
whole-file digest. D-051, D-056, D-062, D-080, D-084, pre-D-100 D-091,
pre-D-100 D-099, D-100, D-112, and D-114 fixtures remain historical evidence;
D-048 remains absent. Every selected pull request and every `main` push still
measures both exact revisions inside its own run, so no successful external
baseline artifact or administrative bootstrap state is required.

PR #131 and its post-merge main run later gave contradictory `+0.10%` and
`+3.66%` outcomes for byte-identical Git trees, proving that one paired median
still permits within-run/order variance to decide a changed-source gate. D-062
fixes the sample count at five per revision and compares the
median of those five medians when D-056's classifier reports `false`; exact
`true` keeps D-056's non-blocking telemetry result. Predecessor-first sealing
and the unchanged greater-than-2% block remain. Every one of the exact ten JSON
files is retained for 90 days; missing, extra, malformed, or symlinked evidence
fails closed. Activation does not close #109 until repeated changed-source PR
and post-merge runs validate the blocking path without result selection.

When a new check is introduced, first merge and observe it successfully on `main`,
then add its exact reported context to branch protection. Never require a guessed or
not-yet-emitted context, because that creates an unfulfillable merge gate.

## Live monitoring scope

Every external monitoring cycle starts by fetching the remote, resolving its
default branch, and recording that branch's exact commit. For every open pull
request, the checkpoint records its number, state, draft status, and exact head;
for every task-active pull request it also records mergeability, unresolved
review threads, and required checks. Subsequent work is event-driven from that
checkpoint: a new default-branch commit; a newly opened or reopened pull
request; a state, draft-status, or head change relative to an inventoried pull
request's baseline; or a mergeability, review-thread, or required-check change
on a task-active pull request is live work. An `updated_at` change caused only
by comments, reactions, labels, or activity outside those fields is not a live
event. Once an eligible new state has been evaluated and recorded, it becomes
the next checkpoint rather than an event to rediscover on every poll.

Links in this specification, an ADR, the roadmap, a retrospective, or a session
log do not enter the live monitoring set by citation alone. Closed incidents,
closed issues, and closed or merged pull requests remain historical evidence.
A task-active pull request's post-checkpoint close or merge is evaluated once
and then removed from the live set. Issues are not part of general repository
polling and are inspected only when the active task names one for a bounded
audit. In particular, D-054's incident #125 and staging PR #119 remain evidence
for the completed one-shot emergency path, not recurring poll targets.

Before waiting for CI, the monitor verifies the pull request's current state,
draft state, mergeability, exact head, and unresolved review threads. A merge,
closure, conflict, or head change ends the old wait immediately. For a newly
observed `main` merge, the monitor evaluates the introduced commit range and
verifies the expected post-merge workflows against that exact merge commit
before advancing the checkpoint.

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

D-172 records a second, distinct one-use authorization for the #558 recovery.
Because that pull request itself removes D-103's exact-byte deadlock, D-125's
external-state-only scope does not apply. The repository owner authorized an
agent-operated D-024 window for that recovery PR only: remove only `audit` from
the required set, merge at most the exact reviewed head within ten minutes,
never relax `ci-gate`, prevent another merge during the window, restore the
complete captured protection snapshot immediately, and publish an independent
post-restore verification. This authorization is exhausted by that operation
and grants no standing bypass permission.

## Session-driven temporary bypass

A second, narrower relaxation path exists alongside the Emergency path
above, for exactly one situation: a required CI check that is provably
stuck due to external repository state, not the current pull request's
own defect. Recorded in `docs/decisions/D-125-session-driven-temporary-ci-check-relaxation.md`, narrowly superseding D-024's
"not delegated to routine tasks" and D-054's "grants no reusable
permission" for this mechanism only. Full workflow: `.claude/skills/ci-temporary-bypass/SKILL.md`.

Unlike the Emergency path above, this one does not require a human
administrator to personally operate GitHub's UI/API for each use -- any
session (attended, or the standing autopilot loop unattended) may invoke
it using its own authenticated `gh` access, provided every step in the
linked skill's workflow is followed: two independent adversarial
`Agent()` verifications (before relaxing, and after restoring), a public
`[ci-bypass]`-prefixed incident issue created before any protection edit,
relaxation of exactly the one named check via the scoped `PATCH
.../protection/required_status_checks` endpoint, and a byte-exact
restore verification. `scripts/manage_ci_bypass.py status` reports
drift between current protection and this document's own baseline,
except when that exact drift is fully explained by a currently open,
unexpired `[ci-bypass]` incident's own recorded pre-relax snapshot and
relaxed check (an in-progress relaxation, not a governance incident);
`AGENTS.md`'s "Protect main" section requires every session's preflight
to run it and restore immediately if drift is found with no live
tracking incident.

The canonical protection snapshot requires the four review-policy fields in
the documented baseline: stale-review dismissal, code-owner review, last-push
approval, and approving-review count. It removes only explicitly classified
response metadata (`url`) from GitHub's `required_pull_request_reviews`
response; every other returned field is preserved in comparisons and persisted
incident snapshots. Changes to the four baseline fields and any additional
effective or unclassified field therefore remain drift. If GitHub adds another
effective review-policy setting, the baseline and metadata classification must
be extended deliberately before that setting can become part of the repository
contract.
Snapshots persisted by an earlier tool version are normalized on read as well,
so an already-open incident remains restorable after this canonicalization.

Every other requirement from the Emergency path above still applies
without exception: exactly one control relaxed at a time, immediate
restoration, full public auditability. The Emergency path itself is
unchanged and remains the path for anything this narrower mechanism does
not cover (broader relaxations, or when no session with the owner's own
`gh` access is available to run it).
