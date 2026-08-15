# Change-Aware CI Feedback Routing Design

## Status and scope

This design implements issue #558 as cross-cutting P1 CI/governance work. It
reduces redundant pull-request execution while preserving the stable required
`ci-gate`, the base-owned `audit` trust anchor, exact compiler coverage, the
Tier-1 matrix, cross compilation, paired frontend performance, and Pages
quality gates whenever their inputs can change.

The design does not lower a threshold, delete a gate, weaken branch
protection, or use the temporary bypass. It changes when an already-required
gate is relevant. The change supersedes D-014's requirement to execute Rust
coverage on literally every pull request: coverage remains mandatory for every
compiler-relevant change, while a base-reviewed, fail-closed classifier may
prove that a pull request cannot affect the coverage denominator or its test,
build, benchmark, and toolchain inputs.

An accepted successor ADR records only that scheduling change; D-014 itself is
not rewritten. The 100% line and region thresholds, workspace scope, isolated
`nobody` sandbox, pinned tools, and whole-file exemption policy remain intact.
`AGENTS.md`, `docs/TESTING.md`, `docs/REPOSITORY_GOVERNANCE.md`, and the
roadmap-evidence checker are updated to state and enforce the new contract.

## Evidence and measurement contract

The fixed baseline is the 20 most recently closed pull requests at `main`
commit `30ff72bb2f9d89fc4824a90bdc3e0e7f5cbf356a`, PRs #519 through #557. The
sample contains 18 merged and two empty closed pull requests, 30 CI attempts,
15 unsuccessful attempts, 1,064 aggregate runner-minutes, and 518
runner-minutes in unsuccessful attempts. Median pull-request lifetime was 18
minutes and median final CI duration was seven minutes. Only PRs #524 and #537
changed compiler code, but all attempts ran the full CI topology.

These figures are a preserved baseline, not an immediate success claim. A
separate follow-up issue will repeat the same calculation after 20 pull
requests have merged following the final CI activation merge. Its start commit
and activation PR number must be recorded from live GitHub state after that
merge. One or two post-activation runs are diagnostic only.

## Alternatives considered

### 1. Always-run, fail-closed classifier (selected)

The CI workflow always starts and first classifies the exact changed paths.
Heavy jobs use job-level conditions, and `ci-gate` accepts a skipped job only
when the corresponding classifier output says that job is irrelevant. Unknown,
empty, malformed, or unsupported input selects the complete gate set.

This keeps the required context stable, provides the largest safe reduction in
runner work, and makes the skip decision reviewable and testable.

### 2. Trigger-level `paths` filters (rejected)

Filtering the whole CI workflow at `on.pull_request.paths` is smaller YAML, but
GitHub would not emit the required `ci-gate` context for excluded changes. A
documentation pull request could then remain permanently pending. This design
cannot be used while `ci-gate` is required.

### 3. Require only fast checks before merge and move the full topology to
`main` or a schedule (rejected)

This maximizes speed but removes pre-merge platform, coverage, and performance
evidence from compiler changes. It contradicts the repository's Tier-1 and
coverage contracts and makes `main` the first place a real regression can be
observed.

## Architecture

### Classifier

Add `scripts/classify_ci_changes.py` with a small functional core and a CLI.
The CLI consumes the NUL-delimited path stream produced by `git diff
--name-only -z` for pull requests and writes exact lowercase boolean outputs
to `$GITHUB_OUTPUT`. A `push` to `main` always selects the complete topology so
post-merge evidence remains intact.

The outputs are:

- `compiler`: Rust source, Cargo/toolchain/build configuration, Rust tests and
  fixtures, compiler benchmarks, the classifier itself, or any unknown path.
- `pages`: website source plus the scripts and fixtures that define the two
  hermetic Lighthouse gates.
- `agent`: repository-owned agent skills, adapters, policies, and their
  validators, used to decide whether the offline alpha skill evals need a
  compiler binary.

Classification is fail-closed. The implementation recognizes a bounded set of
known non-compiler roots and files; a path that matches no reviewed rule sets
all outputs to true. An empty change set, a non-NUL input contract violation,
an unsupported event, or an invalid output destination exits non-zero or emits
the complete topology. Filenames are never interpolated into shell command
text.

The classifier and its test land at their final live paths in the staging PR,
before any workflow relies on them, and enter the D-103 manifest in steady
state. D-103 cannot stage a brand-new absent target: the trusted checker
requires every candidate manifest target to exist as a regular file. Once the
staging PR merges, those exact bytes are base-owned and protected before the
later CI activation uses them. Open PR #518's `.agents/`, `.claude/`, and
`.harden/` paths classify as agent/governance inputs, not as irrelevant
changes.

### Required lightweight jobs

`classify-changes` and `governance` run on every CI invocation.

`classify-changes` uses a full-history, credential-free checkout and diffs the
event's exact base and head revisions. It exports only booleans; no
pull-request-controlled path text crosses a job output boundary.

