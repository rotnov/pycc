# Non-blocking CI Policy Audit Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Recover issue #558 in one pull request by activating change-aware CI, replacing exact-byte successor gating with a trusted property-based audit, and preserving every required security, coverage, and Tier-1 gate.

**Architecture:** The required `audit` continues to execute the current base revision under `pull_request_target` and treats candidate workflows as non-executable data. The base checker validates explicit YAML security and routing properties; it no longer requires general CI/checker files to match a predecessor-staged SHA. The current deadlock is recovered with one owner-authorized, one-use D-024 relaxation of `audit`; `ci-gate` is never relaxed.

**Tech Stack:** GitHub Actions YAML, Ruby 3/Psych/Minitest policy checkers, Python 3 unittest policy/classifier checks, Rust/Cargo, GitHub CLI/API.

## Global Constraints

- Work only in `/tmp/pycc-ci-optimization`; do not modify `/Users/denis/.codex/worktrees/0764/pycc-proto`.
- Start from branch `codex/activate-change-aware-ci`, design commits `bbf2444` and `27e5af1a`, and base `origin/main` `9725f189c9fbc7d8cfcc436a2e6230fcd6b87258` unless a fresh fetch proves `main` advanced.
- Preserve the existing uncommitted Task 7 edits in `.github/workflows/ci.yml`, both agent workflows, `AGENTS.md`, three governance/testing/roadmap documents, and the successor manifest. Never discard or overwrite them with a checkout/reset.
- Issue #558 is P1/release-blocking, remains open, assigned to `rotnov`, and labeled `enhancement` plus `in progress` until the recovery and 20-PR measurement are complete.
- Open PR #560 overlaps `.github/workflows/ci.yml`, roadmap checker/tests, manifest, AGENTS, and docs. Do not edit #560; refresh its head before publication and report that it must rebase/renumber after #558.
- Keep macOS Intel Tier-1 coverage. The D-171 native matrix continues to contain `macos-15-intel` / `x86_64-apple-darwin`, and cross verification continues on `macos-15-intel`.
- Keep `audit` and `ci-gate` required, strict up-to-date protection enabled, administrators enforced, conversations resolved, and force-push/deletion disabled outside the single documented D-024 window.
- The D-024 authorization is one-use, one recovery PR, `audit` only, at most one merge and ten minutes. Never relax `ci-gate`.
- `tests/fixtures/policy-successors/ci-d171.yml` remains immutable historical D-171 evidence. The active workflow uses its routing but pins the remaining mutable Action references.
- Use these reviewed Action tag resolutions captured from the upstream GitHub tag refs on 2026-08-17:

  ```text
  actions/checkout@v6                 d23441a48e516b6c34aea4fa41551a30e30af803
  actions/configure-pages@v5          983d7736d9b0ae728b81ab479565c72886d7745b
  actions/deploy-pages@v4             d6db90164ac5ed86f2b6aed7e0febac5b3c0c03e
  actions/download-artifact@v4        d3f86a106a0bac45b974a628896c90dbdf5c8093
  actions/setup-node@v4               49933ea5288caeca8642d1e84afbd3f7d6820020
  actions/setup-python@v5             a26af69be951a213d495a4c3e4e4022e16d87065
  actions/upload-artifact@v4          ea165f8d65b6e75b540449e92b4886f43607fa02
  actions/upload-pages-artifact@v4    7b1f4a764d45c48632c6b24a0339c27f5614fb0b
  ilammy/msvc-dev-cmd@v1              0b201ec74fa43914dc39ae48a89fd1d8cb592756
  ```

- Preserve 100% line and region coverage, the isolated `nobody` boundary, exact CPython 3.14.7 CI oracle, performance provenance, Pages/accessibility gates, classifier fail-closed behavior, PR-only cancellation, and all required gate truth-table branches.
- Do not claim an improvement until 20 pull requests merge after the recovery merge SHA. Keep the fixed #519-#557 baseline unchanged.

---

### Task 1: Retire the forced exact-successor merge gate

**Files:**
- Modify: `scripts/test_check_ci_permissions.rb`
- Modify: `scripts/check_ci_permissions.rb`

**Interfaces:**
- Consumes: the existing `main(arguments)` workflow parser and `validate_workflow(text, source)` structural validator.
- Produces: `main(arguments) -> Integer` that validates workflow structure without consulting `POLICY_CANDIDATE_ROOT` or requiring any candidate protected target to equal a base-staged successor.

