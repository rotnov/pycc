# Change-Aware CI Feedback Routing Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Preserve every trusted pycc merge invariant while avoiding compiler, cross-platform, performance, and Pages work on pull requests whose exact changed paths cannot affect those gates.

**Architecture:** An always-run Python classifier emits fixed boolean outputs from a NUL-delimited Git diff and defaults unknown input to the complete topology. The required `ci-gate` accepts each heavy job's `skipped` result only when the corresponding reviewed classifier output is exactly false. D-103 delivery follows the same three-merge order as PRs #555/#556/#557: stage checker/CI successors, activate the checker, then activate CI.

**Tech Stack:** GitHub Actions YAML, Python 3 standard library/unittest, Ruby policy validators/minitest, JSON D-103 manifest, Markdown ADR/specification documents, GitHub CLI.

## Global Constraints

- Base every phase on the exact then-current `origin/main`; never mutate the user's dirty `/Users/denis/.codex/worktrees/0764/pycc-proto` worktree.
- Keep `ci-gate` and `audit` required, strict up-to-date protection enabled, administrators enforced, and force pushes/deletions disabled.
- Never use D-125 or any other branch-protection bypass for this issue.
- Preserve 100% line and region coverage, the workspace denominator, isolated `nobody` boundary, pinned tools, Tier-1 targets, cross compilation, performance comparators, and Pages budgets.
- Treat workflow definitions and every repository script they execute as untrusted pull-request input; grant `contents: read` at most and persist no checkout credentials.
- Write repository, issue, commit, and pull-request artifacts in English.
- Keep issue #558 labeled `in progress` until the final activation merge; only the final PR carries `Fixes #558`.
- Preserve the baseline at `30ff72bb2f9d89fc4824a90bdc3e0e7f5cbf356a`; do not claim improvement until 20 PRs merge after the final CI activation.

## File map

- `scripts/classify_ci_changes.py`: pure path classification plus the fail-closed CLI.
- `scripts/test_classify_ci_changes.py`: table-driven behavior, malformed-input, CLI-output, and mutation tests.
- `scripts/check_roadmap_evidence.rb`: base-owned acceptance of the D-171 workflow digest and exact conditional topology.
- `scripts/test_check_roadmap_evidence.rb`: mutation tests for classifier wiring, `ci-gate` truth table, concurrency, and unchanged hard gates.
- `tests/fixtures/policy-successors/check_roadmap_evidence-d171.rb`: final checker bytes staged for the second PR.
- `tests/fixtures/policy-successors/test_check_roadmap_evidence-d171.rb`: final checker self-test bytes staged for the second PR.
- `tests/fixtures/policy-successors/ci-d171.yml`: final CI bytes staged for the third PR.
- `tests/fixtures/policy-successor-manifest.json`: D-103 transition and steady-state bindings.
- `.github/workflows/ci.yml`: final always-run classifier, lightweight governance, conditional heavy jobs, fail-closed aggregate, and PR-only cancellation.
- `.github/workflows/agent-policy.yml`: unique policy validation without duplicate broad unittest discovery, scoped trigger paths.
- `.github/workflows/agent-assets.yml`: unique asset/client/marketplace validation without duplicate broad unittest discovery, scoped trigger paths.
- `docs/decisions/D-171-change-aware-ci-gate-scheduling.md`: successor to D-014's every-PR scheduling clause only.
- `docs/decisions/README.md`: regenerated decision index.
- `AGENTS.md`, `docs/TESTING.md`, `docs/REPOSITORY_GOVERNANCE.md`, and `docs/ROADMAP.md`: normative/current-state contract and follow-up measurement pointer.
- `docs/superpowers/specs/2026-08-15-ci-feedback-routing-design.md`: approved architecture and fixed baseline.
- `docs/sessions/2026-08-15-01-issue-558-ci-stage.md`, `2026-08-15-02-issue-558-checker-activation.md`, and `2026-08-15-03-issue-558-ci-activation.md`: fresh checkpoints for the three phases, with live state re-fetched before commit. If a concurrent merge claims a sequence first, increment the affected filename before creating it.

