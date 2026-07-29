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
  ACTIVE_D84_THROUGHPUT_FLOOR_WORKFLOW =
    Pathname(__dir__).parent / ".github/workflows/ci.yml"
  RETIRED_D51_PAIRED_WORKFLOW =
    Pathname(__dir__).parent / "tests/fixtures/d51-paired-ci.yml"
  D56_SOURCE_AWARE_WORKFLOW_FIXTURE =
    Pathname(__dir__).parent / "tests/fixtures/d56-source-aware-ci.yml"
  D62_REPLICATED_PAIRED_WORKFLOW_FIXTURE =
    Pathname(__dir__).parent / "tests/fixtures/d62-replicated-paired-ci.yml"
  D80_CONFORMANCE_ORACLE_WORKFLOW_FIXTURE =
    Pathname(__dir__).parent / "tests/fixtures/d80-conformance-oracle-ci.yml"
  D84_THROUGHPUT_FLOOR_WORKFLOW_FIXTURE =
    Pathname(__dir__).parent / "tests/fixtures/d84-throughput-floor-ci.yml"
  D91_RELAX_FRONTEND_PERF_MANIFEST_WORKFLOW_FIXTURE =
    Pathname(__dir__).parent /
    "tests/fixtures/d91-relax-frontend-perf-manifest-ci.yml"
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

  def paired_perf_workflow
    jobs = {
      "frontend-perf-measure" =>
        Marshal.load(Marshal.dump(PAIRED_PERF_MEASURE_JOB)),
      "frontend-perf-gate" =>
        Marshal.load(Marshal.dump(PAIRED_PERF_GATE_JOB)),
      "ci-gate" =>
        Marshal.load(Marshal.dump(PAIRED_PERF_CI_GATE_JOB))
    }
    yield jobs if block_given?
    { "jobs" => jobs }.to_yaml
  end

  def source_aware_perf_workflow
    jobs = {
      "frontend-perf-measure" =>
        Marshal.load(Marshal.dump(D56_SOURCE_AWARE_PERF_MEASURE_JOB)),
      "frontend-perf-gate" =>
        Marshal.load(Marshal.dump(D56_SOURCE_AWARE_PERF_GATE_JOB)),
      "ci-gate" =>
        Marshal.load(Marshal.dump(PAIRED_PERF_CI_GATE_JOB))
    }
    yield jobs if block_given?
    { "jobs" => jobs }.to_yaml
  end

  def replicated_perf_workflow
    jobs = {
      "frontend-perf-measure" =>
        Marshal.load(Marshal.dump(REPLICATED_PERF_MEASURE_JOB)),
      "frontend-perf-gate" =>
        Marshal.load(Marshal.dump(REPLICATED_PERF_GATE_JOB)),
      "ci-gate" =>
        Marshal.load(Marshal.dump(PAIRED_PERF_CI_GATE_JOB))
    }
    yield jobs if block_given?
    { "jobs" => jobs }.to_yaml
  end

  def without_workflow_jobs(workflow, *job_names)
    skipping = false
    workflow.lines.reject do |line|
      if (match = /^  (?<name>[a-z0-9-]+):\s*$/.match(line))
        skipping = job_names.include?(match[:name])
      end
      skipping
    end.join
  end

  def roadmap_with_tier1_claim(state)
    claim =
      "The five-target native CI matrix and one cross-host compilation " \
      "path are live on `main`."
    item =
      case state
      when :unchecked
        "- [ ] #{claim}"
      when :absent
        nil
      else
        raise ArgumentError, "unsupported Tier-1 claim state: #{state}"
      end
    ["# pycc Roadmap", item].compact.join("\n") + "\n"
  end

  def retired_d48_workflow
    coverage_workflow.sub(
      "jobs:\n",
      <<~YAML
        jobs:
          frontend-perf-measure:
            runs-on: macos-14
            steps:
              - uses: actions/checkout@d23441a48e516b6c34aea4fa41551a30e30af803
              - name: Run frontend benchmark
                run: cargo bench --bench check_bench -- --save-baseline current
          frontend-perf-gate:
            needs: frontend-perf-measure
            runs-on: macos-14
            steps:
              - name: Compare against canonical main baseline
                run: ruby scripts/check_perf_regression.rb current.json previous.json
      YAML
    )
  end

  def run_paired_predecessor_resolution(
    event_name:,
    pr_base_sha: "exact-base-sha",
    push_before_sha: "exact-before-sha"
  )
    Dir.mktmpdir do |directory|
      output = Pathname(directory) / "github-output"
      output.write("")
      env = {
        "GITHUB_OUTPUT" => output.to_s,
        "GITHUB_EVENT_NAME" => event_name,
        "PR_BASE_SHA" => pr_base_sha,
        "PUSH_BEFORE_SHA" => push_before_sha
      }
      stdout, stderr, status = Open3.capture3(
        env,
        "bash",
        "-s",
        stdin_data: PAIRED_PERF_PREDECESSOR_SCRIPT,
        chdir: directory
      )
      return [stdout, stderr, status, output.read]
    end
  end

  def run_paired_timing_requirement
    Dir.mktmpdir do |directory|
      root =
        Pathname(directory) /
        "target/criterion/pycc_check_frontend_fixture"
      FileUtils.mkdir_p(root)
      yield root
      return Open3.capture3(
        "bash",
        "-s",
        stdin_data: PAIRED_PERF_REQUIRE_SCRIPT,
        chdir: directory
      )
    end
  end

  def run_replicated_timing_requirement
    Dir.mktmpdir do |directory|
      root =
        Pathname(directory) /
        "target/criterion/pycc_check_frontend_fixture"
      FileUtils.mkdir_p(root)
      yield root
      return Open3.capture3(
        "bash",
        "-s",
        stdin_data: REPLICATED_PERF_REQUIRE_SCRIPT,
        chdir: directory
      )
    end
  end

  def run_paired_artifact_identity_requirement(
    previous_artifact_id:,
    current_artifact_id:
  )
    Open3.capture3(
      {
        "PREVIOUS_ARTIFACT_ID" => previous_artifact_id,
        "CURRENT_ARTIFACT_ID" => current_artifact_id
      },
      "bash",
      "-s",
      stdin_data: PAIRED_PERF_ARTIFACT_ID_REQUIRE_SCRIPT
    )
  end


  def run_executable_input_classifier
    Dir.mktmpdir do |directory|
      root = Pathname(directory)
      %w[previous current].each do |revision|
        FileUtils.mkdir_p(root / revision / "src")
        FileUtils.mkdir_p(root / revision / "crates")
      end
      yield root
      output = root / "github-output"
      output.write("")
      stdout, stderr, status = Open3.capture3(
        { "GITHUB_OUTPUT" => output.to_s },
        "bash",
        "-s",
        stdin_data: D56_EXECUTABLE_INPUT_IDENTITY_SCRIPT,
        chdir: root.to_s
      )
      return [stdout, stderr, status, output.read]
    end
  end

  # D-091: same shape as run_executable_input_classifier, but exercises the
  # actual D91_EXECUTABLE_INPUT_IDENTITY_SCRIPT (which adds Cargo.toml,
  # Cargo.lock, and build.rs to the classified path list) instead of D56's
  # src/crates-only predecessor.
  def run_d91_executable_input_classifier
    Dir.mktmpdir do |directory|
      root = Pathname(directory)
      %w[previous current].each do |revision|
        FileUtils.mkdir_p(root / revision / "src")
        FileUtils.mkdir_p(root / revision / "crates")
      end
      yield root
      output = root / "github-output"
      output.write("")
      stdout, stderr, status = Open3.capture3(
        { "GITHUB_OUTPUT" => output.to_s },
        "bash",
        "-s",
        stdin_data: D91_EXECUTABLE_INPUT_IDENTITY_SCRIPT,
        chdir: root.to_s
      )
      return [stdout, stderr, status, output.read]
    end
  end

  # D-091: exercises the real D91_VERIFY_REVISIONS_SCRIPT end to end,
  # including its `git -C previous/current rev-parse HEAD` preamble, against
  # real (throwaway) git repositories -- not just a substring check of the
  # constant's text -- so a change that silently breaks the bench-manifest
  # fingerprint's awk/grep logic actually fails a test, per the same
  # measurement-integrity finding this fingerprint was added to fix.
  def run_d91_verify_revisions
    Dir.mktmpdir do |directory|
      root = Pathname(directory)
      %w[previous current].each do |revision|
        FileUtils.mkdir_p(root / revision)
      end
      yield root
      shas = {}
      %w[previous current].each do |revision|
        repo = (root / revision).to_s
        Open3.capture2("git", "-C", repo, "init", "-q")
        Open3.capture2("git", "-C", repo, "config", "user.email", "test@example.invalid")
        Open3.capture2("git", "-C", repo, "config", "user.name", "Test")
        Open3.capture2("git", "-C", repo, "add", "-A")
        _out, commit_err, commit_status =
          Open3.capture3("git", "-C", repo, "commit", "-q", "-m", "content")
        raise commit_err unless commit_status.success?

        sha, = Open3.capture2("git", "-C", repo, "rev-parse", "HEAD")
        shas[revision] = sha.strip
      end
      env = {
        "EXPECTED_PREDECESSOR_SHA" => shas.fetch("previous"),
        "EXPECTED_CURRENT_SHA" => shas.fetch("current")
      }
      return Open3.capture3(
        env,
        "bash",
        "-s",
        stdin_data: D91_VERIFY_REVISIONS_SCRIPT,
        chdir: root.to_s
      )
    end
  end

  D91_BENCH_MANIFEST_TAIL = <<~TOML
    [dev-dependencies]
    serde_json = "1"
    criterion = { version = "0.8.2", features = ["html_reports"] }

    [[bench]]
    name = "check_bench"
    harness = false
  TOML

  def d91_cargo_toml(dependencies_extra: "", bench_manifest_tail: D91_BENCH_MANIFEST_TAIL)
    <<~TOML
      [package]
      name = "pycc"
      version = "0.1.0"

      [dependencies]
      clap = { version = "4", features = ["derive"] }
      #{dependencies_extra}
      #{bench_manifest_tail}
    TOML
  end

  def run_executable_input_identity_requirement(value)
    Open3.capture3(
      { "EXECUTABLE_INPUTS_EQUAL" => value },
      "bash",
      "-s",
      stdin_data: D56_EXECUTABLE_INPUT_IDENTITY_REQUIRE_SCRIPT
    )
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
        workflow: ACTIVE_D84_THROUGHPUT_FLOOR_WORKFLOW.read
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
      workflow: ACTIVE_D84_THROUGHPUT_FLOOR_WORKFLOW.read
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
      workflow: ACTIVE_D84_THROUGHPUT_FLOOR_WORKFLOW.read
    )

    assert status.success?, stderr
    assert_includes stdout, "Roadmap evidence policy passed."
  end

  def test_ignores_checked_items_rendered_as_indented_code
    ["    - [x] Root code example.\n", ">     - [x] Quoted code example.\n"].each do |example|
      stdout, stderr, status = run_checker(
        roadmap: "# pycc Roadmap\n\n#{example}",
        workflow: ACTIVE_D84_THROUGHPUT_FLOOR_WORKFLOW.read
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

  def test_accepts_conformance_tier1_evidence
    repository_root = Pathname(__dir__).parent
    workflow = (repository_root / ".github/workflows/ci.yml").read
    roadmap = <<~MARKDOWN
      # pycc Roadmap

      ## Current delivery status

      ### v0.1 acceptance checklist

      - [x] `fib` and `mandelbrot-ascii` compile and match CPython output on all five Tier-1 targets. <!-- roadmap-evidence: conformance-fib-mandelbrot-tier1 -->
    MARKDOWN

    stdout, stderr, status = run_checker(roadmap: roadmap, workflow: workflow)

    assert status.success?, stderr
    assert_includes stdout, "Roadmap evidence policy passed."
  end

  def test_rejects_conformance_tier1_evidence_with_the_wrong_claim
    roadmap = <<~MARKDOWN
      # pycc Roadmap

      ## Current delivery status

      ### v0.1 acceptance checklist

      - [x] `fib` and `mandelbrot-ascii` compile on all five Tier-1 targets. <!-- roadmap-evidence: conformance-fib-mandelbrot-tier1 -->
    MARKDOWN

    _stdout, stderr, status = run_checker(roadmap: roadmap, workflow: coverage_workflow)

    refute status.success?
    assert_includes stderr, "does not prove this roadmap claim"
  end

  def test_rejects_conformance_tier1_evidence_outside_the_v0_1_checklist
    roadmap = <<~MARKDOWN
      # pycc Roadmap

      ## v1.0 — spec freeze

      ### v0.1 acceptance checklist

      - [x] `fib` and `mandelbrot-ascii` compile and match CPython output on all five Tier-1 targets. <!-- roadmap-evidence: conformance-fib-mandelbrot-tier1 -->
    MARKDOWN

    _stdout, stderr, status = run_checker(roadmap: roadmap, workflow: coverage_workflow)

    refute status.success?
    assert_includes stderr, "must appear under the expected roadmap section"
  end

  def test_accepts_throughput_floor_evidence
    repository_root = Pathname(__dir__).parent
    workflow = (repository_root / ".github/workflows/ci.yml").read
    roadmap = <<~MARKDOWN
      # pycc Roadmap

      ## Current delivery status

      ### v0.1 acceptance checklist

      - [x] `pycc check` processes 1k LOC in under 50 ms. <!-- roadmap-evidence: check-throughput-1k-loc-50ms -->
    MARKDOWN

    stdout, stderr, status = run_checker(roadmap: roadmap, workflow: workflow)

    assert status.success?, stderr
    assert_includes stdout, "Roadmap evidence policy passed."
  end

  def test_rejects_throughput_floor_evidence_with_the_wrong_claim
    roadmap = <<~MARKDOWN
      # pycc Roadmap

      ## Current delivery status

      ### v0.1 acceptance checklist

      - [x] `pycc check` processes 1k LOC in under 5 seconds. <!-- roadmap-evidence: check-throughput-1k-loc-50ms -->
    MARKDOWN

    _stdout, stderr, status = run_checker(roadmap: roadmap, workflow: coverage_workflow)

    refute status.success?
    assert_includes stderr, "does not prove this roadmap claim"
  end

  def test_rejects_throughput_floor_evidence_outside_the_v0_1_checklist
    roadmap = <<~MARKDOWN
      # pycc Roadmap

      ## v1.0 — spec freeze

      ### v0.1 acceptance checklist

      - [x] `pycc check` processes 1k LOC in under 50 ms. <!-- roadmap-evidence: check-throughput-1k-loc-50ms -->
    MARKDOWN

    _stdout, stderr, status = run_checker(roadmap: roadmap, workflow: coverage_workflow)

    refute status.success?
    assert_includes stderr, "must appear under the expected roadmap section"
  end

  def test_accepts_cli_spec_diagnostic_evidence
    repository_root = Pathname(__dir__).parent
    workflow = (repository_root / ".github/workflows/ci.yml").read
    roadmap = <<~MARKDOWN
      # pycc Roadmap

      ## Current delivery status

      ### v0.1 acceptance checklist

      - [x] The error demonstration matches the stable [CLI specification](./CLI_SPEC.md) output. <!-- roadmap-evidence: cli-spec-diagnostic-match -->
    MARKDOWN

    stdout, stderr, status = run_checker(roadmap: roadmap, workflow: workflow)

    assert status.success?, stderr
    assert_includes stdout, "Roadmap evidence policy passed."
  end

  def test_rejects_cli_spec_diagnostic_evidence_with_the_wrong_claim
    roadmap = <<~MARKDOWN
      # pycc Roadmap

      ## Current delivery status

      ### v0.1 acceptance checklist

      - [x] The error demonstration matches CLI_SPEC.md. <!-- roadmap-evidence: cli-spec-diagnostic-match -->
    MARKDOWN

    _stdout, stderr, status = run_checker(roadmap: roadmap, workflow: coverage_workflow)

    refute status.success?
    assert_includes stderr, "does not prove this roadmap claim"
  end

  def test_rejects_cli_spec_diagnostic_evidence_outside_the_v0_1_checklist
    roadmap = <<~MARKDOWN
      # pycc Roadmap

      ## v1.0 — spec freeze

      ### v0.1 acceptance checklist

      - [x] The error demonstration matches the stable [CLI specification](./CLI_SPEC.md) output. <!-- roadmap-evidence: cli-spec-diagnostic-match -->
    MARKDOWN

    _stdout, stderr, status = run_checker(roadmap: roadmap, workflow: coverage_workflow)

    refute status.success?
    assert_includes stderr, "must appear under the expected roadmap section"
  end

  def test_tier1_workflow_authorization_is_the_active_d84_digest
    assert_equal(
      D84_THROUGHPUT_FLOOR_CI_WORKFLOW_SHA256,
      Digest::SHA256.hexdigest(
        (Pathname(__dir__).parent / ".github/workflows/ci.yml").read
      )
    )
  end

  def test_tier1_workflow_authorization_contains_only_active_d84_and_staged_d91
    assert_equal(
      [
        D84_THROUGHPUT_FLOOR_CI_WORKFLOW_SHA256,
        D91_RELAX_FRONTEND_PERF_MANIFEST_CI_WORKFLOW_SHA256
      ],
      REVIEWED_PERF_CI_WORKFLOW_SHA256S
    )
  end

  # D-090's own fixture is gone: it was staged but never activated, and
  # was found (while opening PR-8's own pull request) to be missing the
  # coverage-sandbox release build D-091 adds -- see D-091's comment in
  # check_roadmap_evidence.rb for the full correction. Its digest constant
  # remains only as a historical record that it was once reviewed and
  # staged, matching D51/D56/D62/D80's own "no longer accepted" pattern.

  # Staged, not yet active: D91's fixture is D84's own live content (the
  # current, unmodified `ci.yml`) plus D-090's originally-intended
  # release-mode `pycc_rt` build step, the coverage-sandbox release build
  # that step's own staged fixture was missing, and the relaxed manifest
  # contract -- the actual composed content PR-8's activation needs.
  def test_d91_relax_frontend_perf_manifest_workflow_digest_matches_the_reviewed_fixture
    assert_equal(
      D91_RELAX_FRONTEND_PERF_MANIFEST_CI_WORKFLOW_SHA256,
      Digest::SHA256.file(D91_RELAX_FRONTEND_PERF_MANIFEST_WORKFLOW_FIXTURE).hexdigest
    )
    assert validate_source_aware_perf_gate_lifecycle(
      D91_RELAX_FRONTEND_PERF_MANIFEST_WORKFLOW_FIXTURE.read,
      D91_RELAX_FRONTEND_PERF_MANIFEST_WORKFLOW_FIXTURE.to_s
    )
  end

  # `coverage_gate_present?`/`COVERAGE_SCRIPT` DO model part of
  # build-test-coverage (the exact body of its "Hard coverage gate" step),
  # unlike the frontend-perf-measure job the lifecycle validator above
  # checks. D-091's own release-profile build line inside that step (see
  # check_roadmap_evidence.rb's COVERAGE_SCRIPT) must keep matching this
  # fixture's actual content, or the activation commit that copies this
  # fixture into ci.yml would fail its own "Check roadmap evidence" step.
  def test_d91_relax_frontend_perf_manifest_workflow_still_has_a_recognized_coverage_gate
    assert coverage_gate_present?(
      D91_RELAX_FRONTEND_PERF_MANIFEST_WORKFLOW_FIXTURE.read,
      D91_RELAX_FRONTEND_PERF_MANIFEST_WORKFLOW_FIXTURE.to_s
    )
  end

  # D-091: the bench-manifest fingerprint (`[dev-dependencies]` onward) must
  # hard-abort on a change to the bench-defining tail itself -- otherwise a
  # PR that only speeds up `criterion`/`[[bench]]` could pass the perf gate
  # as though it sped up the compiler. Confirmed by executing the real
  # script, not by reading its source.
  def test_d91_bench_manifest_fingerprint_hard_aborts_on_bench_tooling_change
    _stdout, stderr, status = run_d91_verify_revisions do |root|
      (root / "previous/Cargo.toml").write(d91_cargo_toml)
      (root / "current/Cargo.toml").write(
        d91_cargo_toml(
          bench_manifest_tail: D91_BENCH_MANIFEST_TAIL.sub("0.8.2", "0.9.0")
        )
      )
    end
    refute status.success?, "expected a criterion version bump to hard-abort, got: #{stderr}"
  end

  # D-091: an ordinary product dependency addition (PR-8's own toml/serde
  # shape) must NOT trip the bench-manifest fingerprint -- only the
  # `[dev-dependencies]`-onward tail is hard-required identical.
  def test_d91_bench_manifest_fingerprint_allows_product_dependency_only_change
    _stdout, stderr, status = run_d91_verify_revisions do |root|
      (root / "previous/Cargo.toml").write(d91_cargo_toml)
      (root / "current/Cargo.toml").write(
        d91_cargo_toml(dependencies_extra: %(toml = "0.8"\nserde = "1"))
      )
    end
    assert status.success?, stderr
  end

  # D-091: the fingerprint's own invariant guard must fail loudly, not
  # silently mis-scope, if a manifest is missing `[dev-dependencies]`
  # entirely.
  def test_d91_bench_manifest_fingerprint_hard_aborts_when_dev_dependencies_is_missing
    _stdout, stderr, status = run_d91_verify_revisions do |root|
      (root / "previous/Cargo.toml").write(d91_cargo_toml)
      (root / "current/Cargo.toml").write(<<~TOML)
        [package]
        name = "pycc"
        version = "0.1.0"

        [dependencies]
        clap = { version = "4", features = ["derive"] }
      TOML
    end
    refute status.success?
    assert_includes stderr, "bench-manifest fingerprint invariant violated"
  end

  # D-091: the guard must also fail loudly if a future manifest reorders
  # sections so something other than `[[bench]]` follows
  # `[dev-dependencies]`, rather than silently widening the hard-required
  # region to swallow an otherwise-reclassifiable dependency.
  def test_d91_bench_manifest_fingerprint_hard_aborts_on_unexpected_trailing_section
    _stdout, stderr, status = run_d91_verify_revisions do |root|
      (root / "previous/Cargo.toml").write(d91_cargo_toml)
      (root / "current/Cargo.toml").write(
        d91_cargo_toml(bench_manifest_tail: "#{D91_BENCH_MANIFEST_TAIL}\n[extra]\nfoo = 1\n")
      )
    end
    refute status.success?
    assert_includes stderr, "unexpected section"
  end

  # D-091: root-level build.rs must be classified (Cargo would silently use
  # it without any Cargo.toml change), while an unrelated identical src/
  # tree still reports executable_inputs_equal=true.
  def test_d91_classifier_reports_identical_and_added_build_rs
    _stdout, stderr, status, output = run_d91_executable_input_classifier do |root|
      (root / "previous/src/lib.rs").write("same\n")
      (root / "current/src/lib.rs").write("same\n")
    end
    assert status.success?, stderr
    assert_equal "executable_inputs_equal=true\n", output

    _stdout, stderr, status, output = run_d91_executable_input_classifier do |root|
      (root / "previous/src/lib.rs").write("same\n")
      (root / "current/src/lib.rs").write("same\n")
      (root / "current/build.rs").write("fn main() {}\n")
    end
    assert status.success?, stderr
    assert_equal "executable_inputs_equal=false\n", output
  end

  def test_tier1_workflow_allowlist_retires_the_pre_alpha_eval_digest
    refute_equal(
      D80_CONFORMANCE_ORACLE_CI_WORKFLOW_SHA256,
      "58e2d5026b59e7c921b57c882d24b6507c95dd8f99e390c0a68af217e5e038c8"
    )
  end

  def test_tier1_workflow_allowlist_retains_the_active_d80_digest
    assert_equal(
      "17611d861d10c34d6ccebbf21bc82d8dfaf006b969bb2fe1e12d57b9e9c81234",
      D80_CONFORMANCE_ORACLE_CI_WORKFLOW_SHA256
    )
  end

  def test_tier1_workflow_allowlist_retires_the_d48_steady_digest
    refute_equal(
      D80_CONFORMANCE_ORACLE_CI_WORKFLOW_SHA256,
      "940b342845a9fc600d72195a0a382ce9437f3cb123cc62f8805b8cb82ae35f56"
    )
  end

  def test_tier1_workflow_allowlist_retires_the_d62_replicated_digest
    refute_equal(
      D80_CONFORMANCE_ORACLE_CI_WORKFLOW_SHA256,
      D62_REPLICATED_SOURCE_AWARE_PERF_CI_WORKFLOW_SHA256
    )
  end

  def test_retired_d51_paired_workflow_remains_a_reviewed_audit_fixture
    assert_equal(
      D51_PAIRED_PERF_CI_WORKFLOW_SHA256,
      Digest::SHA256.file(RETIRED_D51_PAIRED_WORKFLOW).hexdigest
    )
    assert validate_perf_gate_baseline_lifecycle(
      RETIRED_D51_PAIRED_WORKFLOW.read,
      RETIRED_D51_PAIRED_WORKFLOW.to_s
    )
  end

  def test_d56_source_aware_workflow_remains_reviewed_audit_evidence
    assert D56_SOURCE_AWARE_WORKFLOW_FIXTURE.file?
    assert_equal(
      D56_SOURCE_AWARE_PERF_CI_WORKFLOW_SHA256,
      Digest::SHA256.file(D56_SOURCE_AWARE_WORKFLOW_FIXTURE).hexdigest
    )
    assert validate_source_aware_perf_gate_lifecycle(
      D56_SOURCE_AWARE_WORKFLOW_FIXTURE.read,
      D56_SOURCE_AWARE_WORKFLOW_FIXTURE.to_s
    )
    refute_equal(
      D51_PAIRED_PERF_CI_WORKFLOW_SHA256,
      D56_SOURCE_AWARE_PERF_CI_WORKFLOW_SHA256
    )
    assert_equal(
      D56_PERF_CHECKER_SHA256,
      Digest::SHA256.file(
        Pathname(__dir__).parent / "scripts/check_source_aware_perf_regression.rb"
      ).hexdigest
    )
    assert_equal(
      D56_PERF_CHECKER_TEST_SHA256,
      Digest::SHA256.file(
        Pathname(__dir__).parent / "scripts/test_check_source_aware_perf_regression.rb"
      ).hexdigest
    )
  end

  def test_d56_classifier_reports_identical_and_changed_executable_inputs
    _stdout, stderr, status, output = run_executable_input_classifier do |root|
      (root / "previous/src/lib.rs").write("same\n")
      (root / "current/src/lib.rs").write("same\n")
    end
    assert status.success?, stderr
    assert_equal "executable_inputs_equal=true\n", output

    _stdout, stderr, status, output = run_executable_input_classifier do |root|
      (root / "previous/crates/lib.rs").write("old\n")
      (root / "current/crates/lib.rs").write("new\n")
    end
    assert status.success?, stderr
    assert_equal "executable_inputs_equal=false\n", output
  end

  def test_d56_gate_accepts_only_a_boolean_executable_input_identity
    %w[true false].each do |value|
      _stdout, stderr, status =
        run_executable_input_identity_requirement(value)
      assert status.success?, stderr
    end

    ["", "unknown", "TRUE", "0"].each do |value|
      _stdout, stderr, status =
        run_executable_input_identity_requirement(value)
      refute status.success?
      assert_includes stderr, "invalid executable-input identity"
    end
  end

  def test_d56_rejects_missing_executable_input_output
    workflow = source_aware_perf_workflow do |jobs|
      jobs.fetch("frontend-perf-measure").fetch("outputs").delete(
        "executable_inputs_equal"
      )
    end

    error = assert_raises(RoadmapEvidenceError) do
      validate_source_aware_perf_gate_lifecycle(workflow, "ci.yml")
    end
    assert_includes error.message, "reviewed source-aware measurement job"
  end

  def test_d56_rejects_classifier_after_candidate_execution
    workflow = source_aware_perf_workflow do |jobs|
      steps = jobs.fetch("frontend-perf-measure").fetch("steps")
      classifier_index = steps.index do |step|
        step["name"] == "Classify executable benchmark inputs"
      end
      classifier = steps.delete_at(classifier_index)
      candidate_index = steps.index do |step|
        step["name"] == "Benchmark exact candidate"
      end
      steps.insert(candidate_index + 1, classifier)
    end

    error = assert_raises(RoadmapEvidenceError) do
      validate_source_aware_perf_gate_lifecycle(workflow, "ci.yml")
    end
    assert_includes error.message, "reviewed source-aware measurement job"
  end

  def test_d56_rejects_an_incomplete_executable_input_set
    %w[src crates].each do |path|
      workflow = source_aware_perf_workflow do |jobs|
        classifier = jobs.fetch("frontend-perf-measure").fetch("steps").find do |step|
          step["name"] == "Classify executable benchmark inputs"
        end
        classifier["run"] = classifier.fetch("run").sub(
          "for executable_path in src crates",
          "for executable_path in #{path}"
        )
      end

      error = assert_raises(RoadmapEvidenceError) do
        validate_source_aware_perf_gate_lifecycle(workflow, "ci.yml")
      end
      assert_includes error.message, "reviewed source-aware measurement job"
    end
  end

  def test_d56_rejects_gate_without_identity_validation_or_comparator_binding
    ["Require executable-input identity", "Compare exact predecessor and candidate"].each do |step_name|
      workflow = source_aware_perf_workflow do |jobs|
        steps = jobs.fetch("frontend-perf-gate").fetch("steps")
        step = steps.find { |candidate| candidate["name"] == step_name }
        step.delete("env")
      end

      error = assert_raises(RoadmapEvidenceError) do
        validate_source_aware_perf_gate_lifecycle(workflow, "ci.yml")
      end
      assert_includes error.message, "reviewed source-aware comparison job"
    end
  end

  def test_d62_replicated_workflow_remains_a_reviewed_audit_fixture
    assert_equal(
      D62_REPLICATED_SOURCE_AWARE_PERF_CI_WORKFLOW_SHA256,
      Digest::SHA256.file(D62_REPLICATED_PAIRED_WORKFLOW_FIXTURE).hexdigest
    )
    assert validate_source_aware_perf_gate_lifecycle(
      D62_REPLICATED_PAIRED_WORKFLOW_FIXTURE.read,
      D62_REPLICATED_PAIRED_WORKFLOW_FIXTURE.to_s
    )
  end

  def test_d84_throughput_floor_workflow_is_active_and_reviewed
    assert_equal(
      D84_THROUGHPUT_FLOOR_CI_WORKFLOW_SHA256,
      Digest::SHA256.file(D84_THROUGHPUT_FLOOR_WORKFLOW_FIXTURE).hexdigest
    )
    assert_equal(
      D84_THROUGHPUT_FLOOR_CI_WORKFLOW_SHA256,
      Digest::SHA256.file(ACTIVE_D84_THROUGHPUT_FLOOR_WORKFLOW).hexdigest
    )
    assert_equal(
      D84_THROUGHPUT_FLOOR_WORKFLOW_FIXTURE.read,
      ACTIVE_D84_THROUGHPUT_FLOOR_WORKFLOW.read
    )
    assert_equal(
      REPLICATED_PERF_CHECKER_SHA256,
      Digest::SHA256.file(
        Pathname(__dir__).parent /
          "scripts/check_replicated_paired_perf_regression.rb"
      ).hexdigest
    )
    assert_equal(
      REPLICATED_PERF_CHECKER_TEST_SHA256,
      Digest::SHA256.file(
        Pathname(__dir__).parent /
          "scripts/test_check_replicated_paired_perf_regression.rb"
      ).hexdigest
    )
    assert validate_source_aware_perf_gate_lifecycle(
      ACTIVE_D84_THROUGHPUT_FLOOR_WORKFLOW.read,
      ACTIVE_D84_THROUGHPUT_FLOOR_WORKFLOW.to_s
    )
  end

  def test_d80_conformance_oracle_workflow_remains_a_reviewed_audit_fixture
    assert_equal(
      D80_CONFORMANCE_ORACLE_CI_WORKFLOW_SHA256,
      Digest::SHA256.file(D80_CONFORMANCE_ORACLE_WORKFLOW_FIXTURE).hexdigest
    )
    assert validate_source_aware_perf_gate_lifecycle(
      D80_CONFORMANCE_ORACLE_WORKFLOW_FIXTURE.read,
      D80_CONFORMANCE_ORACLE_WORKFLOW_FIXTURE.to_s
    )
  end

  def test_tier1_workflow_allowlist_retires_the_superseded_single_job_digest
    refute_equal(
      D62_REPLICATED_SOURCE_AWARE_PERF_CI_WORKFLOW_SHA256,
      "0079c33c46c085277c4a84996a69a6c2d1777b34de9daf2e5d5e8f1923ceb27c"
    )
  end

  def test_accepts_the_reviewed_paired_perf_trust_boundary
    assert validate_perf_gate_baseline_lifecycle(
      paired_perf_workflow,
      "ci.yml"
    )
  end

  def test_accepts_the_reviewed_fixed_replicate_perf_trust_boundary
    assert validate_source_aware_perf_gate_lifecycle(
      replicated_perf_workflow,
      "ci.yml"
    )
  end

  def test_public_cli_accepts_the_active_d84_workflow
    stdout, stderr, status = run_checker(
      roadmap: roadmap_with_tier1_claim(:absent),
      workflow: D84_THROUGHPUT_FLOOR_WORKFLOW_FIXTURE.read
    )

    assert status.success?, stderr
    assert_includes stdout, "Roadmap evidence policy passed."
  end

  def test_public_cli_rejects_drift_in_the_active_d84_workflow
    workflow = D84_THROUGHPUT_FLOOR_WORKFLOW_FIXTURE.read.sub(
      "for round in 1 2 3 4 5; do",
      "for round in 1 2 3; do"
    )

    _stdout, stderr, status = run_checker(
      roadmap: roadmap_with_tier1_claim(:absent),
      workflow: workflow
    )

    refute status.success?
    assert_includes stderr, "does not match the reviewed active D-084 performance CI workflow"
  end

  def test_public_cli_rejects_an_active_workflow_without_both_perf_jobs
    workflow = without_workflow_jobs(
      D84_THROUGHPUT_FLOOR_WORKFLOW_FIXTURE.read,
      "frontend-perf-measure",
      "frontend-perf-gate"
    )

    _stdout, stderr, status = run_checker(
      roadmap: roadmap_with_tier1_claim(:absent),
      workflow: workflow
    )

    refute status.success?
    assert_includes stderr, "does not match the reviewed active D-084 performance CI workflow"
  end

  def test_public_cli_rejects_retired_d48_with_unchecked_tier1_claim
    _stdout, stderr, status = run_checker(
      roadmap: roadmap_with_tier1_claim(:unchecked),
      workflow: retired_d48_workflow
    )

    refute status.success?
    assert_includes stderr, "does not match the reviewed active D-084 performance CI workflow"
  end

  def test_public_cli_rejects_retired_d48_without_a_tier1_claim
    _stdout, stderr, status = run_checker(
      roadmap: roadmap_with_tier1_claim(:absent),
      workflow: retired_d48_workflow
    )

    refute status.success?
    assert_includes stderr, "does not match the reviewed active D-084 performance CI workflow"
  end

  def test_public_cli_requires_active_digest_without_a_tier1_claim
    workflow =
      D84_THROUGHPUT_FLOOR_WORKFLOW_FIXTURE.read + "\n# unreviewed drift\n"

    _stdout, stderr, status = run_checker(
      roadmap: roadmap_with_tier1_claim(:absent),
      workflow: workflow
    )

    refute status.success?
    assert_includes stderr, "does not match the reviewed active D-084 performance CI workflow"
  end

  def test_public_cli_rejects_the_retired_d56_workflow
    _stdout, stderr, status = run_checker(
      roadmap: roadmap_with_tier1_claim(:absent),
      workflow: D56_SOURCE_AWARE_WORKFLOW_FIXTURE.read
    )

    refute status.success?
    assert_includes stderr, "does not match the reviewed active D-084 performance CI workflow"
  end

  def test_public_cli_rejects_the_retired_d51_workflow
    _stdout, stderr, status = run_checker(
      roadmap: roadmap_with_tier1_claim(:absent),
      workflow: RETIRED_D51_PAIRED_WORKFLOW.read
    )

    refute status.success?
    assert_includes stderr, "does not match the reviewed active D-084 performance CI workflow"
  end

  def test_public_cli_rejects_the_retired_d62_workflow
    _stdout, stderr, status = run_checker(
      roadmap: roadmap_with_tier1_claim(:absent),
      workflow: D62_REPLICATED_PAIRED_WORKFLOW_FIXTURE.read
    )

    refute status.success?
    assert_includes stderr, "does not match the reviewed active D-084 performance CI workflow"
  end

  def test_public_cli_rejects_the_retired_d80_workflow
    _stdout, stderr, status = run_checker(
      roadmap: roadmap_with_tier1_claim(:absent),
      workflow: D80_CONFORMANCE_ORACLE_WORKFLOW_FIXTURE.read
    )

    refute status.success?
    assert_includes stderr, "does not match the reviewed active D-084 performance CI workflow"
  end

  def test_public_cli_rejects_unreviewed_d56_workflow_drift
    workflow =
      D56_SOURCE_AWARE_WORKFLOW_FIXTURE.read + "\n# unreviewed drift\n"
    _stdout, stderr, status = run_checker(
      roadmap: roadmap_with_tier1_claim(:absent),
      workflow: workflow
    )

    refute status.success?
    assert_includes stderr, "does not match the reviewed active D-084 performance CI workflow"
  end

  def test_paired_measurement_resolves_the_exact_pull_request_base
    _stdout, stderr, status, output = run_paired_predecessor_resolution(
      event_name: "pull_request"
    )

    assert status.success?, stderr
    assert_equal "sha=exact-base-sha\n", output
  end

  def test_paired_measurement_resolves_the_exact_push_predecessor
    _stdout, stderr, status, output = run_paired_predecessor_resolution(
      event_name: "push"
    )

    assert status.success?, stderr
    assert_equal "sha=exact-before-sha\n", output
  end

  def test_paired_measurement_rejects_an_unsupported_event
    _stdout, stderr, status, output = run_paired_predecessor_resolution(
      event_name: "workflow_dispatch"
    )

    refute status.success?
    assert_includes stderr, "cannot resolve a performance predecessor"
    assert_empty output
  end

  def test_paired_measurement_rejects_missing_or_zero_predecessors
    [
      ["pull_request", "", "unused"],
      ["push", "unused", "0000000000000000000000000000000000000000"]
    ].each do |event_name, pr_base_sha, push_before_sha|
      _stdout, stderr, status, output = run_paired_predecessor_resolution(
        event_name: event_name,
        pr_base_sha: pr_base_sha,
        push_before_sha: push_before_sha
      )

      refute status.success?
      assert_includes stderr, "cannot resolve the exact performance predecessor SHA"
      assert_empty output
    end
  end

  def test_rejects_paired_measurement_without_an_exact_candidate_checkout
    workflow = paired_perf_workflow do |jobs|
      checkout = jobs.fetch("frontend-perf-measure").fetch("steps").find do |step|
        step["name"] == "Check out candidate"
      end
      checkout.fetch("with")["ref"] = "${{ github.event.pull_request.head.sha }}"
    end

    error = assert_raises(RoadmapEvidenceError) do
      validate_perf_gate_baseline_lifecycle(workflow, "ci.yml")
    end
    assert_includes error.message, "reviewed paired measurement job"
  end

  def test_rejects_paired_measurement_without_any_benchmark_contract_path
    %w[
      benches
      Cargo.toml
      Cargo.lock
      rust-toolchain.toml
      rust-toolchain
      .cargo
    ].each do |path|
      workflow = paired_perf_workflow do |jobs|
        verify = jobs.fetch("frontend-perf-measure").fetch("steps").find do |step|
          step["name"] == "Verify exact benchmark revisions"
        end
        verify["run"] = verify.fetch("run").sub("  #{path}\n", "")
      end

      error = assert_raises(RoadmapEvidenceError) do
        validate_perf_gate_baseline_lifecycle(workflow, "ci.yml")
      end
      assert_includes error.message, "reviewed paired measurement job"
    end
  end

  def test_rejects_paired_measurement_without_local_manifest_and_build_script_binding
    [
      "':(glob)crates/**/Cargo.toml'",
      "':(glob)**/build.rs'"
    ].each do |pathspec|
      workflow = paired_perf_workflow do |jobs|
        verify = jobs.fetch("frontend-perf-measure").fetch("steps").find do |step|
          step["name"] == "Verify exact benchmark revisions"
        end
        verify["run"] = verify.fetch("run").sub(pathspec, "':(glob)missing'")
      end

      error = assert_raises(RoadmapEvidenceError) do
        validate_perf_gate_baseline_lifecycle(workflow, "ci.yml")
      end
      assert_includes error.message, "reviewed paired measurement job"
    end
  end

  def test_rejects_paired_measurement_without_both_revision_checks
    [
      'test "$(git -C previous rev-parse HEAD)" = "$EXPECTED_PREDECESSOR_SHA"',
      'test "$(git -C current rev-parse HEAD)" = "$EXPECTED_CURRENT_SHA"'
    ].each do |revision_check|
      workflow = paired_perf_workflow do |jobs|
        verify = jobs.fetch("frontend-perf-measure").fetch("steps").find do |step|
          step["name"] == "Verify exact benchmark revisions"
        end
        verify["run"] = verify.fetch("run").sub(revision_check, "true")
      end

      error = assert_raises(RoadmapEvidenceError) do
        validate_perf_gate_baseline_lifecycle(workflow, "ci.yml")
      end
      assert_includes error.message, "reviewed paired measurement job"
    end
  end

  def test_rejects_paired_measurement_that_runs_candidate_before_sealing_predecessor
    workflow = paired_perf_workflow do |jobs|
      steps = jobs.fetch("frontend-perf-measure").fetch("steps")
      upload_index = steps.index do |step|
        step["name"] == "Upload sealed predecessor frontend timing"
      end
      upload = steps.delete_at(upload_index)
      candidate_index = steps.index do |step|
        step["name"] == "Benchmark exact candidate"
      end
      steps.insert(candidate_index + 1, upload)
    end

    error = assert_raises(RoadmapEvidenceError) do
      validate_perf_gate_baseline_lifecycle(workflow, "ci.yml")
    end
    assert_includes error.message, "reviewed paired measurement job"
  end

  def test_rejects_paired_measurement_with_shared_target_state
    workflow = paired_perf_workflow do |jobs|
      benchmark = jobs.fetch("frontend-perf-measure").fetch("steps").find do |step|
        step["name"] == "Benchmark exact candidate"
      end
      benchmark["run"] = benchmark.fetch("run").sub(
        'current_target="$RUNNER_TEMP/pycc-paired-perf-current"',
        'current_target="$RUNNER_TEMP/pycc-paired-perf-previous"'
      )
    end

    error = assert_raises(RoadmapEvidenceError) do
      validate_perf_gate_baseline_lifecycle(workflow, "ci.yml")
    end
    assert_includes error.message, "reviewed paired measurement job"
  end

  def test_rejects_mutable_actions_in_the_paired_jobs
    [
      ["frontend-perf-measure", "Check out candidate", "actions/checkout@v6"],
      ["frontend-perf-measure", "Upload sealed predecessor frontend timing", "actions/upload-artifact@v4"],
      ["frontend-perf-measure", "Upload candidate frontend timing", "actions/upload-artifact@v4"],
      ["frontend-perf-gate", "Download sealed predecessor frontend timing", "actions/download-artifact@v4"],
      ["frontend-perf-gate", "Download candidate frontend timing", "actions/download-artifact@v4"]
    ].each do |job_name, step_name, mutable_action|
      workflow = paired_perf_workflow do |jobs|
        step = jobs.fetch(job_name).fetch("steps").find do |candidate|
          candidate["name"] == step_name
        end
        step["uses"] = mutable_action
      end

      error = assert_raises(RoadmapEvidenceError) do
        validate_perf_gate_baseline_lifecycle(workflow, "ci.yml")
      end
      assert_includes error.message, "reviewed"
    end
  end

  def test_rejects_paired_uploads_that_include_a_whole_timing_directory
    [
      ["Upload sealed predecessor frontend timing", "previous_benchmark"],
      ["Upload candidate frontend timing", "current_benchmark"]
    ].each do |step_name, step_id|
      workflow = paired_perf_workflow do |jobs|
        upload = jobs.fetch("frontend-perf-measure").fetch("steps").find do |step|
          step["name"] == step_name
        end
        upload.fetch("with")["path"] =
          "${{ steps.#{step_id}.outputs.timing_dir }}"
      end

      error = assert_raises(RoadmapEvidenceError) do
        validate_perf_gate_baseline_lifecycle(workflow, "ci.yml")
      end
      assert_includes error.message, "reviewed paired measurement job"
    end
  end

  def test_rejects_paired_gate_that_checks_out_a_head_controlled_comparator
    workflow = paired_perf_workflow do |jobs|
      checkout = jobs.fetch("frontend-perf-gate").fetch("steps").find do |step|
        step["name"] == "Check out only the reviewed performance checker"
      end
      checkout.fetch("with").delete("ref")
    end

    error = assert_raises(RoadmapEvidenceError) do
      validate_perf_gate_baseline_lifecycle(workflow, "ci.yml")
    end
    assert_includes error.message, "reviewed paired comparison job"
  end

  def test_paired_gate_requires_distinct_numeric_artifact_identities
    _stdout, stderr, status = run_paired_artifact_identity_requirement(
      previous_artifact_id: "123",
      current_artifact_id: "456"
    )
    assert status.success?, stderr

    [
      ["", "456"],
      ["previous", "456"],
      ["123", ""],
      ["123", "current"],
      ["123", "123"]
    ].each do |previous_artifact_id, current_artifact_id|
      _stdout, stderr, status = run_paired_artifact_identity_requirement(
        previous_artifact_id: previous_artifact_id,
        current_artifact_id: current_artifact_id
      )
      refute status.success?
      assert_match(/invalid artifact identity|distinct artifact identities/, stderr)
    end
  end

  def test_rejects_paired_gate_that_downloads_artifacts_by_name
    [
      [
        "Download sealed predecessor frontend timing",
        "frontend-perf-previous"
      ],
      ["Download candidate frontend timing", "frontend-perf-current"]
    ].each do |step_name, artifact_name|
      workflow = paired_perf_workflow do |jobs|
        download = jobs.fetch("frontend-perf-gate").fetch("steps").find do |step|
          step["name"] == step_name
        end
        download.fetch("with").delete("artifact-ids")
        download.fetch("with")["name"] = artifact_name
      end

      error = assert_raises(RoadmapEvidenceError) do
        validate_perf_gate_baseline_lifecycle(workflow, "ci.yml")
      end
      assert_includes error.message, "reviewed paired comparison job"
    end
  end

  def test_rejects_paired_gate_without_valid_flat_id_bound_downloads
    [
      "Download sealed predecessor frontend timing",
      "Download candidate frontend timing"
    ].each do |step_name|
      [[:missing, nil], [:disabled, false], [:invalid, "flatten"]].each do |mutation, value|
        workflow = paired_perf_workflow do |jobs|
          download = jobs.fetch("frontend-perf-gate").fetch("steps").find do |step|
            step["name"] == step_name
          end
          inputs = download.fetch("with")
          if mutation == :missing
            inputs.delete("merge-multiple")
          else
            inputs["merge-multiple"] = value
          end
        end

        error = assert_raises(RoadmapEvidenceError) do
          validate_perf_gate_baseline_lifecycle(workflow, "ci.yml")
        end
        assert_includes error.message, "reviewed paired comparison job"
      end
    end
  end

  def test_paired_timing_requirement_accepts_exactly_two_regular_files
    _stdout, stderr, status = run_paired_timing_requirement do |root|
      %w[previous current].each do |revision|
        FileUtils.mkdir_p(root / revision)
        (root / revision / "estimates.json").write("{}")
      end
    end

    assert status.success?, stderr
  end

  def test_paired_timing_requirement_rejects_either_missing_estimate
    %w[previous current].each do |present_revision|
      _stdout, stderr, status = run_paired_timing_requirement do |root|
        FileUtils.mkdir_p(root / present_revision)
        (root / present_revision / "estimates.json").write("{}")
      end

      refute status.success?
      missing_revision =
        present_revision == "previous" ? "current" : "previous"
      assert_includes stderr, "missing #{missing_revision}/estimates.json"
    end
  end

  def test_paired_timing_requirement_rejects_an_extra_file
    _stdout, stderr, status = run_paired_timing_requirement do |root|
      %w[previous current].each do |revision|
        FileUtils.mkdir_p(root / revision)
        (root / revision / "estimates.json").write("{}")
      end
      (root / "unexpected.txt").write("extra")
    end

    refute status.success?
    assert_includes stderr, "exactly two regular files"
  end

  def test_paired_timing_requirement_rejects_a_symlink
    _stdout, stderr, status = run_paired_timing_requirement do |root|
      FileUtils.mkdir_p(root / "previous")
      FileUtils.mkdir_p(root / "current")
      (root / "previous" / "estimates.json").write("{}")
      File.symlink(
        root / "previous" / "estimates.json",
        root / "current" / "estimates.json"
      )
    end

    refute status.success?
    assert_includes stderr, "missing current/estimates.json"
  end

  def test_replicated_timing_requirement_accepts_exactly_ten_regular_files
    _stdout, stderr, status = run_replicated_timing_requirement do |root|
      %w[previous current].each do |revision|
        FileUtils.mkdir_p(root / revision)
        5.times do |index|
          (root / revision / "round-#{index + 1}.json").write("{}")
        end
      end
    end

    assert status.success?, stderr
  end

  def test_replicated_timing_requirement_rejects_a_missing_fixed_sample
    %w[previous current].each do |missing_revision|
      _stdout, stderr, status = run_replicated_timing_requirement do |root|
        %w[previous current].each do |revision|
          FileUtils.mkdir_p(root / revision)
          5.times do |index|
            next if revision == missing_revision && index == 2

            (root / revision / "round-#{index + 1}.json").write("{}")
          end
        end
      end

      refute status.success?
      assert_includes stderr, "missing #{missing_revision}/round-3.json"
    end
  end

  def test_replicated_timing_requirement_rejects_an_extra_file
    _stdout, stderr, status = run_replicated_timing_requirement do |root|
      %w[previous current].each do |revision|
        FileUtils.mkdir_p(root / revision)
        5.times do |index|
          (root / revision / "round-#{index + 1}.json").write("{}")
        end
      end
      (root / "unexpected.txt").write("extra")
    end

    refute status.success?
    assert_includes stderr, "exactly two directories and ten regular files"
  end

  def test_replicated_timing_requirement_rejects_an_extra_empty_directory
    _stdout, stderr, status = run_replicated_timing_requirement do |root|
      %w[previous current].each do |revision|
        FileUtils.mkdir_p(root / revision)
        5.times do |index|
          (root / revision / "round-#{index + 1}.json").write("{}")
        end
      end
      FileUtils.mkdir_p(root / "unexpected")
    end

    refute status.success?
    assert_includes stderr, "exactly two directories and ten regular files"
  end

  def test_replicated_timing_requirement_rejects_a_symlink
    _stdout, stderr, status = run_replicated_timing_requirement do |root|
      %w[previous current].each do |revision|
        FileUtils.mkdir_p(root / revision)
        5.times do |index|
          (root / revision / "round-#{index + 1}.json").write("{}")
        end
      end
      File.unlink(root / "current/round-5.json")
      File.symlink(root / "current/round-4.json", root / "current/round-5.json")
    end

    refute status.success?
    assert_includes stderr, "missing current/round-5.json"
  end

  def test_rejects_replicated_measurement_with_a_changed_fixed_sample_count
    ["Benchmark exact predecessor", "Benchmark exact candidate"].each do |step_name|
      workflow = replicated_perf_workflow do |jobs|
        benchmark = jobs.fetch("frontend-perf-measure").fetch("steps").find do |step|
          step["name"] == step_name
        end
        benchmark["run"] = benchmark.fetch("run").sub(
          "for round in 1 2 3 4 5; do",
          "for round in 1 2 3; do"
        )
      end

      error = assert_raises(RoadmapEvidenceError) do
        validate_source_aware_perf_gate_lifecycle(workflow, "ci.yml")
      end
      assert_includes error.message, "reviewed source-aware measurement job"
    end
  end

  def test_rejects_replicated_gate_with_a_paired_single_sample_comparator
    workflow = replicated_perf_workflow do |jobs|
      jobs["frontend-perf-gate"] =
        Marshal.load(Marshal.dump(PAIRED_PERF_GATE_JOB))
    end

    error = assert_raises(RoadmapEvidenceError) do
      validate_source_aware_perf_gate_lifecycle(workflow, "ci.yml")
    end
    assert_includes error.message, "reviewed source-aware comparison job"
  end

  def test_rejects_replicated_measurement_that_runs_candidate_before_sealing_predecessor
    workflow = replicated_perf_workflow do |jobs|
      steps = jobs.fetch("frontend-perf-measure").fetch("steps")
      upload_index = steps.index do |step|
        step["name"] == "Upload sealed predecessor frontend timing"
      end
      upload = steps.delete_at(upload_index)
      candidate_index = steps.index do |step|
        step["name"] == "Benchmark exact candidate"
      end
      steps.insert(candidate_index + 1, upload)
    end

    error = assert_raises(RoadmapEvidenceError) do
      validate_source_aware_perf_gate_lifecycle(workflow, "ci.yml")
    end
    assert_includes error.message, "reviewed source-aware measurement job"
  end

  def test_rejects_mutable_actions_in_the_replicated_jobs
    [
      ["frontend-perf-measure", "Upload candidate frontend timing", "actions/upload-artifact@v4"],
      ["frontend-perf-gate", "Download candidate frontend timing", "actions/download-artifact@v4"]
    ].each do |job_name, step_name, mutable_action|
      workflow = replicated_perf_workflow do |jobs|
        step = jobs.fetch(job_name).fetch("steps").find do |candidate|
          candidate["name"] == step_name
        end
        step["uses"] = mutable_action
      end

      error = assert_raises(RoadmapEvidenceError) do
        validate_source_aware_perf_gate_lifecycle(workflow, "ci.yml")
      end
      assert_includes error.message, "reviewed"
    end
  end

  def test_rejects_replicated_gate_without_flat_id_bound_downloads
    workflow = replicated_perf_workflow do |jobs|
      download = jobs.fetch("frontend-perf-gate").fetch("steps").find do |step|
        step["name"] == "Download candidate frontend timing"
      end
      download.fetch("with").delete("merge-multiple")
    end

    error = assert_raises(RoadmapEvidenceError) do
      validate_source_aware_perf_gate_lifecycle(workflow, "ci.yml")
    end
    assert_includes error.message, "reviewed source-aware comparison job"
  end

  def test_rejects_replicated_gate_with_a_head_controlled_comparator
    workflow = replicated_perf_workflow do |jobs|
      checkout = jobs.fetch("frontend-perf-gate").fetch("steps").find do |step|
        step["name"] == "Check out only the reviewed performance checker"
      end
      checkout.fetch("with").delete("ref")
    end

    error = assert_raises(RoadmapEvidenceError) do
      validate_source_aware_perf_gate_lifecycle(workflow, "ci.yml")
    end
    assert_includes error.message, "reviewed source-aware comparison job"
  end

  def test_rejects_replicated_gate_without_executable_input_binding
    workflow = replicated_perf_workflow do |jobs|
      compare = jobs.fetch("frontend-perf-gate").fetch("steps").find do |step|
        step["name"] == "Compare exact predecessor and candidate"
      end
      compare.delete("env")
    end

    error = assert_raises(RoadmapEvidenceError) do
      validate_source_aware_perf_gate_lifecycle(workflow, "ci.yml")
    end
    assert_includes error.message, "reviewed source-aware comparison job"
  end

  def test_rejects_paired_gate_that_can_skip_the_comparison
    workflow = paired_perf_workflow do |jobs|
      compare = jobs.fetch("frontend-perf-gate").fetch("steps").find do |step|
        step["name"] == "Compare exact predecessor and candidate"
      end
      compare["if"] = "${{ false }}"
    end

    error = assert_raises(RoadmapEvidenceError) do
      validate_perf_gate_baseline_lifecycle(workflow, "ci.yml")
    end
    assert_includes error.message, "reviewed paired comparison job"
  end

  def test_rejects_a_retired_d48_like_comparison_gate
    workflow = paired_perf_workflow do |jobs|
      gate = jobs.fetch("frontend-perf-gate")
      compare = gate.fetch("steps").find do |step|
        step["name"] == "Compare exact predecessor and candidate"
      end
      compare["name"] = "Compare against canonical main baseline"
      compare["run"] =
        "ruby scripts/check_perf_regression.rb current.json previous.json"
    end

    error = assert_raises(RoadmapEvidenceError) do
      validate_perf_gate_baseline_lifecycle(workflow, "ci.yml")
    end
    assert_includes error.message, "reviewed paired comparison job"
  end

  def test_rejects_paired_gate_without_a_measurement_dependency
    workflow = paired_perf_workflow do |jobs|
      jobs.fetch("frontend-perf-gate").delete("needs")
    end

    error = assert_raises(RoadmapEvidenceError) do
      validate_perf_gate_baseline_lifecycle(workflow, "ci.yml")
    end
    assert_includes error.message, "reviewed paired comparison job"
  end

  def test_rejects_a_paired_gate_without_a_measurement_job
    workflow = paired_perf_workflow do |jobs|
      jobs.delete("frontend-perf-measure")
    end

    error = assert_raises(RoadmapEvidenceError) do
      validate_perf_gate_baseline_lifecycle(workflow, "ci.yml")
    end
    assert_includes error.message, "requires frontend-perf-measure"
  end

  def test_rejects_paired_perf_jobs_not_required_by_ci_gate
    workflow = paired_perf_workflow do |jobs|
      jobs.fetch("ci-gate").fetch("needs").delete("frontend-perf-measure")
    end

    error = assert_raises(RoadmapEvidenceError) do
      validate_perf_gate_baseline_lifecycle(workflow, "ci.yml")
    end
    assert_includes error.message, "reviewed fail-closed aggregate job"
  end

  def test_rejects_a_noop_paired_perf_ci_gate
    workflow = paired_perf_workflow do |jobs|
      jobs.fetch("ci-gate")["steps"] = [{ "run" => "true" }]
    end

    error = assert_raises(RoadmapEvidenceError) do
      validate_perf_gate_baseline_lifecycle(workflow, "ci.yml")
    end
    assert_includes error.message, "reviewed fail-closed aggregate job"
  end

  def test_rejects_a_paired_perf_ci_gate_that_can_be_skipped
    workflow = paired_perf_workflow do |jobs|
      jobs.fetch("ci-gate").delete("if")
    end

    error = assert_raises(RoadmapEvidenceError) do
      validate_perf_gate_baseline_lifecycle(workflow, "ci.yml")
    end
    assert_includes error.message, "reviewed fail-closed aggregate job"
  end

  def test_rejects_each_missing_paired_perf_ci_gate_result_check
    PAIRED_PERF_CI_GATE_NEEDS.each do |job_name|
      workflow = paired_perf_workflow do |jobs|
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
    assert_includes stderr, "does not match the reviewed active D-084 performance CI workflow"
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
    workflow = coverage_workflow.sub(
      "#{COVERAGE_STEP_HEADER}\n        run:",
      "#{COVERAGE_STEP_HEADER}\n        continue-on-error: false\n        run:"
    )

    assert coverage_gate_present?(workflow, "ci.yml")
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
                    commands.index("cargo test --workspace -- --include-ignored")
  end
end