- [ ] **Step 1: Add a failing integration test for the non-blocking audit path**

  Add an environment-restoration helper and this test to `scripts/test_check_ci_permissions.rb`:

  ```ruby
  def with_environment(overrides)
    previous = overrides.to_h { |name, _value| [name, ENV[name]] }
    overrides.each { |name, value| ENV[name] = value }
    yield
  ensure
    previous.each do |name, value|
      value.nil? ? ENV.delete(name) : ENV[name] = value
    end
  end

  def test_pull_request_target_main_does_not_consult_retired_successor_state
    status = with_environment(
      "GITHUB_EVENT_NAME" => "pull_request_target",
      "POLICY_CANDIDATE_ROOT" => "/definitely/missing/policy-candidate"
    ) do
      main([WORKFLOW_DIRECTORY.to_s])
    end

    assert_equal 0, status
  end
  ```

  Extend the existing trust-anchor test to assert that every executed checker
  command is rooted at `scripts/` in the base checkout and that no `run` command
  executes `/tmp/pr-policy-input/scripts/...`; candidate checker blobs may be
  downloaded as data but never executed.

- [ ] **Step 2: Run the focused test and record RED**

  Run:

  ```bash
  ruby scripts/test_check_ci_permissions.rb --name /retired_successor_state/
  ```

  Expected: FAIL because the current `validate_steady_state_policy_inputs` path attempts to read the missing candidate manifest and `main` returns `1`.

- [ ] **Step 3: Remove the obsolete steady-state and activation machinery**

  In `scripts/check_ci_permissions.rb`:

  - remove `require "json"` and `require "open3"` once their last users are removed;
  - remove `StrictJsonObject`;
  - retain `Digest`, `TRUST_ANCHOR_FILENAME`, `TRUST_ANCHOR_SHA256_ALLOWLIST`, the AST helpers, workflow discovery, trust-anchor digest check, permission checks, and `validate_workflow`;
  - remove the `SEARCH_*`, `POLICY_SUCCESSOR_*`, and `ACTIVATED_POLICY_*` constants used only by the retired transition;
  - remove `replace_exact_once`, `activated_successor_executable`, `parse_policy_successor_manifest`, `regular_policy_input`, `expected_activated_policy_successor_manifest`, `validate_policy_successor_transition`, `validate_steady_state_policy_inputs`, `git_attributes_manifest`, `activation_tree_metadata`, `pull_request_head_data`, and the uncalled `validate_search_activation_transition`;
  - delete the `validate_steady_state_policy_inputs` call from `main`.

  In `scripts/test_check_ci_permissions.rb`:

  - remove `require "fileutils"` and `write_policy_input` after their last
    transition-test users are deleted;
  - remove `successor_manifest` and `successor_entry`;
  - remove the exact-transition tests from `test_steady_state_policy_accepts_an_unchanged_protected_target` through `test_policy_successor_manifest_rejects_float_version_and_duplicate_keys`;
  - replace `POLICY_SUCCESSOR_MANIFEST_PATH.inspect` in the trust-anchor data-download test with the literal `"tests/fixtures/policy-successor-manifest.json".inspect`; the workflow may continue using the manifest as a bounded input inventory, but Ruby no longer treats it as activation authority.

- [ ] **Step 4: Run the focused and complete permission-policy suite**

  Run:

  ```bash
  ruby scripts/test_check_ci_permissions.rb --name /retired_successor_state/
  ruby scripts/test_check_ci_permissions.rb
  ruby scripts/check_ci_permissions.rb
  ```

  Expected: all commands exit `0`; the focused test passes even with a nonexistent `POLICY_CANDIDATE_ROOT`.

- [ ] **Step 5: Commit only Task 1**

  ```bash
  git add scripts/check_ci_permissions.rb scripts/test_check_ci_permissions.rb
  git diff --cached --check
  git commit -m "ci: retire forced policy successor activation"
  ```

---

### Task 2: Activate D-171 under property-based security and routing checks

**Files:**
- Modify: `scripts/test_check_ci_permissions.rb`
- Modify: `scripts/check_ci_permissions.rb`
- Modify: `scripts/test_check_roadmap_evidence.rb`
- Modify: `scripts/check_roadmap_evidence.rb`
- Modify: `.github/workflows/ci.yml`
- Modify: `.github/workflows/agent-policy.yml`
- Modify: `.github/workflows/agent-assets.yml`
- Modify: `.github/workflows/hook-install-check.yml`
- Modify: `.github/workflows/link-check.yml`
- Modify: `.github/workflows/main-history-audit.yml`
- Modify: `.github/workflows/pages.yml`
- Modify: `tests/fixtures/policy-successor-manifest.json`
- Verify unchanged: `tests/fixtures/policy-successors/ci-d171.yml`

**Interfaces:**
- Produces: `validate_action_reference(node, context) -> nil`, rejecting every non-local Action reference that is not a full 40-character lowercase commit SHA.
- Produces: `validate_job_action_references(entries, context) -> nil`, validating job-level reusable workflows, every step-level Action, and checkout credential persistence.
- Produces: `d171_named_step(job, name, context) -> Hash`, returning exactly one named required step or raising `RoadmapEvidenceError`.
- Produces: `d171_require_failure_propagation(mapping, context) -> nil`, accepting absent/`false` `continue-on-error` and rejecting every suppressing value.
- Produces: `d171_routed_workflow?(workflow_text, source) -> bool`, selecting property-based D-171 validation whenever either D-171 routing job name is present.
- Preserves: `validate_d171_ci_routing(workflow_text, source) -> true`, now binding required properties rather than complete workflow/job hashes.