---

### Task 1: Stage PR — classifier behavior, first

**Files:**
- Create: `scripts/test_classify_ci_changes.py`
- Create: `scripts/classify_ci_changes.py`

**Interfaces:**
- Consumes: a sequence of repository-relative changed paths and event name `pull_request` or `push`.
- Produces: `Selection(compiler: bool, pages: bool, agent: bool)`, `classify_paths(paths, event_name)`, and CLI outputs named `compiler`, `pages`, and `agent`.

- [ ] **Step 1: Write the failing functional tests**

  Add literal cases before the implementation exists. Each expected value is hand-derived:

  ```python
  def test_docs_only_skips_every_heavy_category(self):
      self.assertEqual(
          Selection(False, False, False),
          classify_paths(["docs/TESTING.md"], event_name="pull_request"),
      )

  def test_unknown_top_level_path_selects_everything(self):
      self.assertEqual(
          Selection(True, True, True),
          classify_paths(["future-build-input.toml"], event_name="pull_request"),
      )

  def test_mixed_site_and_compiler_change_unions_categories(self):
      self.assertEqual(
          Selection(True, True, True),
          classify_paths(
              ["crates/pycc_hir/src/lib.rs", "site/index.html"],
              event_name="pull_request",
          ),
      )
  ```

  Cover `src/**`, `crates/**`, Cargo/lock/toolchain/build files, the executable
  `docs/DIAGNOSTICS.md` registry input, `tests/**`, `benches/**`,
  compiler/performance scripts, `site/**`, every Lighthouse
  validator/fixture, agent roots, policy-successor fixtures, general docs,
  other workflow files, `.github/workflows/ci.yml`, classifier/test self
  changes, empty input, absolute/parent paths, embedded NUL/newline, added,
  deleted, and renamed path pairs.

- [ ] **Step 2: Run the classifier test and verify RED**

  Run: `python3 -B scripts/test_classify_ci_changes.py`

  Expected: import failure because `scripts/classify_ci_changes.py` does not
  exist. A syntax error in the test is not the expected failure.

- [ ] **Step 3: Implement the minimal pure classifier**

  Use an immutable dataclass and explicit predicates. The default branch is
  full selection:

  ```python
  @dataclass(frozen=True)
  class Selection:
      compiler: bool
      pages: bool
      agent: bool

      @classmethod
      def full(cls) -> "Selection":
          return cls(True, True, True)

  def classify_paths(paths: Sequence[str], *, event_name: str) -> Selection:
      if event_name == "push":
          return Selection.full()
      if event_name != "pull_request" or not paths:
          return Selection.full()
      # Validate each repository-relative path, union reviewed categories,
      # and return Selection.full() immediately for an unmatched path.
  ```

  Treat general `scripts/**` changes as governance inputs, but explicitly
  select compiler or Pages for scripts consumed by those gates. Every compiler
  input selects both `compiler` and `agent`, because the offline alpha evals
  execute the fresh compiler and bind D-072 diagnostics/backend behavior. This remains
  safe for a newly added CI script because changing `ci.yml` itself selects the
  complete topology; checker tests must bind every existing executed heavy-gate
  script to a reviewed category.

- [ ] **Step 4: Verify GREEN and mutation resistance**

  Run: `python3 -B scripts/test_classify_ci_changes.py`

  Then temporarily change the unknown-path return to an empty selection, run
  the focused unknown-path test and observe failure, restore the implementation,
  and rerun the full file green.

- [ ] **Step 5: Add failing CLI tests**

  Exercise the real subprocess with NUL-delimited stdin. Assert exact output:

  ```text
  compiler=false
  pages=true
  agent=false
  ```

  Cover `push` selecting all, missing terminal NUL selecting all, an invalid
  output path exiting non-zero, and no path text being echoed to output.

