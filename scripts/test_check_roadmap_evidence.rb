#!/usr/bin/env ruby
# frozen_string_literal: true

require "fileutils"
require "minitest/autorun"
require "open3"
require "pathname"
require "psych"
require "rbconfig"
require "tmpdir"

require_relative "check_roadmap_evidence"

class RoadmapEvidenceCliTest < Minitest::Test
  CHECKER = Pathname(__dir__) / "check_roadmap_evidence.rb"
  COVERAGE_STEP_HEADER =
    "      - name: Hard coverage gate — 100% lines + regions (D-014)"
  COVERAGE_COMMAND =
    "run_isolated \"$TRUSTED_COV\" llvm-cov --workspace " \
    "--fail-under-lines 100 --fail-under-regions 100"

  def coverage_workflow(command = COVERAGE_COMMAND)
    <<~YAML
      on:
        pull_request:
      env:
        CARGO_LLVM_COV_VERSION: "0.8.7"
        LLVM_VERSION: "22.1.1"
      jobs:
        build-test-coverage:
          runs-on: macos-14
          steps:
            - uses: actions/checkout@d23441a48e516b6c34aea4fa41551a30e30af803
              with:
                persist-credentials: false
            - name: Show pinned toolchain
              run: rustup show
            - name: Install LLVM 22 (D-015)
              run: brew install llvm@22
            - name: Install llvm-tools-preview
              run: rustup component add llvm-tools-preview
            - name: Add x86_64-apple-darwin Rust target
              run: rustup target add x86_64-apple-darwin
            - name: Hard coverage gate — 100% lines + regions (D-014)
              run: |
                set -euo pipefail
                LLVM_SYS_221_PREFIX_VALUE="$(brew --prefix llvm@22)"
                TRUSTED_CARGO="$(rustup which cargo)"
                TRUSTED_RUSTC="$(rustup which rustc)"
                TRUSTED_RUSTDOC="$(rustup which rustdoc)"
                TRUSTED_COV="/Users/runner/.cargo/bin/cargo-llvm-cov"
                TRUSTED_TOOLCHAIN="$(dirname "$(dirname "$TRUSTED_CARGO")")"
                cd "$RUNNER_TEMP"
                RUSTC="$TRUSTED_RUSTC" RUSTDOC="$TRUSTED_RUSTDOC" "$TRUSTED_CARGO" install cargo-llvm-cov --locked --version "${CARGO_LLVM_COV_VERSION}"
                "$TRUSTED_COV" llvm-cov --version
                sudo chmod o+x /Users/runner /Users/runner/.cargo /Users/runner/.cargo/bin /Users/runner/.rustup /Users/runner/.rustup/toolchains
                sudo chmod -R o+rX "$TRUSTED_TOOLCHAIN"
                sudo chmod o+rx "$TRUSTED_COV"
                ISOLATED_ROOT="$RUNNER_TEMP/pycc-coverage"
                mkdir -p "$ISOLATED_ROOT/home" "$ISOLATED_ROOT/tmp" "$ISOLATED_ROOT/cargo-home" "$ISOLATED_ROOT/target"
                sudo chown -R nobody:nobody "$ISOLATED_ROOT"
                ISOLATED_ENV=(
                  "HOME=$ISOLATED_ROOT/home"
                  "TMPDIR=$ISOLATED_ROOT/tmp/"
                  "CARGO_HOME=$ISOLATED_ROOT/cargo-home"
                  "CARGO_TARGET_DIR=$ISOLATED_ROOT/target"
                  "CARGO=$TRUSTED_CARGO"
                  "RUSTC=$TRUSTED_RUSTC"
                  "RUSTDOC=$TRUSTED_RUSTDOC"
                  "LLVM_SYS_221_PREFIX=$LLVM_SYS_221_PREFIX_VALUE"
                  "PATH=$(dirname "$TRUSTED_CARGO"):/usr/bin:/bin:/usr/sbin:/sbin:/opt/homebrew/bin"
                )
                run_isolated() {
                  sudo -u nobody env -i "${ISOLATED_ENV[@]}" "$@"
                }
                ln -s "$ISOLATED_ROOT/target" "$GITHUB_WORKSPACE/target"
                cd "$GITHUB_WORKSPACE"
                run_isolated "$TRUSTED_CARGO" build --target x86_64-apple-darwin -p pycc_rt
                run_isolated "$TRUSTED_CARGO" build --workspace
                #{command}
                rm "$GITHUB_WORKSPACE/target"
                printf 'LLVM_SYS_221_PREFIX=%s\\n' "$LLVM_SYS_221_PREFIX_VALUE" >> "$GITHUB_ENV"
    YAML
  end

  def run_checker(roadmap:, workflow:)
    Dir.mktmpdir do |directory|
      root = Pathname(directory)
      FileUtils.mkdir_p(root / "docs")
      FileUtils.mkdir_p(root / ".github/workflows")
      (root / "docs/ROADMAP.md").write(roadmap)
      (root / ".github/workflows/ci.yml").write(workflow)
      return Open3.capture3(RbConfig.ruby, CHECKER.to_s, root.to_s)
    end
  end

  def perf_gate_workflow(
    promote_if: nil,
    save_if: nil,
    restore_action: PINNED_CACHE_RESTORE_ACTION,
    missing_step: nil,
    swap_comparison_and_promotion: false,
    comparison_continue_on_error: nil,
    job_continue_on_error: nil
  )
    steps = [
      {
        "uses" => PINNED_CHECKOUT_ACTION,
        "with" => { "persist-credentials" => false }
      },
      *TRUSTED_PERF_LIFECYCLE_STEPS.map(&:dup)
    ]
    restore = steps.find do |step|
      step["name"] == "Restore previous frontend-perf baseline"
    end
    restore["uses"] = restore_action
    comparison = steps.find do |step|
      step["name"] == "Compare against previous baseline (if one was restored)"
    end
    comparison["continue-on-error"] = comparison_continue_on_error unless comparison_continue_on_error.nil?
    promote = steps.find do |step|
      step["name"] == "Save this run's timing as the next run's baseline"
    end
    promote["if"] = promote_if if promote_if
    save = steps.find { |step| step["name"] == "Cache this run's baseline" }
    save["if"] = save_if if save_if
    steps.reject! { |step| step["name"] == missing_step } if missing_step
    if swap_comparison_and_promotion
      comparison_index = steps.index(comparison)
      promote_index = steps.index(promote)
      steps[comparison_index], steps[promote_index] = steps[promote_index], steps[comparison_index]
    end

    perf_job = { "steps" => steps }
    perf_job["continue-on-error"] = job_continue_on_error unless job_continue_on_error.nil?
    { "jobs" => { "frontend-perf-gate" => perf_job } }.to_yaml
  end

  def test_rejects_false_completed_item_without_evidence
    roadmap = <<~MARKDOWN
      # pycc Roadmap

      ## Current delivery status

      ### v0.1 acceptance checklist

      - [x] `pycc check` processes 1k LOC in under 50 ms.
    MARKDOWN

    _stdout, stderr, status = run_checker(roadmap: roadmap, workflow: "jobs: {}\n")

    refute status.success?
    assert_includes stderr, "checked roadmap item is missing an evidence marker"
  end

  def test_rejects_all_checked_markdown_list_bullets_without_evidence
    ["*", "+", "1."].each do |bullet|
      roadmap = <<~MARKDOWN
        # pycc Roadmap

        ## Current delivery status

        ### v0.1 acceptance checklist

        #{bullet} [x] `pycc check` processes 1k LOC in under 50 ms.
      MARKDOWN

      _stdout, stderr, status = run_checker(roadmap: roadmap, workflow: "jobs: {}\n")

      refute status.success?, "expected #{bullet.inspect} task item to be checked"
      assert_includes stderr, "checked roadmap item is missing an evidence marker"
    end
  end

  def test_rejects_checked_items_nested_in_markdown_blockquotes_without_evidence
    ["> -", "> > 1)"].each do |prefix|
      roadmap = <<~MARKDOWN
        # pycc Roadmap

        ## Current delivery status

        ### v0.1 acceptance checklist

        #{prefix} [x] `pycc check` processes 1k LOC in under 50 ms.
      MARKDOWN

      _stdout, stderr, status = run_checker(roadmap: roadmap, workflow: "jobs: {}\n")

      refute status.success?, "expected #{prefix.inspect} task item to be checked"
      assert_includes stderr, "checked roadmap item is missing an evidence marker"
    end
  end

  def test_rejects_checked_items_nested_under_list_containers_without_evidence
    roadmap = <<~MARKDOWN
      # pycc Roadmap

      ## Current delivery status

      ### v0.1 acceptance checklist

      - Grouped acceptance claims:

          - [x] `pycc check` processes 1k LOC in under 50 ms.
    MARKDOWN

    _stdout, stderr, status = run_checker(roadmap: roadmap, workflow: "jobs: {}\n")

    refute status.success?
    assert_includes stderr, "checked roadmap item is missing an evidence marker"
  end

  def test_rejects_evidence_marker_attached_to_the_wrong_claim
    roadmap = <<~MARKDOWN
      # pycc Roadmap

      ## Current delivery status

      ### v0.1 acceptance checklist

      - [x] `pycc check` processes 1k LOC in under 50 ms. <!-- roadmap-evidence: ci-build-test-coverage-100 -->
    MARKDOWN

    _stdout, stderr, status = run_checker(
      roadmap: roadmap,
      workflow: coverage_workflow
    )

    refute status.success?
    assert_includes stderr, "does not prove this roadmap claim"
  end

  def test_rejects_coverage_evidence_moved_outside_the_v0_1_checklist
    roadmap = <<~MARKDOWN
      # pycc Roadmap

      ## v1.0 — spec freeze

      ### v0.1 acceptance checklist

      - [x] The 100% line and region coverage gate is required and green for the current slice. <!-- roadmap-evidence: ci-build-test-coverage-100 -->
    MARKDOWN

    _stdout, stderr, status = run_checker(
      roadmap: roadmap,
      workflow: coverage_workflow
    )

    refute status.success?
    assert_includes stderr, "must appear under the expected roadmap section"
  end

  def test_rejects_coverage_evidence_after_a_blockquoted_heading_changes_the_section
    roadmap = <<~MARKDOWN
      # pycc Roadmap

      ## Current delivery status

      ### v0.1 acceptance checklist

      > #### Other milestone
      >
      > - [x] The 100% line and region coverage gate is required and green for the current slice. <!-- roadmap-evidence: ci-build-test-coverage-100 -->
    MARKDOWN

    _stdout, stderr, status = run_checker(
      roadmap: roadmap,
      workflow: coverage_workflow
    )

    refute status.success?
    assert_includes stderr, "must appear under the expected roadmap section"
  end

  def test_rejects_coverage_evidence_after_a_list_item_heading_changes_the_section
    roadmap = <<~MARKDOWN
      # pycc Roadmap

      ## Current delivery status

      ### v0.1 acceptance checklist

      - #### Other milestone
        - [x] The 100% line and region coverage gate is required and green for the current slice. <!-- roadmap-evidence: ci-build-test-coverage-100 -->
    MARKDOWN

    _stdout, stderr, status = run_checker(
      roadmap: roadmap,
      workflow: coverage_workflow
    )

    refute status.success?
    assert_includes stderr, "must appear under the expected roadmap section"
  end

  def test_rejects_checked_item_continuing_an_empty_list_marker_without_evidence
    roadmap = <<~MARKDOWN
      # pycc Roadmap

      ## Current delivery status

      ### v0.1 acceptance checklist

      -
        [x] `pycc check` processes 1k LOC in under 50 ms.
    MARKDOWN

    _stdout, stderr, status = run_checker(roadmap: roadmap, workflow: "jobs: {}\n")

    refute status.success?
    assert_includes stderr, "checked roadmap item is missing an evidence marker"
  end

  def test_ignores_headings_hidden_in_fences_and_html_comments
    hidden_sections = [
      <<~MARKDOWN,
        ```markdown
        # pycc Roadmap

        ## Current delivery status

        ### v0.1 acceptance checklist
        ```
      MARKDOWN
      <<~MARKDOWN
        <!--
        # pycc Roadmap

        ## Current delivery status

        ### v0.1 acceptance checklist
        -->
      MARKDOWN
    ]

    hidden_sections.each do |hidden_section|
      roadmap = <<~MARKDOWN
        # pycc Roadmap

        ## v1.0 — spec freeze

        #{hidden_section}
        - [x] The 100% line and region coverage gate is required and green for the current slice. <!-- roadmap-evidence: ci-build-test-coverage-100 -->
      MARKDOWN

      _stdout, stderr, status = run_checker(
        roadmap: roadmap,
        workflow: coverage_workflow
      )

      refute status.success?
      assert_includes stderr, "must appear under the expected roadmap section"
    end
  end

  def test_rejects_headings_hidden_inside_raw_html_blocks
    roadmap = <<~MARKDOWN
      # Wrong Roadmap

      <script>
      # pycc Roadmap

      ## Current delivery status

      ### v0.1 acceptance checklist
      </script>

      - [x] The 100% line and region coverage gate is required and green for the current slice. <!-- roadmap-evidence: ci-build-test-coverage-100 -->
    MARKDOWN

    _stdout, stderr, status = run_checker(
      roadmap: roadmap,
      workflow: coverage_workflow
    )

    refute status.success?
    assert_includes stderr, "raw HTML blocks are not supported"
  end

  def test_rejects_tab_indented_pseudo_headings
    roadmap = <<~MARKDOWN
      \t# pycc Roadmap

      \t## Current delivery status

      \t### v0.1 acceptance checklist

      - [x] The 100% line and region coverage gate is required and green for the current slice. <!-- roadmap-evidence: ci-build-test-coverage-100 -->
    MARKDOWN

    _stdout, stderr, status = run_checker(
      roadmap: roadmap,
      workflow: coverage_workflow
    )

    refute status.success?
    assert_includes stderr, "must appear under the expected roadmap section"
  end

  def test_ignores_checked_items_hidden_in_fences_and_html_comments
    hidden_items = [
      "```\n- [x] Hidden example without evidence.\n```\n",
      "<!--\n- [x] Hidden note without evidence.\n-->\n"
    ]

    hidden_items.each do |hidden_item|
      stdout, stderr, status = run_checker(
        roadmap: "# pycc Roadmap\n\n#{hidden_item}",
        workflow: coverage_workflow
      )

      assert status.success?, stderr
      assert_includes stdout, "Roadmap evidence policy passed."
    end
  end

  def test_ignores_checked_items_inside_blockquoted_fences
    roadmap = <<~MARKDOWN
      # pycc Roadmap

      > ```markdown
      > - [x] Quoted code example without evidence.
      > ```
    MARKDOWN

    stdout, stderr, status = run_checker(
      roadmap: roadmap,
      workflow: coverage_workflow
    )

    assert status.success?, stderr
    assert_includes stdout, "Roadmap evidence policy passed."
  end

  def test_ignores_checked_items_inside_fences_nested_under_list_items
    roadmap = <<~MARKDOWN
      # pycc Roadmap

      - Example:

          ```markdown
          - [x] Nested code example without evidence.
          ```
    MARKDOWN

    stdout, stderr, status = run_checker(
      roadmap: roadmap,
      workflow: coverage_workflow
    )

    assert status.success?, stderr
    assert_includes stdout, "Roadmap evidence policy passed."
  end

  def test_ignores_checked_items_rendered_as_indented_code
    ["    - [x] Root code example.\n", ">     - [x] Quoted code example.\n"].each do |example|
      stdout, stderr, status = run_checker(
        roadmap: "# pycc Roadmap\n\n#{example}",
        workflow: coverage_workflow
      )

      assert status.success?, stderr
      assert_includes stdout, "Roadmap evidence policy passed."
    end
  end

  def test_rejects_setext_headings_that_change_the_rendered_section
    roadmap = <<~MARKDOWN
      # pycc Roadmap

      ## Current delivery status

      ### v0.1 acceptance checklist

      Wrong rendered root
      ===================

      - [x] The 100% line and region coverage gate is required and green for the current slice. <!-- roadmap-evidence: ci-build-test-coverage-100 -->
    MARKDOWN

    _stdout, stderr, status = run_checker(
      roadmap: roadmap,
      workflow: coverage_workflow
    )

    refute status.success?
    assert_includes stderr, "Setext headings are not supported"
  end

  def test_rejects_an_unknown_evidence_marker
    roadmap = <<~MARKDOWN
      # pycc Roadmap

      ## Current delivery status

      ### v0.1 acceptance checklist

      - [x] The 100% line and region coverage gate is required and green for the current slice. <!-- roadmap-evidence: invented-proof -->
    MARKDOWN

    _stdout, stderr, status = run_checker(
      roadmap: roadmap,
      workflow: coverage_workflow
    )

    refute status.success?
    assert_includes stderr, 'unknown roadmap evidence "invented-proof"'
  end

  def test_rejects_multiple_evidence_markers_on_one_checked_item
    repository_root = Pathname(__dir__).parent
    workflow = (repository_root / ".github/workflows/ci.yml").read
    roadmap = <<~MARKDOWN
      # pycc Roadmap

      ## Current delivery status

      ### v0.1 acceptance checklist

      - [x] The five-target native CI matrix and one cross-host compilation path are live on `main`. <!-- roadmap-evidence: ci-tier1-cross-compile --> <!-- roadmap-evidence: invented-proof -->
    MARKDOWN

    _stdout, stderr, status = run_checker(roadmap: roadmap, workflow: workflow)

    refute status.success?
    assert_includes stderr, "must contain exactly one evidence marker"
  end

  def test_accepts_reviewed_tier1_matrix_evidence
    repository_root = Pathname(__dir__).parent
    workflow = (repository_root / ".github/workflows/ci.yml").read
    roadmap = <<~MARKDOWN
      # pycc Roadmap

      ## Current delivery status

      ### v0.1 acceptance checklist

      - [x] The five-target native CI matrix and one cross-host compilation path are live on `main`. <!-- roadmap-evidence: ci-tier1-cross-compile -->
    MARKDOWN

    stdout, stderr, status = run_checker(roadmap: roadmap, workflow: workflow)

    assert status.success?, stderr
    assert_includes stdout, "Roadmap evidence policy passed."
  end

  def test_tier1_workflow_authorization_is_an_allowlist
    assert_kind_of Array, TIER1_CI_WORKFLOW_SHA256S
    assert_includes(
      TIER1_CI_WORKFLOW_SHA256S,
      Digest::SHA256.hexdigest(
        (Pathname(__dir__).parent / ".github/workflows/ci.yml").read
      )
    )
  end

  def test_tier1_workflow_allowlist_retires_the_pre_alpha_eval_digest
    refute_includes(
      TIER1_CI_WORKFLOW_SHA256S,
      "58e2d5026b59e7c921b57c882d24b6507c95dd8f99e390c0a68af217e5e038c8"
    )
  end

  def test_tier1_workflow_allowlist_stages_the_frontend_perf_gate_digest
    # Per docs/TESTING.md's staged-update procedure: the reviewed prospective
    # digest for the pending frontend-perf-gate ci.yml revision (adding the
    # frontend-perf-gate job and requiring it in ci-gate) is appended here
    # while the current digest remains active, ahead of the pull request that
    # actually activates that workflow and retires this repository's current
    # digest.
    assert_includes(
      TIER1_CI_WORKFLOW_SHA256S,
      PR4_REQUIRED_PERF_CI_WORKFLOW_SHA256
    )
  end

  def test_accepts_a_perf_baseline_published_only_after_success
    assert validate_perf_gate_baseline_lifecycle(
      perf_gate_workflow,
      "ci.yml"
    )
  end

  def test_rejects_promoting_a_perf_baseline_after_a_failed_comparison
    error = assert_raises(RoadmapEvidenceError) do
      validate_perf_gate_baseline_lifecycle(
        perf_gate_workflow(promote_if: "always()"),
        "ci.yml"
      )
    end
    assert_includes error.message, "fail-closed sequence"
  end

  def test_rejects_caching_a_perf_baseline_after_a_failed_comparison
    error = assert_raises(RoadmapEvidenceError) do
      validate_perf_gate_baseline_lifecycle(
        perf_gate_workflow(save_if: "always()"),
        "ci.yml"
      )
    end
    assert_includes error.message, "fail-closed sequence"
  end

  def test_rejects_a_mutable_perf_cache_action
    error = assert_raises(RoadmapEvidenceError) do
      validate_perf_gate_baseline_lifecycle(
        perf_gate_workflow(restore_action: "actions/cache/restore@v4"),
        "ci.yml"
      )
    end
    assert_includes error.message, "reviewed immutable pins"
  end

  def test_rejects_a_missing_perf_comparison
    error = assert_raises(RoadmapEvidenceError) do
      validate_perf_gate_baseline_lifecycle(
        perf_gate_workflow(
          missing_step: "Compare against previous baseline (if one was restored)"
        ),
        "ci.yml"
      )
    end
    assert_includes error.message, "ordered baseline lifecycle"
  end

  def test_rejects_a_reordered_perf_comparison
    error = assert_raises(RoadmapEvidenceError) do
      validate_perf_gate_baseline_lifecycle(
        perf_gate_workflow(swap_comparison_and_promotion: true),
        "ci.yml"
      )
    end
    assert_includes error.message, "ordered baseline lifecycle"
  end

  def test_rejects_a_perf_comparison_that_can_hide_failure
    error = assert_raises(RoadmapEvidenceError) do
      validate_perf_gate_baseline_lifecycle(
        perf_gate_workflow(comparison_continue_on_error: true),
        "ci.yml"
      )
    end
    assert_includes error.message, "fail-closed sequence"
  end

  def test_rejects_a_perf_job_that_can_hide_failure
    error = assert_raises(RoadmapEvidenceError) do
      validate_perf_gate_baseline_lifecycle(
        perf_gate_workflow(job_continue_on_error: true),
        "ci.yml"
      )
    end
    assert_includes error.message, "must propagate failures"
  end

  def test_tier1_workflow_allowlist_stages_language_track_guard_digest
    assert_equal(
      "b77ab0c1c3bcc69e69d3cb8f08e081f6eae246e7d5d19c9356455db1ff4291d2",
      ACTIVE_TIER1_CI_WORKFLOW_SHA256
    )
    assert_equal(
      "05ea9d7882ea817a764afae7e0fe850fbb76c73780c93ae3d922f7fbde9290e0",
      STAGED_TIER1_CI_WORKFLOW_SHA256
    )
    assert_equal(
      [
        ACTIVE_TIER1_CI_WORKFLOW_SHA256,
        PR4_REQUIRED_PERF_CI_WORKFLOW_SHA256,
        STAGED_TIER1_CI_WORKFLOW_SHA256
      ],
      TIER1_CI_WORKFLOW_SHA256S
    )
  end

  def test_staged_language_track_workflow_fixture_is_not_the_active_workflow
    active_workflow =
      (Pathname(__dir__).parent / ".github/workflows/ci.yml").read

    refute_equal active_workflow, STAGED_TIER1_CI_WORKFLOW_FIXTURE.read
  end

  def test_rejects_changed_tier1_matrix_workflow
    repository_root = Pathname(__dir__).parent
    workflow = (repository_root / ".github/workflows/ci.yml").read.sub(
      "macos-15-intel",
      "macos-14"
    )
    roadmap = <<~MARKDOWN
      # pycc Roadmap

      ## Current delivery status

      ### v0.1 acceptance checklist

      - [x] The five-target native CI matrix and one cross-host compilation path are live on `main`. <!-- roadmap-evidence: ci-tier1-cross-compile -->
    MARKDOWN

    _stdout, stderr, status = run_checker(roadmap: roadmap, workflow: workflow)

    refute status.success?
    assert_includes stderr, "does not match the reviewed Tier-1 CI workflow"
  end

  def test_requires_the_hard_coverage_gate_while_its_roadmap_claim_is_unchecked
    roadmap = <<~MARKDOWN
      # pycc Roadmap

      ## Current delivery status

      ### v0.1 acceptance checklist

      - [ ] The 100% line and region coverage gate is required and green for the current slice.
    MARKDOWN

    _stdout, stderr, status = run_checker(
      roadmap: roadmap,
      workflow: coverage_workflow("true")
    )

    refute status.success?
    assert_includes stderr, "does not provide the exact 100% line and region gate"
  end

  def test_rejects_coverage_evidence_when_the_threshold_is_lowered
    roadmap = <<~MARKDOWN
      # pycc Roadmap

      ## Current delivery status

      ### v0.1 acceptance checklist

      - [x] The 100% line and region coverage gate is required and green for the current slice. <!-- roadmap-evidence: ci-build-test-coverage-100 -->
    MARKDOWN

    _stdout, stderr, status = run_checker(
      roadmap: roadmap,
      workflow: coverage_workflow(
        "run_isolated \"$TRUSTED_COV\" llvm-cov --workspace " \
        "--fail-under-lines 99 --fail-under-regions 100"
      )
    )

    refute status.success?
    assert_includes stderr, "does not provide the exact 100% line and region gate"
  end

  def test_rejects_a_coverage_step_that_can_be_skipped
    roadmap = <<~MARKDOWN
      # pycc Roadmap

      ## Current delivery status

      ### v0.1 acceptance checklist

      - [x] The 100% line and region coverage gate is required and green for the current slice. <!-- roadmap-evidence: ci-build-test-coverage-100 -->
    MARKDOWN
    workflow = coverage_workflow.sub(
      "#{COVERAGE_STEP_HEADER}\n        run:",
      "#{COVERAGE_STEP_HEADER}\n        if: false\n        run:"
    )

    _stdout, stderr, status = run_checker(roadmap: roadmap, workflow: workflow)

    refute status.success?
    assert_includes stderr, "coverage evidence must run unconditionally"
  end

  def test_rejects_a_preceding_step_that_can_shadow_cargo
    roadmap = <<~MARKDOWN
      # pycc Roadmap

      ## Current delivery status

      ### v0.1 acceptance checklist

      - [x] The 100% line and region coverage gate is required and green for the current slice. <!-- roadmap-evidence: ci-build-test-coverage-100 -->
    MARKDOWN
    workflow = coverage_workflow.sub(
      "    steps:\n",
      "    steps:\n      - name: Shadow cargo\n        run: echo fake-cargo >> \"$GITHUB_PATH\"\n"
    )

    _stdout, stderr, status = run_checker(roadmap: roadmap, workflow: workflow)

    refute status.success?
    assert_includes stderr, "coverage setup steps do not match the trusted sequence"
  end

  def test_rejects_environment_that_can_shadow_coverage_tools
    roadmap = <<~MARKDOWN
      # pycc Roadmap

      ## Current delivery status

      ### v0.1 acceptance checklist

      - [x] The 100% line and region coverage gate is required and green for the current slice. <!-- roadmap-evidence: ci-build-test-coverage-100 -->
    MARKDOWN
    workflow = coverage_workflow.sub(
      "  CARGO_LLVM_COV_VERSION: \"0.8.7\"\n",
      "  CARGO_LLVM_COV_VERSION: \"0.8.7\"\n  PATH: /tmp/fake-bin\n"
    )

    _stdout, stderr, status = run_checker(roadmap: roadmap, workflow: workflow)

    refute status.success?
    assert_includes stderr, "coverage workflow environment does not match the trusted values"
  end

  def test_accepts_explicit_continue_on_error_false
    roadmap = <<~MARKDOWN
      # pycc Roadmap

      ## Current delivery status

      ### v0.1 acceptance checklist

      - [x] The 100% line and region coverage gate is required and green for the current slice. <!-- roadmap-evidence: ci-build-test-coverage-100 -->
    MARKDOWN
    workflow = coverage_workflow.sub(
      "#{COVERAGE_STEP_HEADER}\n        run:",
      "#{COVERAGE_STEP_HEADER}\n        continue-on-error: false\n        run:"
    )

    stdout, stderr, status = run_checker(roadmap: roadmap, workflow: workflow)

    assert status.success?, stderr
    assert_includes stdout, "Roadmap evidence policy passed."
  end

  def test_rejects_a_coverage_job_with_dependencies
    roadmap = <<~MARKDOWN
      # pycc Roadmap

      ## Current delivery status

      ### v0.1 acceptance checklist

      - [x] The 100% line and region coverage gate is required and green for the current slice. <!-- roadmap-evidence: ci-build-test-coverage-100 -->
    MARKDOWN
    workflow = coverage_workflow
               .sub(
                 "jobs:\n",
                 "jobs:\n  setup:\n    if: false\n    steps: []\n"
               )
               .sub(
                 "  build-test-coverage:\n",
                 "  build-test-coverage:\n    needs: setup\n"
               )

    _stdout, stderr, status = run_checker(roadmap: roadmap, workflow: workflow)

    refute status.success?
    assert_includes stderr, "coverage evidence must not depend on other jobs"
  end

  def test_rejects_continue_on_error_for_the_coverage_job
    roadmap = <<~MARKDOWN
      # pycc Roadmap

      ## Current delivery status

      ### v0.1 acceptance checklist

      - [x] The 100% line and region coverage gate is required and green for the current slice. <!-- roadmap-evidence: ci-build-test-coverage-100 -->
    MARKDOWN
    workflow = coverage_workflow.sub(
      "  build-test-coverage:\n",
      "  build-test-coverage:\n    continue-on-error: true\n"
    )

    _stdout, stderr, status = run_checker(roadmap: roadmap, workflow: workflow)

    refute status.success?
    assert_includes stderr, "coverage job must propagate failures"
  end

  def test_rejects_a_custom_shell_for_the_coverage_step
    roadmap = <<~MARKDOWN
      # pycc Roadmap

      ## Current delivery status

      ### v0.1 acceptance checklist

      - [x] The 100% line and region coverage gate is required and green for the current slice. <!-- roadmap-evidence: ci-build-test-coverage-100 -->
    MARKDOWN
    workflow = coverage_workflow.sub(
      "#{COVERAGE_STEP_HEADER}\n        run:",
      "#{COVERAGE_STEP_HEADER}\n        shell: 'true {0}'\n        run:"
    )

    _stdout, stderr, status = run_checker(roadmap: roadmap, workflow: workflow)

    refute status.success?
    assert_includes stderr, "coverage step must use the default shell"
  end

  def test_rejects_workflow_and_job_run_defaults
    roadmap = <<~MARKDOWN
      # pycc Roadmap

      ## Current delivery status

      ### v0.1 acceptance checklist

      - [x] The 100% line and region coverage gate is required and green for the current slice. <!-- roadmap-evidence: ci-build-test-coverage-100 -->
    MARKDOWN
    inherited_defaults = [
      coverage_workflow.sub(
        "jobs:\n",
        "defaults:\n  run:\n    shell: 'true {0}'\njobs:\n"
      ),
      coverage_workflow.sub(
        "  build-test-coverage:\n",
        "  build-test-coverage:\n    defaults:\n      run:\n        shell: 'true {0}'\n"
      )
    ]

    inherited_defaults.each do |workflow|
      _stdout, stderr, status = run_checker(roadmap: roadmap, workflow: workflow)

      refute status.success?
      assert_includes stderr, "coverage evidence must not inherit run defaults"
    end
  end

  def test_rejects_coverage_evidence_not_scheduled_for_pull_requests
    roadmap = <<~MARKDOWN
      # pycc Roadmap

      ## Current delivery status

      ### v0.1 acceptance checklist

      - [x] The 100% line and region coverage gate is required and green for the current slice. <!-- roadmap-evidence: ci-build-test-coverage-100 -->
    MARKDOWN
    workflow = coverage_workflow.sub("pull_request:", "workflow_dispatch:")

    _stdout, stderr, status = run_checker(roadmap: roadmap, workflow: workflow)

    refute status.success?
    assert_includes stderr, "coverage evidence must run on every pull request"
  end

  def test_repository_ci_runs_the_self_tests_and_checker
    repository_root = Pathname(__dir__).parent
    workflow = Psych.load((repository_root / ".github/workflows/ci.yml").read)
    run_blocks = workflow
                 .fetch("jobs")
                 .fetch("build-test-coverage")
                 .fetch("steps")
                 .map { |step| step["run"] }
                 .compact
    commands = run_blocks.flat_map { |run| run.lines.map(&:strip) }

    assert_includes commands, "ruby scripts/test_check_roadmap_evidence.rb"
    assert_includes commands, "ruby scripts/check_roadmap_evidence.rb"
    assert_includes commands,
                    "printf 'LLVM_SYS_221_PREFIX=%s\\n' " \
                    "\"$LLVM_SYS_221_PREFIX_VALUE\" >> \"$GITHUB_ENV\""
    assert_includes commands, 'sudo -u nobody env -i "${ISOLATED_ENV[@]}" "$@"'
    assert_includes commands, 'sudo chmod -R o+rX "$TRUSTED_TOOLCHAIN"'
    assert_includes commands,
                    'ln -s "$ISOLATED_ROOT/target" "$GITHUB_WORKSPACE/target"'
    assert_includes commands,
                    'run_isolated "$TRUSTED_CARGO" build ' \
                    "--target x86_64-apple-darwin -p pycc_rt"
    assert_operator commands.index("cargo build --workspace"),
                    :<,
                    commands.index("cargo test --workspace")
  end
end
