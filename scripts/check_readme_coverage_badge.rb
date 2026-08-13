#!/usr/bin/env ruby
# frozen_string_literal: true

# Validates that the README's static coverage badge matches the
# coverage threshold actually enforced in CI (issue #211).
#
# The README displays a Shields badge:
#   [![test coverage: 100%](https://img.shields.io/badge/test%20coverage-100%25-brightgreen)]
#
# The CI workflow enforces:
#   cargo llvm-cov --workspace --fail-under-lines 100 --fail-under-regions 100
#
# This validator binds the badge's visible percentage to the CI
# threshold so that a false badge (e.g. "101%" or "95%") is rejected
# even when every other check stays green.
#
# Usage: ruby scripts/check_readme_coverage_badge.rb [repository_root]
#
# Exits 0 if the badge matches the enforced threshold, 1 otherwise.

require "pathname"

class CoverageBadgeError < StandardError; end

REPO_ROOT = Pathname(ARGV[0] || Pathname(__dir__).parent)
README_PATH = REPO_ROOT / "README.md"
CI_WORKFLOW_PATH = REPO_ROOT / ".github" / "workflows" / "ci.yml"

# The badge URL pattern in README.md.
# Matches: [![test coverage: N%](https://img.shields.io/badge/test%20coverage-N%25-...)]
BADGE_PATTERN = /
  \[!\[test\s+coverage:\s*(\d+)%\]
  \((https:\/\/img\.shields\.io\/badge\/test%20coverage-(\d+)%25-[a-z]+)\)
\]/x.freeze

# The CI coverage gate pattern.
# Matches: --fail-under-lines N
FAIL_UNDER_LINES_PATTERN = /--fail-under-lines\s+(\d+)/.freeze

def check!
  raise CoverageBadgeError, "README.md not found at #{README_PATH}" unless README_PATH.exist?
  raise CoverageBadgeError, "ci.yml not found at #{CI_WORKFLOW_PATH}" unless CI_WORKFLOW_PATH.exist?

  readme_text = README_PATH.read
  ci_text = CI_WORKFLOW_PATH.read

  # --- Extract the badge percentage from README ---
  badge_match = readme_text.match(BADGE_PATTERN)
  unless badge_match
    raise CoverageBadgeError,
          "README.md must contain a 'test coverage: N%' Shields badge"
  end

  badge_alt_pct = badge_match[1].to_i
  badge_url_pct = badge_match[3].to_i

  # The alt text percentage and the URL percentage must agree.
  if badge_alt_pct != badge_url_pct
    raise CoverageBadgeError,
          "README coverage badge alt text says #{badge_alt_pct}% but " \
          "the URL says #{badge_url_pct}% — they must match"
  end

  # --- Extract the enforced threshold from CI ---
  ci_match = ci_text.match(FAIL_UNDER_LINES_PATTERN)
  unless ci_match
    raise CoverageBadgeError,
          "ci.yml must contain a --fail-under-lines threshold"
  end

  ci_threshold = ci_match[1].to_i

  # --- Bind the badge to the CI threshold ---
  if badge_alt_pct != ci_threshold
    raise CoverageBadgeError,
          "README coverage badge says #{badge_alt_pct}% but CI enforces " \
          "--fail-under-lines #{ci_threshold} — the badge must match the " \
          "enforced threshold"
  end

  # --- Also verify --fail-under-regions matches ---
  regions_match = ci_text.match(/--fail-under-regions\s+(\d+)/)
  if regions_match
    regions_threshold = regions_match[1].to_i
    if regions_threshold != ci_threshold
      raise CoverageBadgeError,
            "CI --fail-under-lines (#{ci_threshold}) and " \
            "--fail-under-regions (#{regions_threshold}) differ — " \
            "the badge binds to a single threshold"
    end
  end

  # --- Verify the badge links to docs/TESTING.md ---
  unless readme_text.match?(/\[!\[test\s+coverage:\s*\d+%\]\([^)]+\)\]\(\.\/docs\/TESTING\.md\)/)
    raise CoverageBadgeError,
          "README coverage badge must link to ./docs/TESTING.md"
  end

  puts "README coverage badge matches CI threshold (#{ci_threshold}%)."
rescue CoverageBadgeError => e
  warn "README coverage badge check failed: #{e.message}"
  exit 1
end

check!