- [ ] **Step 6: Implement and verify the CLI**

  Parse `--event-name` and `--github-output`, require NUL termination for a
  non-empty pull-request stream, write with exclusive append semantics, and
  emit only the three fixed keys with lowercase booleans.

  Run:

  ```bash
  python3 -B scripts/test_classify_ci_changes.py
  python3 -B -m unittest discover -s scripts -p 'test_*.py'
  ```

  Expected: both commands exit 0.

### Task 2: Stage PR — construct the final CI workflow

**Files:**
- Create: `tests/fixtures/policy-successors/ci-d171.yml`
- Modify later by byte copy only: `.github/workflows/ci.yml`

**Interfaces:**
- Consumes: `classify-changes.outputs.{compiler,pages,agent}`.
- Produces: the stable `ci-gate` check and unchanged heavy-gate evidence when selected.

- [ ] **Step 1: Copy current CI into the inert successor**

  Copy `.github/workflows/ci.yml` to
  `tests/fixtures/policy-successors/ci-d171.yml` using a normal file copy, then
  edit only the inert successor. The active workflow must remain byte-identical
  to `origin/main` throughout the stage PR.

- [ ] **Step 2: Add PR-only concurrency and the classifier job**

  Add:

  ```yaml
  concurrency:
    group: ci-${{ github.event_name == 'pull_request' && github.event.pull_request.number || github.run_id }}
    cancel-in-progress: ${{ github.event_name == 'pull_request' }}
  ```

  Add `classify-changes` with `contents: read`, credential-free full-history
  checkout, exact event SHAs, `git cat-file -e` validation, `git diff
  --no-renames --name-only --diff-filter=ACDMRTUXB -z "$base_sha" "$head_sha"`,
  retaining the exact base/head range, and the classifier CLI. It exports the
  three fixed outputs.

- [ ] **Step 3: Add the single required governance job**

  Move broad Python policy discovery plus workflow/roadmap/README policy checks
  out of `build-test-coverage` into `governance`. Keep unique validation
  commands unchanged. Run the compiler build and both alpha skill evals only
  when `agent == 'true'`. Give `governance` the exact job-level condition
  `${{ !cancelled() }}` so classifier failure does not suppress the required
  policy evidence while PR concurrency cancellation still stops obsolete work.

- [ ] **Step 4: Condition every heavy job**

  Add `needs: classify-changes` and job-level `if` to coverage, native matrix,
  cross build/verify, and frontend measure/gate for `compiler == 'true'`.
  Add the equivalent Pages condition to both Lighthouse jobs. Preserve all
  existing command and gate bodies inside each selected job byte-for-byte
  except for the policy commands moved in Step 3. The two inherited mutable
  `actions/checkout@v4` steps in `native-build-test` and
  `cross-compile-build` are the explicit structural exception: pin both to
  the repository's reviewed checkout v6 SHA and set
  `persist-credentials: false`, as required by the global untrusted-input
  invariant.

- [ ] **Step 5: Implement the exact `ci-gate` truth table**

  Add `classify-changes` and `governance` to `needs`. The fail step must reject
  missing/non-boolean outputs, require both required jobs to succeed, require
  `success` for each selected optional job, and require `skipped` for each
  unselected optional job. Keep `if: always()`, Ubuntu, and `permissions: {}`.

- [ ] **Step 6: Validate YAML and unchanged gate bytes locally**

  Parse the fixture with Ruby's safe YAML loader used by the checker. Diff the
  coverage shell body, Tier-1 matrix values, paired-performance commands,
  artifact boundaries, Pages commands, and thresholds against active
  `ci.yml`; only routing, moved governance steps, and the two explicit
  checkout-hardening changes may differ.

### Task 3: Stage PR — extend the trusted checker with tests first

**Files:**
- Create: `tests/fixtures/policy-successors/test_check_roadmap_evidence-d171.rb`
- Create: `tests/fixtures/policy-successors/check_roadmap_evidence-d171.rb`