- [ ] **Step 1: Add RED mutation tests for Action pins and checkout credentials**

  Add these cases to `scripts/test_check_ci_permissions.rb` using the existing `workflow` helper:

  ```ruby
  def test_rejects_mutable_and_short_action_references
    ["actions/setup-python@v5", "actions/setup-python@main",
     "actions/setup-python@a26af69"].each do |reference|
      text = workflow("runs-on: ubuntu-latest\nsteps:\n  - uses: #{reference}")
      error = assert_raises(PolicyError) { validate_workflow(text) }
      assert_match(/full commit SHA/, error.message)
    end
  end

  def test_accepts_full_sha_and_local_action_references
    validate_workflow(workflow(<<~YAML))
      runs-on: ubuntu-latest
      steps:
        - uses: actions/setup-python@a26af69be951a213d495a4c3e4e4022e16d87065
        - uses: ./local-action
    YAML
  end

  def test_rejects_mutable_reusable_workflow_reference
    error = assert_raises(PolicyError) do
      validate_workflow(workflow("uses: owner/repo/.github/workflows/test.yml@main"))
    end
    assert_match(/full commit SHA/, error.message)
  end

  def test_checkout_requires_non_persisted_credentials_even_with_case_drift
    [
      "steps:\n  - uses: actions/checkout@d23441a48e516b6c34aea4fa41551a30e30af803",
      "steps:\n  - uses: Actions/Checkout@d23441a48e516b6c34aea4fa41551a30e30af803\n    with:\n      persist-credentials: true"
    ].each do |steps|
      error = assert_raises(PolicyError) do
        validate_workflow(workflow("runs-on: ubuntu-latest\n#{steps}"))
      end
      assert_match(/persist-credentials.*false/, error.message)
    end
  end
  ```

- [ ] **Step 2: Add RED acceptance tests proving D-171 is property-bound**

  In `scripts/test_check_roadmap_evidence.rb`, add a helper that calls both `validate_d171_ci_routing` and `run_checker`, then add these accepted mutations:

  ```ruby
  def assert_d171_routing_accepted(workflow, label)
    assert validate_d171_ci_routing(workflow.to_yaml, "#{label}.yml")
    _stdout, stderr, status = run_checker(
      roadmap: "# pycc Roadmap\n",
      workflow: workflow.to_yaml
    )
    assert status.success?, "#{label}: #{stderr}"
  end

  def test_d171_accepts_safe_ordinary_job_body_changes
    workflow = d171_workflow
    workflow.dig("jobs", "governance")["timeout-minutes"] = "10"
    workflow.dig("jobs", "governance", "steps") << {
      "name" => "Additional read-only diagnostic",
      "run" => "true"
    }
    assert_d171_routing_accepted(workflow, "safe-governance-extension")
  end

  def test_d171_accepts_an_unrequired_read_only_job
    workflow = d171_workflow
    workflow.fetch("jobs")["informational"] = {
      "runs-on" => "ubuntu-latest",
      "permissions" => {},
      "steps" => [{ "run" => "true" }]
    }
    assert_d171_routing_accepted(workflow, "informational-job")
  end
  ```

  Update the old extra-structure test so it still rejects an unexpected top-level key but no longer expects a safe extra job, timeout, or harmless governance step to fail.

- [ ] **Step 3: Run focused tests and record RED**

  ```bash
  ruby scripts/test_check_ci_permissions.rb --name '/action|checkout|reusable/'
  ruby scripts/test_check_roadmap_evidence.rb --name '/safe_ordinary|unrequired_read_only/'
  ```

  Expected: Action tests fail because mutable tags are currently accepted; D-171 acceptance tests fail on exact job-set/body hashes and exact active workflow digest routing.

