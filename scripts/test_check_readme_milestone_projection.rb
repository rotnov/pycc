#!/usr/bin/env ruby
# frozen_string_literal: true

# Test suite for scripts/check_readme_milestone_projection.rb — mutation
# tests that verify the validator catches a README Roadmap projection that
# drifts from docs/ROADMAP.md's commit-relative milestone status
# (issue #215).
#
# Production-path mutation directions covered:
#   - completed v0.1 rendered unchecked (understates a met milestone)
#   - an incomplete/in-progress milestone rendered checked/currently
#     shipped (overstates an unmet milestone)
#   - pre-alpha boundary dropped or reinterpreted as production readiness
#   - Tier-1 boundary silently downgraded to "Linux/macOS"
#   - missing docs/ROADMAP.md link
#   - an accepted v0.1 evidence identifier not reflected in the projection

require "minitest/autorun"
require "pathname"
require "rbconfig"
require "tmpdir"

REPO_ROOT = Pathname(__dir__).parent
CHECKER = REPO_ROOT / "scripts" / "check_readme_milestone_projection.rb"
README_PATH = REPO_ROOT / "README.md"
ROADMAP_PATH = REPO_ROOT / "docs" / "ROADMAP.md"

class TestCheckReadmeMilestoneProjection < Minitest::Test
  # Run the checker against a synthetic repository root containing the
  # given README and ROADMAP text.
  def run_checker(readme_text, roadmap_text)
    Dir.mktmpdir do |dir|
      root = Pathname(dir)
      (root / "docs").mkpath
      (root / "README.md").write(readme_text)
      (root / "docs" / "ROADMAP.md").write(roadmap_text)
      output = `#{RbConfig.ruby} #{CHECKER} #{root} 2>&1`
      return $?.exitstatus, output
    end
  end

  def read_live_readme
    README_PATH.read
  end

  def read_live_roadmap
    ROADMAP_PATH.read
  end

  # A valid ROADMAP mirroring the live document's milestone state.
  def valid_roadmap
    <<~MARKDOWN
      # pycc Roadmap

      Milestone = shippable + demo-able.

      ## Current delivery status

      Last reviewed on 2026-08-07.

      **Current milestone: v0.2 — acceptance criteria met; v0.3 in progress.** All five v0.1 acceptance-checklist bullets below are green.

      ### v0.1 acceptance checklist

      - [x] `fib` and `mandelbrot-ascii` compile and match CPython output on all five Tier-1 targets. <!-- roadmap-evidence: conformance-fib-mandelbrot-tier1 -->
      - [x] `pycc check` processes 1k LOC in under 75 ms. <!-- roadmap-evidence: check-throughput-1k-loc-75ms -->
      - [x] The error demonstration matches the stable CLI specification output. <!-- roadmap-evidence: cli-spec-diagnostic-match -->
      - [x] The five-target native CI matrix and one cross-host compilation path are live on `main`. <!-- roadmap-evidence: ci-tier1-cross-compile -->
      - [x] The 100% line and region coverage gate is required and green for the current slice. <!-- roadmap-evidence: ci-build-test-coverage-100 -->
      - [x] The README coverage badge percentage is bound to ci.yml's enforced thresholds. <!-- roadmap-evidence: readme-coverage-badge-bound -->

      ## v0.3 — classes & pattern matching

      Classes, inheritance, dataclasses, enums, protocols, `match`.

      **Accept:** conformance rows green.
    MARKDOWN
  end

  # A valid README projection mirroring the live document.
  def valid_readme_projection
    <<~MARKDOWN
      # pycc

      ## Roadmap

      **Current status (pre-alpha):** v0.1 and v0.2 acceptance criteria are both
      met. For v0.1: `fib` and `mandelbrot-ascii` match pinned CPython output on
      all five Tier-1 targets (Linux x64/arm64, macOS x64/arm64, Windows x64),
      `pycc check` clears its <75ms/1000 LOC throughput floor, diagnostic output
      matches the CLI specification, the five-target native CI matrix and one
      cross-host compilation path are live, the 100% line/region coverage gate is
      required and green, and the README coverage badge is bound to the enforced
      CI coverage thresholds. v0.2's container/generics corpus and `--release`
      speedup floor are likewise met on all five Tier-1 targets. v0.3 (classes,
      dataclasses, protocols, pattern matching, exceptions) is the current
      delivery milestone, in progress. Later capabilities remain planned. The
      compiler is still pre-alpha: documented gaps are roadmap work, not
      production readiness.

      The complete, commit-relative milestone status lives in
      [`docs/ROADMAP.md`](./docs/ROADMAP.md), the sole milestone status owner.
    MARKDOWN
  end

  # --- Positive: the live README and ROADMAP pass ---

  def test_live_readme_and_roadmap_pass
    status, output = run_checker(read_live_readme, read_live_roadmap)
    assert_equal 0, status, "live README + ROADMAP failed:\n#{output}"
  end

  # --- Positive: a valid synthetic projection passes ---

  def test_valid_projection_passes
    status, output = run_checker(valid_readme_projection, valid_roadmap)
    assert_equal 0, status, "valid projection failed:\n#{output}"
  end

  # --- Negative: missing Roadmap section ---

  def test_rejects_missing_roadmap_section
    readme = "# pycc\n\nNo roadmap here.\n"
    status, = run_checker(readme, valid_roadmap)
    refute_equal 0, status, "accepted README without a Roadmap section"
  end

  # --- Negative: missing README ---

  def test_rejects_missing_readme
    Dir.mktmpdir do |dir|
      (Pathname(dir) / "docs").mkpath
      (Pathname(dir) / "docs" / "ROADMAP.md").write(valid_roadmap)
      output = `#{RbConfig.ruby} #{CHECKER} #{dir} 2>&1`
      assert $?.exitstatus != 0, "accepted missing README.md"
    end
  end

  # --- Negative: missing ROADMAP ---

  def test_rejects_missing_roadmap
    Dir.mktmpdir do |dir|
      (Pathname(dir) / "README.md").write(valid_readme_projection)
      output = `#{RbConfig.ruby} #{CHECKER} #{dir} 2>&1`
      assert $?.exitstatus != 0, "accepted missing docs/ROADMAP.md"
    end
  end

  # --- Mutation: completed v0.1 rendered unchecked ---

  def test_rejects_completed_v0_1_rendered_unchecked
    readme = valid_readme_projection.sub(
      /v0\.1 and v0\.2 acceptance criteria are both\nmet\./,
      "v0.1 and v0.2 acceptance criteria are both\nmet.\n- [ ] **MVP:** v0.1 surface → Linux/macOS binary\n"
    )
    status, output = run_checker(readme, valid_roadmap)
    refute_equal 0, status, "accepted an unchecked MVP/v0.1 item while v0.1 is complete"
    assert_match(/unchecked MVP\/v0\.1/i, output)
  end

  # --- Mutation: completed v0.1 rendered as not met ---

  def test_rejects_v0_1_stated_as_not_met
    readme = valid_readme_projection.sub(
      /v0\.1 and v0\.2 acceptance criteria are both\nmet\./,
      "v0.1 acceptance criteria are not yet met; v0.2 is the next milestone."
    )
    status, output = run_checker(readme, valid_roadmap)
    refute_equal 0, status, "accepted v0.1 stated as not met"
    assert_match(/v0\.1.*not.*met/i, output)
  end

  # --- Mutation: incomplete/in-progress milestone rendered as shipped ---

  def test_rejects_in_progress_milestone_rendered_as_shipped
    readme = valid_readme_projection.sub(
      /v0\.3 \(classes,\ndataclasses, protocols, pattern matching, exceptions\) is the current\ndelivery milestone, in progress\./,
      "v0.3 (classes, dataclasses, protocols, pattern matching, exceptions) is the current delivery milestone, complete and shipped."
    )
    status, output = run_checker(readme, valid_roadmap)
    refute_equal 0, status, "accepted the in-progress milestone rendered as complete/shipped"
    assert_match(/v0\.3.*complete\/shipped.*in progress/i, output)
  end

  # --- Mutation: pre-alpha boundary dropped ---

  def test_rejects_dropped_pre_alpha_boundary
    readme = valid_readme_projection.gsub(/pre-alpha/i, "alpha-stage")
    status, output = run_checker(readme, valid_roadmap)
    refute_equal 0, status, "accepted projection without the pre-alpha boundary"
    assert_match(/pre-alpha/i, output)
  end

  # --- Mutation: checked milestone reinterpreted as production readiness ---

  def test_rejects_production_ready_claim
    readme = valid_readme_projection.sub(
      /not\nproduction readiness\./,
      "now production ready."
    )
    status, output = run_checker(readme, valid_roadmap)
    refute_equal 0, status, "accepted a production-ready claim in the projection"
    assert_match(/production readiness/i, output)
  end

  # --- Mutation: Tier-1 boundary downgraded to Linux/macOS ---

  def test_rejects_linux_macos_only_boundary
    readme = valid_readme_projection.sub(
      /all five Tier-1 targets \(Linux x64\/arm64, macOS x64\/arm64, Windows x64\)/,
      "a Linux/macOS binary"
    )
    status, output = run_checker(readme, valid_roadmap)
    refute_equal 0, status, "accepted 'Linux/macOS binary' Tier-1 boundary"
    assert_match(/Linux\/macOS/i, output)
  end

  # --- Mutation: Windows dropped from Tier-1 boundary ---

  def test_rejects_tier1_boundary_missing_windows
    readme = valid_readme_projection.sub(
      /Linux x64\/arm64, macOS x64\/arm64, Windows x64/,
      "Linux x64/arm64 and macOS x64/arm64"
    )
    status, output = run_checker(readme, valid_roadmap)
    refute_equal 0, status, "accepted Tier-1 boundary without Windows"
    assert_match(/missing.*Windows/i, output)
  end

  # --- Mutation: missing docs/ROADMAP.md link ---

  def test_rejects_missing_roadmap_link
    readme = valid_readme_projection.sub(
      /\[`docs\/ROADMAP\.md`\]\(\.\/docs\/ROADMAP\.md\)/,
      "the roadmap"
    )
    status, output = run_checker(readme, valid_roadmap)
    refute_equal 0, status, "accepted projection without a docs/ROADMAP.md link"
    assert_match(/link to docs\/ROADMAP\.md/i, output)
  end

  # --- Mutation: an accepted v0.1 evidence identifier not reflected ---

  def test_rejects_missing_fib_mandelbrot_evidence
    readme = valid_readme_projection.sub(/`fib` and `mandelbrot-ascii` match pinned CPython output on\n/, "")
    status, output = run_checker(readme, valid_roadmap)
    refute_equal 0, status, "accepted projection missing the fib/mandelbrot evidence"
    assert_match(/conformance-fib-mandelbrot-tier1/i, output)
  end

  def test_rejects_missing_throughput_evidence
    readme = valid_readme_projection.sub(/`pycc check` clears its <75ms\/1000 LOC throughput floor, /, "")
    status, output = run_checker(readme, valid_roadmap)
    refute_equal 0, status, "accepted projection missing the throughput evidence"
    assert_match(/check-throughput-1k-loc-75ms/i, output)
  end

  def test_rejects_missing_coverage_badge_evidence
    readme = valid_readme_projection.sub(/, and the README coverage badge is bound to the enforced\nCI coverage thresholds\./, ".")
    status, output = run_checker(readme, valid_roadmap)
    refute_equal 0, status, "accepted projection missing the coverage-badge evidence"
    assert_match(/readme-coverage-badge-bound/i, output)
  end

  # --- Mutation: a new accepted v0.1 evidence identifier with no binding ---

  def test_rejects_new_evidence_id_without_binding
    roadmap = valid_roadmap.sub(
      /(?<=readme-coverage-badge-bound -->\n)/,
      "\n- [x] A new accepted evidence item. <!-- roadmap-evidence: new-unbound-evidence -->\n"
    )
    status, output = run_checker(valid_readme_projection, roadmap)
    refute_equal 0, status, "accepted a new evidence identifier with no README binding"
    assert_match(/new-unbound-evidence.*no.*binding/i, output)
  end

  # --- Mutation: ROADMAP marks a v0.1 evidence item unchecked ---

  def test_rejects_roadmap_with_unchecked_v0_1_evidence
    roadmap = valid_roadmap.sub(
      /- \[x\] `fib` and `mandelbrot-ascii`/,
      "- [ ] `fib` and `mandelbrot-ascii`"
    )
    status, output = run_checker(valid_readme_projection, roadmap)
    refute_equal 0, status, "accepted roadmap with an unchecked v0.1 evidence item"
    assert_match(/unchecked.*v0\.1 is not complete/i, output)
  end

  # --- Mutation: ROADMAP missing the current-milestone statement ---

  def test_rejects_roadmap_without_current_milestone
    roadmap = valid_roadmap.sub(/\*\*Current milestone:.*?\*\*/m, "")
    status, output = run_checker(valid_readme_projection, roadmap)
    refute_equal 0, status, "accepted roadmap without a current-milestone statement"
    assert_match(/current milestone/i, output)
  end

  # --- Mutation: ROADMAP with no in-progress milestone ---

  def test_rejects_roadmap_without_in_progress_milestone
    roadmap = valid_roadmap.sub(/; v0\.3 in progress\./, ".")
    status, output = run_checker(valid_readme_projection, roadmap)
    refute_equal 0, status, "accepted roadmap with no in-progress milestone"
    assert_match(/in progress/i, output)
  end

  # --- Positive: validator is present in pages.yml ---

  def test_live_pages_yml_contains_validator
    pages = (REPO_ROOT / ".github" / "workflows" / "pages.yml").read
    assert_match(/check_readme_milestone_projection\.rb/, pages,
                 "pages.yml must run the README milestone projection validator")
  end

  def test_live_pages_yml_contains_validator_tests
    pages = (REPO_ROOT / ".github" / "workflows" / "pages.yml").read
    assert_match(/test_check_readme_milestone_projection\.rb/, pages,
                 "pages.yml must run the README milestone projection validator tests")
  end
end