**Interfaces:**
- Consumes: exact final `ci-d171.yml` bytes.
- Produces: acceptance of current main and D-171 during coexistence, plus fail-closed structural mutation checks.

- [ ] **Step 1: Copy current checker self-test and write RED mutations**

  Copy the live checker self-test to the D-171 successor and point its staged
  workflow fixture constant at `ci-d171.yml`. Add tests that mutate one property
  at a time: classifier permission/checkout/diff; every workflow checkout made
  mutable or changed to persist credentials; governance changed back to
  `always()` or otherwise made cancellation-incompatible; each optional job
  condition; missing classifier/governance dependency; every selected/skipped
  result branch; malformed/missing output; PR cancellation disabled; main
  cancellation enabled; coverage threshold/workspace/sandbox changes; matrix
  leg removal; performance provenance drift; and Pages gate removal.

- [ ] **Step 2: Verify RED against a copied unmodified checker**

  Copy the live checker to the D-171 successor and run:

  `ruby tests/fixtures/policy-successors/test_check_roadmap_evidence-d171.rb`

  Expected: new D-171 tests fail because the digest and topology are unknown.

- [ ] **Step 3: Implement minimal D-171 checker support**

  Add the SHA-256 of the checked-in `ci-d171.yml` to the reviewed coexistence
  array. Add an exact D-171 routing validator that recognizes the classifier,
  optional-job conditions, concurrency, and aggregate truth table while
  delegating unchanged coverage/performance/Pages checks to their existing
  validators. Do not broaden acceptance for older workflow digests.

- [ ] **Step 4: Verify GREEN and the live-workflow compatibility path**

  Run:

  ```bash
  ruby tests/fixtures/policy-successors/test_check_roadmap_evidence-d171.rb
  ruby scripts/test_check_roadmap_evidence.rb
  ruby scripts/check_roadmap_evidence.rb
  ```

  Expected: staged tests and current live checker tests all exit 0; current
  `.github/workflows/ci.yml` remains accepted during the coexistence window.

### Task 4: Stage PR — policy manifest, ADR, and current-state docs

**Files:**
- Create: `docs/decisions/D-171-change-aware-ci-gate-scheduling.md`
- Modify: `docs/decisions/README.md`
- Modify: `tests/fixtures/policy-successor-manifest.json`
- Modify: `docs/TESTING.md`
- Already created: design and plan files

**Interfaces:**
- Consumes: exact staged file digests.
- Produces: a D-103-valid transition with live `ci.yml`/checker unchanged.

- [ ] **Step 1: Write D-171 and stage-state documentation**

  Record that only D-014's every-PR scheduling sentence is superseded. State
  that thresholds, sandbox, workspace, exemptions, Tier-1, perf, and Pages
  requirements are unchanged. In `docs/TESTING.md`, describe the proposal as
  staged, not active.

- [ ] **Step 2: Regenerate the decision index**

  Run: `python3 scripts/generate_decisions_index.py`

  Confirm `docs/decisions/README.md` contains D-171 once, status `accepted`.

- [ ] **Step 3: Update the complete D-103 manifest**

  Add the live classifier and test as steady-state protected targets. Point
  only the existing checker and checker self-test entries at their D-171
  proposal files with exact lowercase SHA-256 values. Keep the live CI target
  self-sourced at its existing digest, and add `ci-d171.yml` as a steady-state
  protected transitive target. Add other proposal files as protected inputs
  only where the current manifest convention requires it. Keep every existing
  target and entry unchanged otherwise.