- [ ] **Step 4: Implement the Action-reference validator**

  Add these helpers to `scripts/check_ci_permissions.rb` and call `validate_job_action_references` for every parsed job before privilege evaluation:

  ```ruby
  FULL_ACTION_COMMIT = /\A(?:[A-Za-z0-9_.-]+\/)+(?:[A-Za-z0-9_.-]+)@[0-9a-f]{40}\z/

  def sequence_entries(node, context)
    raise PolicyError, "#{context} must be a sequence" unless node.is_a?(Psych::Nodes::Sequence)
    node.children
  end

  def validate_action_reference(node, context)
    reference = scalar_value(node, context).strip
    return reference if reference.start_with?("./")
    return reference if FULL_ACTION_COMMIT.match?(reference)

    raise PolicyError, "#{context} must use a full commit SHA"
  end

  def validate_job_action_references(entries, context)
    validate_action_reference(entries["uses"], "#{context} reusable workflow") if entries["uses"]
    return unless entries["steps"]

    sequence_entries(entries["steps"], "#{context} steps").each_with_index do |step_node, index|
      step = mapping_entries(step_node, "#{context} step #{index}")
      next unless step["uses"]

      reference = validate_action_reference(step["uses"], "#{context} step #{index} action")
      next unless reference.split("@", 2).first.downcase == "actions/checkout"

      with = step["with"] && mapping_entries(step["with"], "#{context} checkout inputs")
      persisted = with && with["persist-credentials"] &&
                  scalar_value(with["persist-credentials"], "#{context} checkout persist-credentials")
      unless persisted == "false"
        raise PolicyError, "#{context} checkout must set persist-credentials to false"
      end
    end
  end
  ```

- [ ] **Step 5: Pin every live mutable Action and harden every checkout**

  Replace every `uses: ...@vN` found by

  ```bash
  rg -n 'uses:\s+[^ #]+@v[0-9]' .github/workflows
  ```

  with the exact SHA from Global Constraints. Convert every old checkout to the reviewed v6 SHA and add:

  ```yaml
  with:
    persist-credentials: false
  ```

  Pin the five mutable references inherited by active D-171 routing (`setup-python` twice, `msvc-dev-cmd`, cross-build `upload-artifact`, and cross-verify `download-artifact`) without editing `tests/fixtures/policy-successors/ci-d171.yml`. Pin the remaining mutable references in hook-install, link-check, agent-assets, Pages, and main-history-audit workflows. Keep `.github/workflows/workflow-policy.yml` byte-identical to its approved trust-anchor digest.

- [ ] **Step 6: Replace D-171 whole-body hashes with named properties**

  In `scripts/check_roadmap_evidence.rb`:

  - retain the historical `D171_CHANGE_AWARE_CI_WORKFLOW_SHA256` and fixture digest assertion;
  - replace `D171_JOB_NAMES` equality with a required-name subset check;
  - remove `D171_JOB_BODY_SHA256S`, `d171_canonical_value`, and `d171_canonical_sha256` from active validation;
  - scan every checkout present rather than comparing exact checkout counts;
  - keep the classifier steps, outputs, exact base/head bindings, concurrency, optional routing conditions/dependencies, Tier-1 matrix, and `ci-gate` truth table exact;
  - require each required job to propagate failures;
  - require the four governance policy steps exactly once, with their reviewed `run` blocks and no step-level `if`; allow additional unprivileged steps;
  - keep the three agent-only governance steps conditional on `agent == 'true'`;
  - require the cross-build upload and cross-verify download Actions at their reviewed SHAs, exact artifact name/path, `macos-14` build runner, `macos-15-intel` verification runner, and the exact native-output check that exits unless output is `42`;
  - continue delegating coverage, performance provenance, Pages performance, and Pages accessibility to their existing property validators.

  Implement the helpers with these contracts:

  ```ruby
  def d171_named_step(job, name, context)
    steps = d171_sequence(job["steps"], "#{context} steps")
    matches = steps.select { |step| step.is_a?(Hash) && step["name"] == name }
    d171_require_equal(matches.length, 1, "#{context} step #{name.inspect}")
    matches.first
  end

  def d171_require_failure_propagation(mapping, context)
    return unless mapping.key?("continue-on-error")
    d171_require_equal(mapping["continue-on-error"], "false", "#{context} failure propagation")
  end
  ```

- [ ] **Step 7: Route active D-171 by structure rather than full digest**

  Add `d171_routed_workflow?` using the same Psych AST conversion as the validator. Return true when either `classify-changes` or `governance` is present, so deleting both cannot fall through to a permissive legacy path: an unknown digest still fails the historical allowlist.

  Change `validate_evidence` to:

  ```ruby
  if d171_routed_workflow?(workflow_text, workflow.to_s)
    validate_d171_ci_routing(workflow_text, workflow.to_s)
    return
  end
  ```

  before the legacy reviewed-digest branch. Preserve exact digests only for immutable historical workflow fixtures.

- [ ] **Step 8: Fix the five stale activation assertions without weakening gates**

  In `scripts/test_check_roadmap_evidence.rb`:

  - replace `test_tier1_workflow_authorization_is_in_the_python_3147_transition` with an assertion that live CI passes `validate_d171_ci_routing`;
  - make `test_python_3147_transition_workflow_is_active_and_reviewed` retain the old fixture digest assertions but validate live CI as D-171 routing;
  - replace `test_live_ci_yml_is_an_exact_python_3147_transition_shape` with `test_live_ci_yml_uses_active_d171_routing`;
  - read roadmap-policy commands from `jobs.governance.steps` in `test_repository_ci_runs_the_self_tests_and_checker`, while keeping coverage/toolchain/sandbox/build-order assertions against `jobs.build-test-coverage.steps`;
  - change `test_rejects_changed_tier1_matrix_workflow` to assert the property-specific `Tier-1 strategy` diagnostic;
  - retain every negative mutation for permissions, checkout credentials, classifier bindings, cancellation, routing conditions, gate truth table, coverage, matrix legs, performance provenance, Pages commands, required governance commands, artifact transfer, and native Intel verification.