`governance` performs the repository's Python policy suite and the Ruby
workflow, roadmap-evidence, and README binding checks once on Ubuntu. When
`agent=true`, it also builds the compiler binary and runs both offline alpha
skill evals. The macOS coverage job no longer repeats those policy checks.

The standalone `Agent policy` and `Agent assets` workflows retain
`validate_agent_policies.py`, `validate_agent_assets.py`, the CI-monitoring
shell tests, pinned CLI version checks, and both marketplace validations. They
stop repeating only the complete Python unittest discovery already owned by
`governance`, and their triggers are limited to the assets they validate. They
remain informational contexts unless branch protection is deliberately changed
in a later task.

### Conditional heavy jobs

The following jobs run only when `compiler=true`:

- `build-test-coverage`
- the four-leg `native-build-test` matrix
- `cross-compile-build` and `cross-compile-verify`
- `frontend-perf-measure` and `frontend-perf-gate`

The two Lighthouse jobs run only when `pages=true`. The selected jobs retain
their existing commands, thresholds, permissions, artifact provenance, and
trusted predecessor boundaries. Conditional routing must not edit the
coverage command, the 100% line/region thresholds, the Tier-1 matrix, the
performance comparator, or the Pages budgets.

### Fail-closed aggregate gate

`ci-gate` keeps `if: always()` and `permissions: {}`. It always requires the
classifier and governance jobs to report literal success. For every optional
job it applies this truth table:

| Classifier output | Accepted job result |
|---|---|
| `true` | `success` only |
| `false` | `skipped` only |

Every other combination fails, including a selected job that is skipped after
an upstream failure and an unselected job that unexpectedly executes. This is
stronger than treating every `skipped` result as success.

### Concurrency

CI receives a per-PR concurrency group and cancels only an older run for the
same pull request. Main pushes are never cancelled. This prevents a superseded
head from consuming all Tier-1 runners without erasing post-merge evidence.

## Security and failure handling

- `audit` remains required and unchanged. The classifier, checker, checker
  self-test, final CI workflow, and their exact digests are staged as
  base-owned data before activation.
- Pull-request jobs retain read-only or empty token permissions. No secret,
  write scope, OIDC permission, protected environment, cache, or artifact
  boundary is added to the classifier or governance jobs.
- A missing base/head revision, checkout failure, classifier failure, or
  malformed output prevents `ci-gate` from succeeding.
- Changes to `.github/workflows/ci.yml`, the classifier, its tests, or another
  unrecognized path select every heavy gate, so the activation pull request
  proves the complete topology before merge.
- Cross-job outputs contain fixed keys and literal booleans only.

## Protected delivery sequence

D-103 and the base-owned roadmap-evidence checker require three sequential
pull requests, following PRs #555, #556, and #557:

1. **Stage.** Add the classifier and its test at their final live paths, add
   them to the manifest in steady state, and add the superseding ADR and
   documentation plus exact inert successors for
   `check_roadmap_evidence.rb`, its self-test, and `ci.yml`. Update the
   complete manifest so the three existing protected targets point at their
   staged successors. Existing live protected targets do not change. This PR
   does not close #558.
2. **Activate the checker.** Copy the staged checker and self-test byte for
   byte into their live paths and return their manifest entries to steady
   state. The live CI workflow remains unchanged. This PR does not close #558.
3. **Activate CI.** Copy the staged CI workflow byte for byte into its live
   path, return its manifest entry to steady state, and apply the
   manifest-unlisted standalone workflow cleanup. This PR carries `Fixes
   #558`.

Every phase starts from the then-current `origin/main`, receives a full local
validation and independent deep review, passes `audit` and `ci-gate`, and is
merged with a merge commit before the next phase starts. No phase uses branch
protection bypass.

## Test strategy

Classifier tests use literal, hand-derived path tables and exercise:

- compiler, Pages, agent, documentation-only, policy-successor, and unknown
  paths;
- added, modified, renamed, and deleted paths;
- mixed changes, where outputs are the union of every selected category;
- empty and malformed inputs;
- main-push full selection;
- exact lowercase GitHub output and failure on an invalid destination.

Checker mutation tests load the staged final workflow and prove that validation
fails when:

- unknown paths cease to select the full topology;
- a selected job may be skipped;
- an unselected job may succeed instead of being skipped;
- classifier or governance success is not mandatory;
- a heavy job loses its classifier dependency or condition;
- PR concurrency stops cancelling superseded heads or begins cancelling main;
- coverage thresholds, Tier-1 fan-in, performance provenance, or Pages gates
  are weakened.

Before each push, run the repository's complete locally reproducible gate set
from the active workflow, including prepared workspace build/tests, exact
coverage, clippy, Python script tests, agent validators, Ruby policy/evidence
tests, and marketplace checks. Platform-only behavior remains verified by CI.

## Out of scope

- Lowering coverage, performance, throughput, nbody, Lighthouse, or
  accessibility thresholds.
- Removing a Tier-1 target or changing supported platforms.
- Making `Agent policy`, `Agent assets`, or status-page checks required.
- Replacing GitHub Actions or changing runner vendors.
- Claiming an improvement before the fixed 20-PR post-activation sample is
  available.
