#!/usr/bin/env ruby
# frozen_string_literal: true

# Test suite for scripts/check_readme_coverage_badge.rb — validates
# that the README coverage badge is bound to the CI-enforced threshold
# (issue #211).

require "minitest/autorun"
require "pathname"
require "rbconfig"
require "tempfile"

REPO_ROOT = Pathname(__dir__).parent
CHECKER = REPO_ROOT / "scripts" / "check_readme_coverage_badge.rb"
README = REPO_ROOT / "README.md"
CI_WORKFLOW = REPO_ROOT / ".github" / "workflows" / "ci.yml"

class TestCheckReadmeCoverageBadge < Minitest::Test
  def run_checker(readme_text, ci_text)
    Dir.mktmpdir do |dir|
      root = Pathname(dir)
      (root / ".github" / "workflows").mkpath
      (root / "README.md").write(readme_text)
      (root / ".github" / "workflows" / "ci.yml").write(ci_text)
      output = `#{RbConfig.ruby} #{CHECKER} #{root} 2>&1`
      return $?.exitstatus, output
    end
  end

  def live_readme
    README.read
  end

  def live_ci
    CI_WORKFLOW.read
  end

  # --- Positive: the live README and CI pass ---

  def test_live_readme_and_ci_pass
    status, output = run_checker(live_readme, live_ci)
    assert_equal 0, status, "live README + CI failed:\n#{output}"
  end

  def test_live_readme_has_coverage_badge
    assert_match(/test coverage:\s*\d+%/, live_readme,
                 "README must have a coverage badge")
  end

  def test_live_ci_has_fail_under_lines
    assert_match(/--fail-under-lines\s+\d+/, live_ci,
                 "ci.yml must have a --fail-under-lines threshold")
  end

  # --- Negative: badge percentage mismatches ---

  def test_rejects_badge_with_wrong_percentage
    text = live_readme.sub(
      /test%20coverage-100%25-brightgreen/,
      "test%20coverage-99%25-brightgreen"
    ).sub(/test coverage: 100%/, "test coverage: 99%")
    status, = run_checker(text, live_ci)
    refute_equal 0, status, "accepted badge with 99% when CI enforces 100%"
  end

  def test_rejects_badge_with_inflated_percentage
    text = live_readme.sub(
      /test%20coverage-100%25-brightgreen/,
      "test%20coverage-101%25-brightgreen"
    ).sub(/test coverage: 100%/, "test coverage: 101%")
    status, = run_checker(text, live_ci)
    refute_equal 0, status, "accepted badge with 101% when CI enforces 100%"
  end

  def test_rejects_alt_text_and_url_disagreeing
    text = live_readme.sub(
      /test%20coverage-100%25-brightgreen/,
      "test%20coverage-95%25-brightgreen"
    )
    # Alt text still says 100%, URL says 95%
    status, = run_checker(text, live_ci)
    refute_equal 0, status, "accepted badge where alt text and URL disagree"
  end

  # --- Negative: CI threshold mismatch ---

  def test_rejects_ci_threshold_different_from_badge
    text = live_ci.sub(
      /--fail-under-lines 100/,
      "--fail-under-lines 95"
    )
    status, = run_checker(live_readme, text)
    refute_equal 0, status, "accepted CI threshold 95 when badge says 100%"
  end

  # --- Negative: missing badge ---

  def test_rejects_missing_coverage_badge
    text = live_readme.sub(
      /\[!\[test coverage: \d+%\]\([^)]+\)\]\(\.\/docs\/TESTING\.md\)\n/,
      ""
    )
    status, = run_checker(text, live_ci)
    refute_equal 0, status, "accepted README without coverage badge"
  end

  # --- Negative: badge not linking to TESTING.md ---

  def test_rejects_badge_not_linking_to_testing_md
    text = live_readme.sub(
      /\]\(\.\/docs\/TESTING\.md\)/,
      "](./docs/TESTING.md)"
    )
    # This sub is a no-op; do a real mutation:
    text = live_readme.sub(
      /\[!\[test coverage: \d+%\]\([^)]+\)\]\(\.\/docs\/TESTING\.md\)/,
      "[![test coverage: 100%](https://img.shields.io/badge/test%20coverage-100%25-brightgreen)](./docs/ROADMAP.md)"
    )
    status, = run_checker(text, live_ci)
    refute_equal 0, status, "accepted badge linking to ROADMAP.md instead of TESTING.md"
  end

  # --- Negative: missing CI threshold ---

  def test_rejects_missing_ci_threshold
    text = live_ci.sub(
      /--fail-under-lines 100/,
      "--fail-under-lines"
    )
    status, = run_checker(live_readme, text)
    refute_equal 0, status, "accepted ci.yml without --fail-under-lines value"
  end

  # --- Negative: lines and regions thresholds disagree ---

  def test_rejects_lines_and_regions_thresholds_disagreeing
    text = live_ci.sub(
      /--fail-under-regions 100/,
      "--fail-under-regions 95"
    )
    status, = run_checker(live_readme, text)
    refute_equal 0, status, "accepted CI with lines=100 but regions=95"
  end

  # --- Negative: CI badge mutations (issue #211) ---

  def test_rejects_ci_badge_with_wrong_branch
    text = live_readme.sub(
      /badge\.svg\?branch=main\)\]\(/,
      "badge.svg?branch=dev)]("
    )
    status, = run_checker(text, live_ci)
    refute_equal 0, status, "accepted CI badge with branch=dev instead of branch=main"
  end

  def test_rejects_ci_badge_with_wrong_workflow
    text = live_readme.sub(
      %r{actions/workflows/ci\.yml/badge\.svg},
      "actions/workflows/pages.yml/badge.svg"
    ).sub(
      %r{actions/workflows/ci\.yml\)},
      "actions/workflows/pages.yml)"
    )
    status, = run_checker(text, live_ci)
    refute_equal 0, status, "accepted CI badge referencing pages.yml instead of ci.yml"
  end

  def test_rejects_ci_badge_with_wrong_repository
    text = live_readme.sub(
      %r{github\.com/rotnov/pycc/actions},
      "github.com/wrong/repo/actions"
    )
    status, = run_checker(text, live_ci)
    refute_equal 0, status, "accepted CI badge linking to wrong repository"
  end

  def test_rejects_missing_ci_badge
    text = live_readme.sub(
      /\[!\[CI\]\([^)]+\)\]\([^)]+\)\n/,
      ""
    )
    status, = run_checker(text, live_ci)
    refute_equal 0, status, "accepted README without CI badge"
  end

  # --- Negative: duplicate coverage badge (issue #211) ---

  def test_rejects_duplicate_coverage_badge
    text = live_readme.sub(
      /\[!\[test coverage: \d+%\]\([^)]+\)\]\(\.\/docs\/TESTING\.md\)\n/,
      "[![test coverage: 100%](https://img.shields.io/badge/test%20coverage-100%25-brightgreen)](./docs/TESTING.md)\n" \
      "[![test coverage: 100%](https://img.shields.io/badge/test%20coverage-100%25-brightgreen)](./docs/TESTING.md)\n"
    )
    status, = run_checker(text, live_ci)
    refute_equal 0, status, "accepted README with duplicate coverage badge"
  end

  # --- Negative: coverage step can be skipped (issue #211) ---

  def test_rejects_coverage_step_with_if_condition
    text = live_ci.sub(
      /(      - name: Hard coverage gate[^\n]*\n)/,
      "\\1        if: failure()\n"
    )
    status, = run_checker(live_readme, text)
    refute_equal 0, status, "accepted coverage step with if: failure() that could skip it"
  end

  # --- Negative: ci-gate does not depend on build-test-coverage (issue #211) ---

  def test_rejects_ci_gate_without_build_test_coverage_dependency
    text = live_ci.sub(
      /      - build-test-coverage\n/,
      ""
    )
    status, = run_checker(live_readme, text)
    refute_equal 0, status, "accepted ci-gate without build-test-coverage dependency"
  end

  # --- Positive: validator is present in ci.yml (issue #211) ---

  def test_live_ci_contains_coverage_badge_validator
    assert_match(/check_readme_coverage_badge\.rb/, live_ci,
                 "ci.yml must run the README coverage badge validator")
  end

  def test_live_ci_contains_coverage_badge_validator_tests
    assert_match(/test_check_readme_coverage_badge\.rb/, live_ci,
                 "ci.yml must run the README coverage badge validator tests")
  end

  # --- Negative: coverage step with continue-on-error (issue #211) ---

  def test_rejects_coverage_step_with_continue_on_error
    text = live_ci.sub(
      /(      - name: Hard coverage gate[^\n]*\n)/,
      "\\1        continue-on-error: true\n"
    )
    status, = run_checker(live_readme, text)
    refute_equal 0, status, "accepted coverage step with continue-on-error: true"
  end

  # --- Negative: CI badge link target mismatch (issue #211) ---

  def test_rejects_ci_badge_link_target_wrong_repo
    text = live_readme.sub(
      %r{\)\]\(https://github\.com/rotnov/pycc/actions/workflows/ci\.yml\)},
      ")](https://github.com/evil/repo/actions/workflows/ci.yml)"
    )
    status, = run_checker(text, live_ci)
    refute_equal 0, status, "accepted CI badge with link target pointing to wrong repo"
  end

  def test_rejects_ci_badge_link_target_wrong_workflow
    text = live_readme.sub(
      %r{\)\]\(https://github\.com/rotnov/pycc/actions/workflows/ci\.yml\)},
      ")](https://github.com/rotnov/pycc/actions/workflows/pages.yml)"
    )
    status, = run_checker(text, live_ci)
    refute_equal 0, status, "accepted CI badge with link target pointing to wrong workflow"
  end

  # --- Negative: duplicate CI badge (issue #211) ---

  def test_rejects_duplicate_ci_badge
    ci_badge_line = live_readme.match(/\[!\[CI\]\([^)]+\)\]\([^)]+\)\n/).to_s
    text = live_readme.sub(
      /\[!\[CI\]\([^)]+\)\]\([^)]+\)\n/,
      ci_badge_line + "[![CI](https://github.com/wrong/repo/actions/workflows/ci.yml/badge.svg?branch=dev)](https://github.com/wrong/repo/actions/workflows/ci.yml)\n"
    )
    status, = run_checker(text, live_ci)
    refute_equal 0, status, "accepted README with duplicate CI badge"
  end
end
