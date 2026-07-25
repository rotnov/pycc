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
  D48_ACTIVATION_WORKFLOW_FIXTURE =
    Pathname(__dir__).parent / "tests/fixtures/d48-activation-ci.yml"
  D48_STEADY_WORKFLOW_FIXTURE =
    Pathname(__dir__).parent / "tests/fixtures/d48-steady-ci.yml"
  ACTIVATION_HEAD_SHA = "a" * 40
  DIFFERENT_HEAD_SHA = "b" * 40
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

  def split_perf_workflow(steady: false)
    jobs = {
      "frontend-perf-measure" =>
        Marshal.load(Marshal.dump(SPLIT_PERF_MEASURE_JOB)),
      "frontend-perf-gate" =>
        Marshal.load(
          Marshal.dump(
            steady ? STEADY_SPLIT_PERF_GATE_JOB : SPLIT_PERF_GATE_JOB
          )
        ),
      "ci-gate" =>
        Marshal.load(Marshal.dump(SPLIT_PERF_CI_GATE_JOB))
    }
    yield jobs if block_given?
    { "jobs" => jobs }.to_yaml
  end

  def run_perf_baseline_validation(
    event_name:,
    github_ref:,
    base_ref: "main",
    base_sha: "current-main-sha",
    current_main_sha: "current-main-sha",
    pr_head_sha: ACTIVATION_HEAD_SHA,
    trusted_activation_head: ACTIVATION_HEAD_SHA,
    push_after_sha: "activation-sha",
    push_before_sha: "pre-split-sha",
    github_sha: "activation-sha",
    run_attempt: "1",
    workflow_path: Pathname(__dir__).parent / ".github/workflows/ci.yml",
    api_failure: false,
    baseline_present: false
  )
    Dir.mktmpdir do |directory|
      root = Pathname(directory)
      bin = root / "bin"
      FileUtils.mkdir_p(bin)
      gh = bin / "gh"
      gh.write(<<~'SHELL')
        #!/bin/sh
        if [ "${STUB_GH_FAIL:-0}" = "1" ]; then
          exit 42
        fi
        case "$*" in
          *"/git/ref/heads/main"*)
            printf '%s\n' "$STUB_CURRENT_MAIN_SHA"
            ;;
          *"/contents/.github/workflows/ci.yml"*)
            /bin/cat "$STUB_WORKFLOW"
            ;;
          *)
            echo "unexpected gh invocation: $*" >&2
            exit 43
            ;;
        esac
      SHELL
      FileUtils.chmod(0o755, gh)

      output = root / "github-output"
      output.write("")
      if baseline_present
        baseline = root / PERF_BASELINE_PATH / "estimates.json"
        FileUtils.mkdir_p(baseline.dirname)
        baseline.write("{}")
      end
      env = {
        "PATH" => "#{bin}:#{ENV.fetch('PATH')}",
        "GITHUB_OUTPUT" => output.to_s,
        "RUNNER_TEMP" => root.to_s,
        "GITHUB_EVENT_NAME" => event_name,
        "GITHUB_REF" => github_ref,
        "GITHUB_RUN_ATTEMPT" => run_attempt,
        "GITHUB_SHA" => github_sha,
        "GITHUB_REPOSITORY" => "rotnov/pycc",
        "PR_BASE_REF" => base_ref,
        "PR_BASE_SHA" => base_sha,
        "PR_HEAD_SHA" => pr_head_sha,
        "TRUSTED_ACTIVATION_HEAD" => trusted_activation_head,
        "PUSH_AFTER_SHA" => push_after_sha,
        "PUSH_BEFORE_SHA" => push_before_sha,
        "STUB_CURRENT_MAIN_SHA" => current_main_sha,
        "STUB_WORKFLOW" => workflow_path.to_s,
        "STUB_GH_FAIL" => api_failure ? "1" : "0"
      }
      stdout, stderr, status = Open3.capture3(
        env,
        "bash",
        "-s",
        stdin_data: PERF_BASELINE_VALIDATION_SCRIPT,
        chdir: root.to_s
      )
      return [stdout, stderr, status, output.read]
    end
  end

  def run_perf_baseline_lookup(
    run_rows: "",
    artifact_run_id: "",
    api_failure: false,
    event_name: "pull_request",
    pr_base_sha: "exact-base-sha",
    push_before_sha: "exact-before-sha"
  )
    Dir.mktmpdir do |directory|
      root = Pathname(directory)
      gh = root / "gh"
      gh.write(<<~'SHELL')
        #!/bin/sh
        if [ "${STUB_GH_FAIL:-0}" = "1" ]; then
          exit 42
        fi
        case "$*" in
          *"/actions/workflows/ci.yml/runs"*)
            case "$*" in
              *"head_sha=${STUB_EXPECTED_BASE_SHA}"*)
                printf '%b' "$STUB_RUN_ROWS"
                ;;
              *)
                echo "workflow-run query omitted the exact predecessor SHA" >&2
                exit 44
                ;;
            esac
            ;;
          *"/actions/runs/${STUB_ARTIFACT_RUN_ID}/artifacts"*)
            printf 'artifact-id\n'
            ;;
          *"/actions/runs/"*"/artifacts"*)
            ;;
          *)
            echo "unexpected gh invocation: $*" >&2
            exit 43
            ;;
        esac
      SHELL
      FileUtils.chmod(0o755, gh)
      output = root / "github-output"
      output.write("")
      env = {
        "PATH" => "#{root}:#{ENV.fetch('PATH')}",
        "GITHUB_OUTPUT" => output.to_s,
        "GITHUB_EVENT_NAME" => event_name,
        "GITHUB_REPOSITORY" => "rotnov/pycc",
        "PR_BASE_SHA" => pr_base_sha,
        "PUSH_BEFORE_SHA" => push_before_sha,
        "STUB_GH_FAIL" => api_failure ? "1" : "0",
        "STUB_EXPECTED_BASE_SHA" =>
          event_name == "pull_request" ? pr_base_sha : push_before_sha,
        "STUB_RUN_ROWS" => run_rows,
        "STUB_ARTIFACT_RUN_ID" => artifact_run_id
      }
      stdout, stderr, status = Open3.capture3(
        env,
        "bash",
        "-s",
        stdin_data: PERF_BASELINE_LOOKUP_SCRIPT,
        chdir: root.to_s
      )
      return [stdout, stderr, status, output.read]
    end
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

  def test_tier1_workflow_allowlist_stages_the_split_frontend_perf_gate_digest
    # Per docs/TESTING.md's staged-update procedure: the reviewed prospective
    # digests for activation and bootstrap-free steady state are appended while
    # the current digest remains active, ahead of the two pull requests that
    # activate the workflow and then retire the transition.
    assert_includes(
      TIER1_CI_WORKFLOW_SHA256S,
      D48_SPLIT_PERF_CI_WORKFLOW_SHA256
    )
    assert_includes(
      TIER1_CI_WORKFLOW_SHA256S,
      D48_STEADY_PERF_CI_WORKFLOW_SHA256
    )
  end

  def test_d48_workflow_digests_match_the_reviewed_inert_fixtures
    assert_equal(
      D48_SPLIT_PERF_CI_WORKFLOW_SHA256,
      Digest::SHA256.file(D48_ACTIVATION_WORKFLOW_FIXTURE).hexdigest
    )
    assert_equal(
      D48_STEADY_PERF_CI_WORKFLOW_SHA256,
      Digest::SHA256.file(D48_STEADY_WORKFLOW_FIXTURE).hexdigest
    )
    assert validate_perf_gate_baseline_lifecycle(
      D48_ACTIVATION_WORKFLOW_FIXTURE.read,
      D48_ACTIVATION_WORKFLOW_FIXTURE.to_s
    )
    assert validate_perf_gate_baseline_lifecycle(
      D48_STEADY_WORKFLOW_FIXTURE.read,
      D48_STEADY_WORKFLOW_FIXTURE.to_s
    )
  end

  def test_tier1_workflow_allowlist_retires_the_superseded_single_job_digest
    refute_includes(
      TIER1_CI_WORKFLOW_SHA256S,
      "0079c33c46c085277c4a84996a69a6c2d1777b34de9daf2e5d5e8f1923ceb27c"
    )
  end

  def test_accepts_the_reviewed_split_perf_trust_boundary
    assert validate_perf_gate_baseline_lifecycle(
      split_perf_workflow,
      "ci.yml"
    )
  end

  def test_accepts_the_reviewed_steady_state_perf_trust_boundary
    assert validate_perf_gate_baseline_lifecycle(
      split_perf_workflow(steady: true),
      "ci.yml"
    )
  end

  def test_rejects_steady_state_gate_without_a_canonical_baseline_requirement
    workflow = split_perf_workflow(steady: true) do |jobs|
      require_baseline = jobs.fetch("frontend-perf-gate").fetch("steps").find do |step|
        step["name"] == "Require canonical main frontend timing"
      end
      require_baseline["run"] = "true"
    end

    error = assert_raises(RoadmapEvidenceError) do
      validate_perf_gate_baseline_lifecycle(workflow, "ci.yml")
    end
    assert_includes error.message, "reviewed isolated comparison job"
  end

  def test_rejects_split_measurement_with_a_mutable_upload_action
    workflow = split_perf_workflow do |jobs|
      upload = jobs.fetch("frontend-perf-measure").fetch("steps").find do |step|
        step["name"] == "Upload current frontend timing"
      end
      upload["uses"] = "actions/upload-artifact@v4"
    end

    error = assert_raises(RoadmapEvidenceError) do
      validate_perf_gate_baseline_lifecycle(workflow, "ci.yml")
    end
    assert_includes error.message, "reviewed untrusted measurement job"
  end

  def test_rejects_split_gate_without_checker_hash_verification
    workflow = split_perf_workflow do |jobs|
      verify = jobs.fetch("frontend-perf-gate").fetch("steps").find do |step|
        step["name"] == "Verify reviewed performance checker"
      end
      verify["run"] = "true"
    end

    error = assert_raises(RoadmapEvidenceError) do
      validate_perf_gate_baseline_lifecycle(workflow, "ci.yml")
    end
    assert_includes error.message, "reviewed isolated comparison job"
  end

  def test_rejects_split_gate_with_a_mutable_download_action
    workflow = split_perf_workflow do |jobs|
      download = jobs.fetch("frontend-perf-gate").fetch("steps").find do |step|
        step["name"] == "Download current frontend timing"
      end
      download["uses"] = "actions/download-artifact@v4"
    end

    error = assert_raises(RoadmapEvidenceError) do
      validate_perf_gate_baseline_lifecycle(workflow, "ci.yml")
    end
    assert_includes error.message, "reviewed isolated comparison job"
  end

  def test_rejects_split_gate_that_can_ignore_a_download_failure
    workflow = split_perf_workflow do |jobs|
      download = jobs.fetch("frontend-perf-gate").fetch("steps").find do |step|
        step["name"] == "Download canonical main frontend timing"
      end
      download["continue-on-error"] = true
    end

    error = assert_raises(RoadmapEvidenceError) do
      validate_perf_gate_baseline_lifecycle(workflow, "ci.yml")
    end
    assert_includes error.message, "reviewed isolated comparison job"
  end

  def test_split_gate_never_uses_a_ref_ambiguous_actions_cache
    refute_match(
      %r{actions/cache},
      SPLIT_PERF_GATE_JOB.to_s
    )
  end

  def test_rejects_split_gate_that_queries_pull_request_runs_for_a_baseline
    workflow = split_perf_workflow do |jobs|
      locate = jobs.fetch("frontend-perf-gate").fetch("steps").find do |step|
        step["name"] == "Locate latest successful main baseline"
      end
      locate["run"] = locate.fetch("run").sub("-f event=push", "-f event=pull_request")
    end

    error = assert_raises(RoadmapEvidenceError) do
      validate_perf_gate_baseline_lifecycle(workflow, "ci.yml")
    end
    assert_includes error.message, "reviewed isolated comparison job"
  end

  def test_rejects_split_gate_that_accepts_an_expired_main_artifact
    workflow = split_perf_workflow do |jobs|
      locate = jobs.fetch("frontend-perf-gate").fetch("steps").find do |step|
        step["name"] == "Locate latest successful main baseline"
      end
      locate["run"] = locate.fetch("run").sub(
        "select(.expired == false)",
        "select(.expired == true)"
      )
    end

    error = assert_raises(RoadmapEvidenceError) do
      validate_perf_gate_baseline_lifecycle(workflow, "ci.yml")
    end
    assert_includes error.message, "reviewed isolated comparison job"
  end

  def test_rejects_split_gate_without_explicit_successful_main_run_provenance
    workflow = split_perf_workflow do |jobs|
      download = jobs.fetch("frontend-perf-gate").fetch("steps").find do |step|
        step["name"] == "Download canonical main frontend timing"
      end
      download.fetch("with").delete("run-id")
    end

    error = assert_raises(RoadmapEvidenceError) do
      validate_perf_gate_baseline_lifecycle(workflow, "ci.yml")
    end
    assert_includes error.message, "reviewed isolated comparison job"
  end

  def test_rejects_split_gate_with_a_reusable_missing_baseline_bootstrap
    workflow = split_perf_workflow do |jobs|
      validate = jobs.fetch("frontend-perf-gate").fetch("steps").find do |step|
        step["name"] == "Require a main-owned baseline or reviewed activation"
      end
      validate["run"] = validate.fetch("run").sub(
        PRE_SPLIT_PERF_CI_WORKFLOW_SHA256,
        D48_SPLIT_PERF_CI_WORKFLOW_SHA256
      )
    end

    error = assert_raises(RoadmapEvidenceError) do
      validate_perf_gate_baseline_lifecycle(workflow, "ci.yml")
    end
    assert_includes error.message, "reviewed isolated comparison job"
  end

  def test_baseline_validation_accepts_the_bound_activation_pull_request
    _stdout, stderr, status, output = run_perf_baseline_validation(
      event_name: "pull_request",
      github_ref: "refs/pull/86/merge"
    )

    assert status.success?, stderr
    assert_equal "bootstrap=true\n", output
  end

  def test_baseline_validation_rejects_a_stale_activation_pull_request
    _stdout, stderr, status, output = run_perf_baseline_validation(
      event_name: "pull_request",
      github_ref: "refs/pull/86/merge",
      current_main_sha: "new-main-sha"
    )

    refute status.success?
    assert_includes stderr, "base is stale"
    assert_empty output
  end

  def test_baseline_validation_rejects_a_replayed_activation_pull_request
    _stdout, stderr, status, output = run_perf_baseline_validation(
      event_name: "pull_request",
      github_ref: "refs/pull/86/merge",
      run_attempt: "2"
    )

    refute status.success?
    assert_includes stderr, "pull request cannot be replayed"
    assert_empty output
  end

  def test_baseline_validation_rejects_a_non_main_pull_request
    _stdout, stderr, status, output = run_perf_baseline_validation(
      event_name: "pull_request",
      github_ref: "refs/pull/86/merge",
      base_ref: "release"
    )

    refute status.success?
    assert_includes stderr, "only for a pull request targeting main"
    assert_empty output
  end

  def test_baseline_validation_rejects_a_fresh_run_for_a_new_pull_request_head
    _stdout, stderr, status, output = run_perf_baseline_validation(
      event_name: "pull_request",
      github_ref: "refs/pull/86/merge",
      pr_head_sha: DIFFERENT_HEAD_SHA
    )

    refute status.success?
    assert_includes stderr, "bound to one exact pull request head"
    assert_empty output
  end

  def test_baseline_validation_rejects_an_invalid_trusted_activation_head
    _stdout, stderr, status, output = run_perf_baseline_validation(
      event_name: "pull_request",
      github_ref: "refs/pull/86/merge",
      trusted_activation_head: "#{ACTIVATION_HEAD_SHA}\n#{DIFFERENT_HEAD_SHA}"
    )

    refute status.success?
    assert_includes stderr, "not one lowercase 40-hex commit SHA"
    assert_empty output
  end

  def test_baseline_validation_accepts_the_first_main_push
    _stdout, stderr, status, output = run_perf_baseline_validation(
      event_name: "push",
      github_ref: "refs/heads/main",
      current_main_sha: "activation-sha"
    )

    assert status.success?, stderr
    assert_equal "bootstrap=true\n", output
  end

  def test_baseline_validation_rejects_a_replayed_activation_push
    _stdout, stderr, status, output = run_perf_baseline_validation(
      event_name: "push",
      github_ref: "refs/heads/main",
      current_main_sha: "activation-sha",
      run_attempt: "2"
    )

    refute status.success?
    assert_includes stderr, "cannot be replayed"
    assert_empty output
  end

  def test_baseline_validation_rejects_a_push_that_is_not_the_live_main_head
    _stdout, stderr, status, output = run_perf_baseline_validation(
      event_name: "push",
      github_ref: "refs/heads/main",
      current_main_sha: "newer-main-sha"
    )

    refute status.success?
    assert_includes stderr, "not the live main head"
    assert_empty output
  end

  def test_baseline_validation_rejects_a_push_event_sha_mismatch
    _stdout, stderr, status, output = run_perf_baseline_validation(
      event_name: "push",
      github_ref: "refs/heads/main",
      push_after_sha: "different-after-sha"
    )

    refute status.success?
    assert_includes stderr, "does not match the checked-out main commit"
    assert_empty output
  end

  def test_baseline_validation_rejects_an_activation_push_to_another_ref
    _stdout, stderr, status, output = run_perf_baseline_validation(
      event_name: "push",
      github_ref: "refs/heads/release"
    )

    refute status.success?
    assert_includes stderr, "must target refs/heads/main"
    assert_empty output
  end

  def test_baseline_validation_fails_closed_after_activation
    _stdout, stderr, status, output = run_perf_baseline_validation(
      event_name: "pull_request",
      github_ref: "refs/pull/86/merge",
      workflow_path: CHECKER
    )

    refute status.success?
    assert_includes stderr, "not the reviewed one-time activation"
    assert_empty output
  end

  def test_baseline_validation_propagates_api_failure
    _stdout, _stderr, status, output = run_perf_baseline_validation(
      event_name: "pull_request",
      github_ref: "refs/pull/86/merge",
      api_failure: true
    )

    assert_equal 42, status.exitstatus
    assert_empty output
  end

  def test_baseline_validation_accepts_a_downloaded_main_baseline_without_bootstrap
    _stdout, stderr, status, output = run_perf_baseline_validation(
      event_name: "pull_request",
      github_ref: "refs/pull/999/merge",
      base_ref: "release",
      baseline_present: true
    )

    assert status.success?, stderr
    assert_equal "bootstrap=false\n", output
  end

  def test_baseline_lookup_propagates_api_failure
    _stdout, _stderr, status, output = run_perf_baseline_lookup(
      api_failure: true
    )

    assert_equal 42, status.exitstatus
    assert_empty output
  end

  def test_baseline_lookup_selects_the_newest_non_expired_main_artifact
    _stdout, stderr, status, output = run_perf_baseline_lookup(
      run_rows: "300\\texact-base-sha\\n200\\texact-base-sha\\n",
      artifact_run_id: "300"
    )

    assert status.success?, stderr
    assert_equal "found=true\nrun_id=300\n", output
  end

  def test_baseline_lookup_skips_a_run_without_a_non_expired_artifact
    _stdout, stderr, status, output = run_perf_baseline_lookup(
      run_rows: "300\\texact-base-sha\\n200\\texact-base-sha\\n",
      artifact_run_id: "200"
    )

    assert status.success?, stderr
    assert_equal "found=true\nrun_id=200\n", output
  end

  def test_baseline_lookup_reports_no_artifact
    _stdout, stderr, status, output = run_perf_baseline_lookup(
      run_rows: "300\\texact-base-sha\\n200\\texact-base-sha\\n"
    )

    assert status.success?, stderr
    assert_equal "found=false\n", output
  end

  def test_baseline_lookup_rejects_a_successful_run_for_an_older_main_sha
    _stdout, stderr, status, output = run_perf_baseline_lookup(
      run_rows: "300\\tolder-main-sha\\n200\\texact-base-sha\\n",
      artifact_run_id: "300"
    )

    assert status.success?, stderr
    assert_equal "found=false\n", output
  end

  def test_baseline_lookup_uses_the_push_predecessor_sha
    _stdout, stderr, status, output = run_perf_baseline_lookup(
      event_name: "push",
      run_rows: "300\\texact-before-sha\\n",
      artifact_run_id: "300"
    )

    assert status.success?, stderr
    assert_equal "found=true\nrun_id=300\n", output
  end

  def test_rejects_split_gate_without_a_measurement_dependency
    workflow = split_perf_workflow do |jobs|
      jobs.fetch("frontend-perf-gate").delete("needs")
    end

    error = assert_raises(RoadmapEvidenceError) do
      validate_perf_gate_baseline_lifecycle(workflow, "ci.yml")
    end
    assert_includes error.message, "reviewed isolated comparison job"
  end

  def test_rejects_a_split_gate_without_a_measurement_job
    workflow = split_perf_workflow do |jobs|
      jobs.delete("frontend-perf-measure")
    end

    error = assert_raises(RoadmapEvidenceError) do
      validate_perf_gate_baseline_lifecycle(workflow, "ci.yml")
    end
    assert_includes error.message, "requires frontend-perf-measure"
  end

  def test_rejects_split_perf_jobs_not_required_by_ci_gate
    workflow = split_perf_workflow do |jobs|
      jobs.fetch("ci-gate").fetch("needs").delete("frontend-perf-measure")
    end

    error = assert_raises(RoadmapEvidenceError) do
      validate_perf_gate_baseline_lifecycle(workflow, "ci.yml")
    end
    assert_includes error.message, "reviewed fail-closed aggregate job"
  end

  def test_rejects_a_noop_split_perf_ci_gate
    workflow = split_perf_workflow do |jobs|
      jobs.fetch("ci-gate")["steps"] = [{ "run" => "true" }]
    end

    error = assert_raises(RoadmapEvidenceError) do
      validate_perf_gate_baseline_lifecycle(workflow, "ci.yml")
    end
    assert_includes error.message, "reviewed fail-closed aggregate job"
  end

  def test_rejects_a_split_perf_ci_gate_that_can_be_skipped
    workflow = split_perf_workflow do |jobs|
      jobs.fetch("ci-gate").delete("if")
    end

    error = assert_raises(RoadmapEvidenceError) do
      validate_perf_gate_baseline_lifecycle(workflow, "ci.yml")
    end
    assert_includes error.message, "reviewed fail-closed aggregate job"
  end

  def test_rejects_each_missing_split_perf_ci_gate_result_check
    SPLIT_PERF_CI_GATE_NEEDS.each do |job_name|
      workflow = split_perf_workflow do |jobs|
        gate_step = jobs.fetch("ci-gate").fetch("steps").first
        predicate = "needs.#{job_name}.result != 'success'"
        gate_step["if"] = gate_step.fetch("if").sub(
          /(?: \|\| )?#{Regexp.escape(predicate)}/,
          ""
        )
      end

      error = assert_raises(RoadmapEvidenceError, job_name) do
        validate_perf_gate_baseline_lifecycle(workflow, "ci.yml")
      end
      assert_includes(
        error.message,
        "reviewed fail-closed aggregate job",
        job_name
      )
    end
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
