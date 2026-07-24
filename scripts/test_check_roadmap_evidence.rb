#!/usr/bin/env ruby
# frozen_string_literal: true

require "fileutils"
require "minitest/autorun"
require "open3"
require "pathname"
require "psych"
require "rbconfig"
require "tmpdir"

class RoadmapEvidenceCliTest < Minitest::Test
  CHECKER = Pathname(__dir__) / "check_roadmap_evidence.rb"

  def coverage_workflow(command = "cargo llvm-cov --workspace --fail-under-lines 100 --fail-under-regions 100")
    <<~YAML
      on:
        pull_request:
      jobs:
        build-test-coverage:
          steps:
            - name: Hard coverage gate — 100% lines + regions (D-014)
              run: #{command}
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

  def test_rejects_false_completed_item_without_evidence
    roadmap = <<~MARKDOWN
      ### v0.1 acceptance checklist

      - [x] `pycc check` processes 1k LOC in under 50 ms.
    MARKDOWN

    _stdout, stderr, status = run_checker(roadmap: roadmap, workflow: "jobs: {}\n")

    refute status.success?
    assert_includes stderr, "checked roadmap item is missing an evidence marker"
  end

  def test_rejects_evidence_marker_attached_to_the_wrong_claim
    roadmap = <<~MARKDOWN
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

  def test_rejects_an_unknown_evidence_marker
    roadmap = <<~MARKDOWN
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

  def test_rejects_coverage_evidence_when_the_threshold_is_lowered
    roadmap = <<~MARKDOWN
      ### v0.1 acceptance checklist

      - [x] The 100% line and region coverage gate is required and green for the current slice. <!-- roadmap-evidence: ci-build-test-coverage-100 -->
    MARKDOWN

    _stdout, stderr, status = run_checker(
      roadmap: roadmap,
      workflow: coverage_workflow(
        "cargo llvm-cov --workspace --fail-under-lines 99 --fail-under-regions 100"
      )
    )

    refute status.success?
    assert_includes stderr, "does not provide the exact 100% line and region gate"
  end

  def test_rejects_a_coverage_step_that_can_be_skipped
    roadmap = <<~MARKDOWN
      ### v0.1 acceptance checklist

      - [x] The 100% line and region coverage gate is required and green for the current slice. <!-- roadmap-evidence: ci-build-test-coverage-100 -->
    MARKDOWN
    workflow = coverage_workflow.sub(
      "        run:",
      "        if: false\n        run:"
    )

    _stdout, stderr, status = run_checker(roadmap: roadmap, workflow: workflow)

    refute status.success?
    assert_includes stderr, "coverage evidence must run unconditionally"
  end

  def test_accepts_explicit_continue_on_error_false
    roadmap = <<~MARKDOWN
      ### v0.1 acceptance checklist

      - [x] The 100% line and region coverage gate is required and green for the current slice. <!-- roadmap-evidence: ci-build-test-coverage-100 -->
    MARKDOWN
    workflow = coverage_workflow.sub(
      "        run:",
      "        continue-on-error: false\n        run:"
    )

    stdout, stderr, status = run_checker(roadmap: roadmap, workflow: workflow)

    assert status.success?, stderr
    assert_includes stdout, "Roadmap evidence policy passed."
  end

  def test_rejects_coverage_evidence_not_scheduled_for_pull_requests
    roadmap = <<~MARKDOWN
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
    workflow = Psych.safe_load(
      (repository_root / ".github/workflows/ci.yml").read,
      [],
      [],
      false
    )
    run_blocks = workflow
                 .fetch("jobs")
                 .fetch("build-test-coverage")
                 .fetch("steps")
                 .map { |step| step["run"] }
                 .compact
    commands = run_blocks.flat_map { |run| run.lines.map(&:strip) }

    assert_includes commands, "ruby scripts/test_check_roadmap_evidence.rb"
    assert_includes commands, "ruby scripts/check_roadmap_evidence.rb"
  end
end