- [ ] **Step 9: Reconcile the active workflow and manifest**

  Preserve the existing Task 7 classifier/governance/conditional-job/`ci-gate` diff. Preserve the scoped agent-policy/agent-assets paths and removal of duplicate broad Python discovery. Compute the hardened live CI digest and update only its self-sourced manifest entry:

  ```bash
  shasum -a 256 .github/workflows/ci.yml
  ```

  The entry must have both `path` and `source_path` equal to `.github/workflows/ci.yml`. Verify the historical fixture did not change:

  ```bash
  test "$(git hash-object tests/fixtures/policy-successors/ci-d171.yml)" = \
    "$(git rev-parse origin/main:tests/fixtures/policy-successors/ci-d171.yml)"
  ```

- [ ] **Step 10: Run the core GREEN suite**

  ```bash
  ruby scripts/test_check_ci_permissions.rb
  ruby scripts/check_ci_permissions.rb
  ruby scripts/test_check_roadmap_evidence.rb
  ruby scripts/check_roadmap_evidence.rb
  python3 scripts/test_classify_ci_changes.py
  python3 scripts/validate_agent_policies.py
  python3 scripts/validate_agent_assets.py
  actionlint
  ruby -rpsych -e 'Dir[".github/workflows/*.{yml,yaml}"].sort.each { |p| Psych.parse_stream(File.binread(p), filename: p) }'
  git diff --check
  ```

  Expected: every command exits `0`; the roadmap suite has no stale activation failures; `rg -n 'uses:\s+[^ #]+@v[0-9]' .github/workflows` returns no matches.

- [ ] **Step 11: Commit the atomic active policy state**

  Stage only the files listed in Task 2, inspect the complete staged diff, and commit:

  ```bash
  git add .github/workflows scripts/check_ci_permissions.rb \
    scripts/test_check_ci_permissions.rb scripts/check_roadmap_evidence.rb \
    scripts/test_check_roadmap_evidence.rb \
    tests/fixtures/policy-successor-manifest.json
  git diff --cached --check
  git commit -m "ci: activate nonblocking change-aware policy"
  ```

---

### Task 3: Record D-172 and align repository governance documentation

**Files:**
- Create: `docs/decisions/D-172-nonblocking-property-based-ci-policy-audit.md`
- Modify: `docs/decisions/D-103-keep-search-policy-successors-base-owned-through.md` only to set frontmatter status `superseded` and its prose status to `superseded by D-172`, preserving the decision body as historical evidence
- Modify: `docs/decisions/README.md` through the generator
- Modify: `AGENTS.md`
- Modify: `docs/REPOSITORY_GOVERNANCE.md`
- Modify: `docs/TESTING.md`
- Modify: `docs/ROADMAP.md`
- Verify: `docs/superpowers/specs/2026-08-17-nonblocking-ci-policy-audit-design.md`

**Interfaces:**
- Produces: accepted D-172, narrowly superseding D-103's forced exact-byte general CI transition and recording the one-use owner-authorized D-024 recovery.
- Preserves: D-103 as historical evidence and D-125 unchanged for external-state-only failures.

- [ ] **Step 1: Write the accepted successor decision**

  Use this decision shape:

  ```markdown
  ---
  id: D-172
  title: "Use a base-owned property audit without forced CI successor activation"
  status: accepted
  ---

  ## D-172: Use a base-owned property audit without forced CI successor activation

  - Status: accepted
  - Context: PR #562 left `main` in a D-103 contradiction: unchanged CI failed the exact successor transition, while exact D-171 activation ran five stale self-test assertions. Two merges did not prevent the same maintainer from staging and later activating weak policy, but did make unrelated PRs unmergeable.
  - Decision: keep required `audit` base-owned under `pull_request_target`, download candidate workflows only as data, and validate permissions, Action pins, checkout credentials, trusted-event guards, D-171 routing, Tier-1 coverage, and `ci-gate` truth-table properties. General CI/checker files no longer require predecessor-staged whole-file bytes. D-125 remains external-state-only. The owner authorized one D-024 relaxation of `audit` for the #558 recovery PR; `ci-gate` remains required and protection is restored immediately.
  - Consequences: ordinary CI changes use one PR; historical successor fixtures remain evidence but cannot force activation. Trust-anchor workflow changes remain separately protected. The recovery records its protection snapshot, one-merge/ten-minute window, exact merge SHA, restore readback, and independent post-restore verification.
  ```