- [ ] **Step 4: Verify the stage tree against the base-owned checker**

  Run:

  ```bash
  ruby scripts/test_check_ci_permissions.rb
  ruby scripts/check_ci_permissions.rb
  ruby scripts/test_check_roadmap_evidence.rb
  ruby scripts/check_roadmap_evidence.rb
  python3 -B -m unittest discover -s scripts -p 'test_*.py'
  python3 scripts/validate_agent_policies.py
  ```

  Also run the trusted audit locally with a temporary candidate tree if the
  checker provides that fixture helper. First prove Stage 1 passes against the
  `origin/main` base while every active protected target is unchanged. Then
  treat the Stage 1 tree and manifest as the trusted base: prove a candidate
  can copy only the checker and self-test successors, return those two entries
  to self-source, and stage the still-unchanged CI target from `ci-d171.yml`.
  Also prove premature CI activation and omitted checker copies fail closed.

- [ ] **Step 5: Self-review and commit the stage deliverable**

  Verify no placeholder text, run `git diff --check`, stage every new file, run
  the pinned deep reviewer, resolve actionable findings, then commit focused
  changes. Do not include `Fixes #558` in the eventual PR body.

### Task 5: Stage PR — full local gates, publish, monitor, merge

**Files:**
- Create: `docs/sessions/2026-08-15-01-issue-558-ci-stage.md` (or the next free same-day sequence if a concurrent merge claims `01` first)

- [ ] **Step 1: Run every locally reproducible current-CI command**

  Use the exact commands in the active `.github/workflows/ci.yml`, including
  prepared workspace builds/tests, exact isolated coverage when the local macOS
  toolchain supports it, clippy with `-D warnings`, Python suites and
  validators, Ruby policy/evidence tests, alpha evals, CI-monitoring shell
  tests, and both marketplace validators. Record any platform-only command as
  CI-only rather than claiming local execution.

- [ ] **Step 2: Re-fetch and reconcile**

  Re-fetch `origin/main`, re-list every open PR and their changed files, and
  rebase only this session's commits if main moved. Rerun affected gates. Stop
  if another PR now overlaps the protected successor sequence.

- [ ] **Step 3: Create the live session checkpoint**

  Re-fetch immediately before writing the checkpoint. Record exact base/head,
  #558 state, #518 state/head, staged targets, verification, and the next
  activation step. Commit it on the non-main branch.

- [ ] **Step 4: Push and open Stage 1/3 PR**

  Use a temporary body file and `gh pr create --body-file`. Body text must say
  `Stage 1/3 for #558`, explain that live checker/CI bytes are unchanged, link
  the design/plan, include tests, and omit `Fixes #558`.

- [ ] **Step 5: Monitor and merge**

  Verify live state, head, mergeability, required checks, and unresolved
  threads before every wait. Address findings through the deep-review loop.
  With `audit` and `ci-gate` green and zero unresolved threads, reread the full
  diff, merge with a merge commit, delete the remote branch, fetch, and verify
  the merge commit on `origin/main`.

### Task 6: Checker activation PR

**Files:**
- Modify by byte copy: `scripts/check_roadmap_evidence.rb`
- Modify by byte copy: `scripts/test_check_roadmap_evidence.rb`
- Modify: `tests/fixtures/policy-successor-manifest.json`
- Modify: `docs/TESTING.md`
- Create: next session checkpoint

- [ ] **Step 1: Start a fresh branch from the exact post-stage main**

  Fetch, confirm the stage merge, ensure no manifest entry other than the
  intended D-171 transition is unexpectedly mid-transition, run bypass status
  and `cargo doc --workspace --no-deps`.

- [ ] **Step 2: Activate checker and test byte-for-byte**

  Copy the two base-owned successors to their live paths. Verify with `cmp` and
  SHA-256. Return those two manifest entries to `source_path == path` with their
  active digests, and redirect the still-unchanged CI target to the base-owned
  `ci-d171.yml` digest. Do not modify the live CI workflow or either standalone
  agent workflow in this PR.

- [ ] **Step 3: Update phase-state docs and run full local gates**

  State that the checker is active and CI is still staged. Run both live Ruby
  suites/checkers, Python suite/validators, build/test/coverage/clippy, asset
  validations, and `git diff --check`.