- [ ] **Step 2: Align active-state documentation**

  Update the existing Task 7 documentation drafts so they no longer claim that live CI is protected by or must equal SHA `785e6415...`. State instead:

  - D-171 routing is active and checked by named properties;
  - compiler changes run coverage plus the complete Tier-1/macOS Intel topology;
  - docs/agent/Pages classifications skip only unrelated heavy work;
  - `audit` remains base-owned and required;
  - the successor manifest is historical input inventory, not merge authorization;
  - D-125 is unchanged, and the #558 D-024 authority is one-use only;
  - zero GitHub approving reviews remain correct for the solo-maintainer repository, while local pinned review and resolved conversations remain required.

- [ ] **Step 3: Regenerate and verify the decision index**

  ```bash
  python3 scripts/generate_decisions_index.py docs/decisions docs/decisions/README.md
  python3 scripts/generate_decisions_index.py --check docs/decisions docs/decisions/README.md
  python3 scripts/test_generate_decisions_index.py
  git diff --check
  ```

- [ ] **Step 4: Run policy/documentation checks**

  ```bash
  ruby scripts/test_check_ci_permissions.rb
  ruby scripts/check_ci_permissions.rb
  ruby scripts/test_check_roadmap_evidence.rb
  ruby scripts/check_roadmap_evidence.rb
  python3 -m unittest discover -s scripts -p 'test_*.py'
  ```

  Expected: all commands exit `0`; decision index is fresh; no document asserts the retired exact live-CI digest authorization.

- [ ] **Step 5: Commit the governance record**

  ```bash
  git add AGENTS.md docs/REPOSITORY_GOVERNANCE.md docs/ROADMAP.md \
    docs/TESTING.md docs/decisions/D-103-keep-search-policy-successors-base-owned-through.md \
    docs/decisions/D-172-nonblocking-property-based-ci-policy-audit.md \
    docs/decisions/README.md
  git diff --cached --check
  git commit -m "docs: accept nonblocking CI policy audit"
  ```

---

### Task 4: Verify the complete recovery candidate and obtain independent review

**Files:**
- Review: complete range from `origin/main` through `HEAD`
- Write ignored evidence only: `.superpowers/sdd/2026-08-15-ci-feedback-routing/task-7-recovery-report.md`

**Interfaces:**
- Produces: local verification evidence, current-base failure classification, and a reviewed immutable PR head.

- [ ] **Step 1: Refresh state and prove the diff scope**

  ```bash
  git fetch --no-tags origin main
  git status --short
  git diff --check origin/main...HEAD
  git diff --stat origin/main...HEAD
  gh pr list --repo rotnov/pycc --state open --limit 50 \
    --json number,title,headRefOid,mergeStateStatus,files
  ```

  Expected: no unstaged implementation files remain; #560 is still recorded as an overlapping, separately owned PR; unrelated #518 paths remain untouched.

- [ ] **Step 2: Run all locally available repository gates**

  ```bash
  cargo doc --workspace --no-deps
  cargo build --target x86_64-apple-darwin -p pycc_rt
  cargo build --workspace
  cargo build --release -p pycc_rt
  cargo test --workspace -- --skip init_reports_an_unavailable_cwd_without_panicking
  cargo clippy --workspace --all-targets -- -D warnings
  ruby scripts/test_check_ci_permissions.rb
  ruby scripts/check_ci_permissions.rb
  ruby scripts/test_check_roadmap_evidence.rb
  ruby scripts/check_roadmap_evidence.rb
  ruby scripts/test_check_readme_coverage_badge.rb
  ruby scripts/check_readme_coverage_badge.rb
  python3 -m unittest discover -s scripts -p 'test_*.py'
  python3 scripts/test_classify_ci_changes.py
  python3 scripts/validate_agent_policies.py
  python3 scripts/validate_agent_assets.py
  bash scripts/check-codex-marketplace.sh
  bash scripts/check-claude-marketplace.sh
  python3 scripts/run_alpha_skill_evals.py --client codex --pycc-bin target/debug/pycc
  python3 scripts/run_alpha_skill_evals.py --client claude --pycc-bin target/debug/pycc
  ruby scripts/check_frontend_throughput.rb target/debug/pycc tests/fixtures/pr6_1000_loc_bench.py 75
  actionlint
  ruby -rpsych -e 'Dir[".github/workflows/*.{yml,yaml}"].sort.each { |p| Psych.parse_stream(File.binread(p), filename: p) }'
  ```

  Record the known macOS deleted-CWD test hang and the installed Python 3.14.6 versus required 3.14.7 limitation exactly; do not claim those CI-only gates passed locally. Do not weaken or delete them.

- [ ] **Step 3: Simulate the current base-owned audit**

  Create a detached temporary worktree and run the base checker against candidate data:

  ```bash
  recovery_base=$(mktemp -d)
  git worktree add --detach "$recovery_base" origin/main
  ruby "$recovery_base/scripts/check_ci_permissions.rb" .github/workflows
  GITHUB_EVENT_NAME=pull_request_target \
    POLICY_CANDIDATE_ROOT="$PWD" \
    ruby "$recovery_base/scripts/check_ci_permissions.rb" .github/workflows
  ruby "$recovery_base/scripts/check_roadmap_evidence.rb" "$PWD"
  git worktree remove "$recovery_base"
  ```

  Expected: the first structural permission run exits `0`; the latter two base-policy runs fail only on the retired exact successor/full-workflow authorization. Any other diagnostic is a blocker and must be fixed before D-024.

- [ ] **Step 4: Run the repository-required review loop**

  ```bash
  python3 scripts/check_claude_reviewer_binding.py
  ```

  Review the complete committed range from `git merge-base HEAD origin/main` through `HEAD` with the eligible read-only deep reviewer. Address every P0/P1 and every actionable correctness/contract finding, rerun affected tests, and make focused fix commits. If the reviewer gives no progress for three consecutive bounded waits, interrupt it, record reviewer unavailability, and complete the repository's inline 11-point review without blocking indefinitely.

- [ ] **Step 5: Freeze the reviewed head**

  ```bash
  git status --short
  git rev-parse HEAD
  git diff --check origin/main...HEAD
  ```

  Record the exact reviewed head SHA and do not amend it after publication without repeating Tasks 4.1-4.5.

---

### Task 5: Publish the PR and perform the one-use D-024 recovery

**Files:**
- GitHub: update issue #558, create one recovery PR, create one public bypass incident, merge, restore protection, and attach evidence
- Do not modify: PR #560

**Interfaces:**
- Produces: merged recovery commit on `main`, exact branch-protection restoration evidence, and a post-merge CI measurement checkpoint.

- [ ] **Step 1: Reconcile immediately before publication**

  Fetch `origin/main`, verify the branch is based on its current tip, re-list open PRs, and compare overlapping paths/head SHAs. If `main` advanced, rebase non-interactively, resolve only this branch, and rerun Task 4. If #560 changed, refresh the overlap record; never copy its unrelated compiler changes.

- [ ] **Step 2: Push and open the recovery PR**

  Push `codex/activate-change-aware-ci`, open a PR whose title begins `P1:`, links but does not prematurely close #558, explains the exact-byte deadlock, lists the property invariants, records macOS Intel retention, and states that `audit` is expected to fail only on the superseded base rule. Comment the PR URL and reviewed head SHA on #558.

- [ ] **Step 3: Wait for every non-`audit` gate**

  Require `ci-gate` and every visible matrix/gate job to succeed, no unresolved review threads, no merge conflicts, current strict base, and no actionable review findings. Reproduce the `audit` failure from its logs and confirm it matches only the two expected retired exact-authorization paths. Do not begin D-024 while any candidate-caused structural or functional failure remains.

- [ ] **Step 4: Open the public D-024 incident before protection changes**

  Create an issue titled `[ci-bypass] D-103 exact-byte recovery for #558`. Its body must include:

  - owner authorization date `2026-08-17` and exact scope (`audit` only, one PR, one merge, ten minutes);
  - recovery PR number and immutable reviewed head SHA;
  - current `main` SHA;
  - complete required-check/protection snapshot with credentials and response URLs omitted;
  - exact `audit` failure evidence and successful `ci-gate` URL;
  - expiry timestamp;
  - exact restore payload and command;
  - statement that no other PR may merge during the window.