- [ ] **Step 4: Deep-review, publish Stage 2/3 PR, monitor, and merge**

  Body: `Intermediate checker activation for #558 (step 2/3)`; no `Fixes`.
  Follow the same live-state, review-thread, CI, full-diff, merge-commit, and
  post-merge verification gates as Task 5.

### Task 7: Final CI activation PR

**Files:**
- Modify by byte copy: `.github/workflows/ci.yml`
- Modify: `tests/fixtures/policy-successor-manifest.json`
- Modify: `.github/workflows/agent-policy.yml`
- Modify: `.github/workflows/agent-assets.yml`
- Modify: `AGENTS.md`
- Modify: `docs/TESTING.md`
- Modify: `docs/REPOSITORY_GOVERNANCE.md`
- Modify: `docs/ROADMAP.md`
- Create: next session checkpoint

- [ ] **Step 1: Start from exact post-checker main and revalidate overlap**

  Fetch, confirm the checker activation merge and CI successor binding, inspect
  all open PRs, run bypass status and `cargo doc --workspace --no-deps`.

- [ ] **Step 2: Activate CI byte-for-byte**

  Copy `ci-d171.yml` to `.github/workflows/ci.yml`, prove `cmp` equality, and
  return the CI manifest entry to `source_path == path` with that active digest.
  Do not edit the live CI file after the byte comparison; any required change
  restarts staging. The standalone workflow cleanup and final active-state docs
  remain later steps in this same final PR, not part of checker activation.

- [ ] **Step 3: Remove only duplicated standalone-workflow discovery**

  Keep every unique agent-policy/asset, monitoring, pinned CLI, and marketplace
  command. Remove only the broad `python3 -m unittest discover` duplication and
  add path triggers covering each workflow's inputs plus the workflow file
  itself.

- [ ] **Step 4: Update active normative/current-state documentation**

  `AGENTS.md` and `docs/TESTING.md` must say coverage is mandatory for every
  compiler-relevant PR selected by the fail-closed classifier and every main
  push. `docs/REPOSITORY_GOVERNANCE.md` must describe the exact new fan-in and
  current staged digest. `docs/ROADMAP.md` records #558's active CI routing and
  the deferred 20-PR measurement without claiming an outcome.

- [ ] **Step 5: Run classifier replay and the entire local gate set**

  Replay the preserved #519-#557 changed-file sets through the classifier as a
  diagnostic and record which gates each would select. This does not replace
  the later outcome sample. Run every command from Task 5 plus the live D-171
  checker mutation suite and YAML validation.

- [ ] **Step 6: Deep-review, publish final PR, monitor, and merge**

  Use `Fixes #558` only here. Verify the activation PR selects the complete CI
  topology because it changes `ci.yml` and classifier-protected inputs. Merge
  with a merge commit only after `audit`, `ci-gate`, every selected dependency,
  and every review thread are green/resolved. Fetch and verify issue #558 closed
  and the exact activated bytes on `origin/main`.

### Task 8: Preserve the post-activation measurement checkpoint

**Files:**
- No repository file required unless the final session checkpoint needs a link.

- [ ] **Step 1: Create the bounded follow-up issue**

  Priority: P2, no milestone because it is cross-cutting measurement. Title:
  `P2: Re-measure PR CI after 20 post-activation merges`.

  Record the fixed baseline, final activation PR/merge commit/time, the first
  eligible subsequent PR, exact definitions for PR lifetime/final CI/attempts/
  runner-minutes/failure attribution, and the trigger: 20 merged PRs after the
  activation merge. Do not label it `in progress` before the threshold is met.

- [ ] **Step 2: Final post-merge verification**

  Verify branch protection still exactly requires `audit` and `ci-gate`, the
  main-history audit succeeded for every merge, the final CI workflow matches
  its staged fixture, #558 is closed, and the follow-up issue is open. Report
  delivery and the future measurement trigger without claiming speedup.