- [ ] **Step 5: Relax only `audit`, merge once, and restore in all exit paths**

  Use the scoped required-status-checks PATCH endpoint, preserving `strict` and the app binding for `ci-gate`. Do not use the whole-protection PUT endpoint. Re-read protection immediately and assert every field except the absent `audit` check equals the pre-relax snapshot. Merge only the reviewed PR head, verify the resulting `main` merge SHA, then immediately PATCH the exact original `required_status_checks` payload back. If merge fails, restore before diagnosing.

  Execute the mutation from one bounded shell using values read directly from
  GitHub; do not interpolate issue text or any other untrusted free text:

  ```bash
  recovery_state=$(mktemp -d)
  recovery_pr=$(gh pr view --repo rotnov/pycc --json number --jq .number)
  recovery_head=$(gh pr view "$recovery_pr" --repo rotnov/pycc --json headRefOid --jq .headRefOid)
  protection_endpoint=repos/rotnov/pycc/branches/main/protection
  checks_endpoint=repos/rotnov/pycc/branches/main/protection/required_status_checks

  gh api "$protection_endpoint" > "$recovery_state/protection.before.json"
  jq '.required_status_checks | {strict, checks}' \
    "$recovery_state/protection.before.json" > "$recovery_state/checks.restore.json"
  jq '.required_status_checks | {strict, checks: [.checks[] | select(.context != "audit")]}' \
    "$recovery_state/protection.before.json" > "$recovery_state/checks.relaxed.json"

  restore_checks() {
    gh api --method PATCH "$checks_endpoint" \
      --input "$recovery_state/checks.restore.json" >/dev/null
  }
  trap restore_checks EXIT INT TERM HUP

  gh api --method PATCH "$checks_endpoint" \
    --input "$recovery_state/checks.relaxed.json" >/dev/null
  gh api "$protection_endpoint" > "$recovery_state/protection.relaxed.json"
  gh pr merge "$recovery_pr" --repo rotnov/pycc --squash --admin \
    --match-head-commit "$recovery_head"
  restore_checks
  trap - EXIT INT TERM HUP
  gh api "$protection_endpoint" > "$recovery_state/protection.restored.json"
  ```

  Before `gh pr merge`, compare the relaxed readback with the saved snapshot
  using a normalization that removes response-only `url` fields and permits
  only the absent `audit` check. After restoration, the same normalization must
  compare equal byte-for-byte. If either comparison fails, invoke
  `restore_checks` and stop before merging or closing the incident.

  ```bash
  jq 'walk(if type == "object" then del(.url) else . end) |
      .required_status_checks.checks |= map(select(.context != "audit"))' \
    "$recovery_state/protection.before.json" > "$recovery_state/protection.relaxed.expected.json"
  jq 'walk(if type == "object" then del(.url) else . end)' \
    "$recovery_state/protection.relaxed.json" > "$recovery_state/protection.relaxed.actual.json"
  cmp "$recovery_state/protection.relaxed.expected.json" \
      "$recovery_state/protection.relaxed.actual.json"

  jq 'walk(if type == "object" then del(.url) else . end)' \
    "$recovery_state/protection.before.json" > "$recovery_state/protection.before.normalized.json"
  jq 'walk(if type == "object" then del(.url) else . end)' \
    "$recovery_state/protection.restored.json" > "$recovery_state/protection.restored.normalized.json"
  cmp "$recovery_state/protection.before.normalized.json" \
      "$recovery_state/protection.restored.normalized.json"
  ```

  The recovery window ends on the first of: successful merge, any command failure, or ten minutes. No second merge is authorized.

- [ ] **Step 6: Perform post-restore Gate 2**

  Re-read the full branch protection and byte-compare its normalized effective fields with the pre-relax snapshot. Run:

  ```bash
  python3 scripts/manage_ci_bypass.py --repo rotnov/pycc status
  gh api repos/rotnov/pycc/branches/main/protection
  ```

  Verify strict required checks are exactly app-bound `audit` plus `ci-gate`, administrators are enforced, conversations are required, approving reviews remain zero, and force pushes/deletion remain disabled. Run a fresh independent adversarial review of the incident, merge SHA, and before/after settings. Attach the evidence to the incident and close it only after the post-merge `audit`, `ci-gate`, and main-history audit succeed.

- [ ] **Step 7: Record the post-merge checkpoint on #558**

  Comment the exact recovery merge SHA, workflow run URLs, protection-restore evidence, and measurement-window start. Keep #558 labeled `in progress`; do not open another implementation issue and do not claim savings yet. Notify PR #560 that it overlaps the newly merged CI/roadmap policy and must rebase and renumber its colliding D-171/D-172 decisions.

---

### Task 6: Measure after 20 subsequent merged pull requests

**Files:**
- GitHub issue #558: append the fixed-window comparison

**Interfaces:**
- Consumes: recovery merge SHA and fixed baseline comment `https://github.com/rotnov/pycc/issues/558#issuecomment-5302145633`.
- Produces: comparable 20-PR post-activation measurement and final #558 disposition.

- [ ] **Step 1: Count only subsequent merged PRs**

  Starting strictly after the recovery merge SHA, count merged PRs targeting `main`. Exclude closed-unmerged PRs from the merged sample but report them separately exactly as in the baseline.

- [ ] **Step 2: Recompute the same metrics at 20 merges**

  Report median PR lifetime, median final CI duration, total runner-minutes, unsuccessful runner-minutes, compiler versus non-compiler classification, and macOS Intel native/cross minutes using the same definitions as the fixed #519-#557 baseline.

- [ ] **Step 3: Publish the comparison and close #558 only on evidence**

  Compare 20 post-activation merges against: 18 merged/2 closed-unmerged, 18-minute median lifetime, 7-minute median final CI, 1,064 total runner-minutes, and 518 unsuccessful runner-minutes. State regressions as plainly as improvements. Close #558 and remove `in progress` only after this comparison is public and any release-blocking regression has a separately prioritized issue.
