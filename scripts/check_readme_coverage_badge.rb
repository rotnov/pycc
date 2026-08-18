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
require "psych"

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

# The CI badge pattern in README.md.
# Matches: [![CI](https://github.com/<owner>/<repo>/actions/workflows/ci.yml/badge.svg?branch=<branch>)](https://github.com/<owner>/<repo>/actions/workflows/ci.yml)
CI_BADGE_PATTERN = /
  \[!\[CI\]\(
    (https:\/\/github\.com\/(?<repo>[^\/]+\/[^\/]+)\/actions\/workflows\/(?<workflow>[^\/?]+)\/badge\.svg\?branch=(?<branch>[^)\s]+))
  \)\]\(
    https:\/\/github\.com\/(?<link_repo>[^\/]+\/[^\/]+)\/actions\/workflows\/(?<link_workflow>[^\/?]+)
  \)
/x.freeze

# The coverage step name in ci.yml's build-test-coverage job.
COVERAGE_STEP_NAME = "Hard coverage gate"

def yaml_mapping(node, context)
  raise CoverageBadgeError, "#{context} must be a mapping" unless node.is_a?(Psych::Nodes::Mapping)

  entries = {}
  node.children.each_slice(2) do |key, value|
    unless key.is_a?(Psych::Nodes::Scalar)
      raise CoverageBadgeError, "#{context} contains a non-scalar key"
    end
    entries[key.value] = value
  end
  entries
end

def yaml_scalar(node, context)
  raise CoverageBadgeError, "#{context} must be a scalar" unless node.is_a?(Psych::Nodes::Scalar)
  node.value
end

def check!
  raise CoverageBadgeError, "README.md not found at #{README_PATH}" unless README_PATH.exist?
  raise CoverageBadgeError, "ci.yml not found at #{CI_WORKFLOW_PATH}" unless CI_WORKFLOW_PATH.exist?

  readme_text = README_PATH.read
  ci_text = CI_WORKFLOW_PATH.read

  # --- Extract the badge percentage from README ---
  badge_matches = readme_text.scan(BADGE_PATTERN)
  if badge_matches.empty?
    raise CoverageBadgeError,
          "README.md must contain a 'test coverage: N%' Shields badge"
  end

  if badge_matches.length > 1
    raise CoverageBadgeError,
          "README.md must contain exactly one coverage badge, found #{badge_matches.length}"
  end

  badge_match = badge_matches.first
  badge_alt_pct = badge_match[0].to_i
  badge_url_pct = badge_match[2].to_i

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

  # --- Verify the CI badge ---
  ci_badge_matches = readme_text.scan(CI_BADGE_PATTERN)
  if ci_badge_matches.empty?
    raise CoverageBadgeError,
          "README.md must contain a CI badge linking to the ci.yml workflow"
  end

  if ci_badge_matches.length > 1
    raise CoverageBadgeError,
          "README.md must contain exactly one CI badge, found #{ci_badge_matches.length}"
  end

  ci_badge_match = ci_badge_matches.first
  ci_badge_repo = ci_badge_match[0]
  ci_badge_workflow = ci_badge_match[1]
  ci_badge_branch = ci_badge_match[2]
  ci_badge_link_repo = ci_badge_match[3]
  ci_badge_link_workflow = ci_badge_match[4]

  unless ci_badge_repo == "rotnov/pycc"
    raise CoverageBadgeError,
          "README CI badge must link to rotnov/pycc, found #{ci_badge_repo.inspect}"
  end

  unless ci_badge_workflow == "ci.yml"
    raise CoverageBadgeError,
          "README CI badge must reference actions/workflows/ci.yml, " \
          "found #{ci_badge_workflow.inspect}"
  end

  unless ci_badge_branch == "main"
    raise CoverageBadgeError,
          "README CI badge must use branch=main, found #{ci_badge_branch.inspect}"
  end

  # The clickable link target must match the badge image source.
  unless ci_badge_link_repo == ci_badge_repo
    raise CoverageBadgeError,
          "README CI badge link target repo (#{ci_badge_link_repo.inspect}) " \
          "does not match badge image repo (#{ci_badge_repo.inspect})"
  end

  unless ci_badge_link_workflow == ci_badge_workflow
    raise CoverageBadgeError,
          "README CI badge link target workflow (#{ci_badge_link_workflow.inspect}) " \
          "does not match badge image workflow (#{ci_badge_workflow.inspect})"
  end

  # --- Verify the coverage step cannot be skipped ---
  validate_coverage_step_unconditional(ci_text)

  # --- Verify ci-gate depends on build-test-coverage ---
  validate_ci_gate_needs_build_test_coverage(ci_text)

  puts "README coverage badge matches CI threshold (#{ci_threshold}%)."
rescue CoverageBadgeError => e
  warn "README coverage badge check failed: #{e.message}"
  exit 1
end

# Parses ci.yml and verifies the coverage step in build-test-coverage
# has no `if:` condition that could skip it (issue #211).
def validate_coverage_step_unconditional(ci_text)
  stream = Psych.parse_stream(ci_text, filename: CI_WORKFLOW_PATH.to_s)
  root = yaml_mapping(stream.children.first.root, CI_WORKFLOW_PATH.to_s)
  jobs = yaml_mapping(root["jobs"], "#{CI_WORKFLOW_PATH} jobs")
  coverage_job_node = jobs["build-test-coverage"]
  unless coverage_job_node
    raise CoverageBadgeError,
          "#{CI_WORKFLOW_PATH}: build-test-coverage job not found"
  end
  coverage_job = yaml_mapping(coverage_job_node, "#{CI_WORKFLOW_PATH} build-test-coverage")
  steps = coverage_job["steps"]
  unless steps && steps.is_a?(Psych::Nodes::Sequence)
    raise CoverageBadgeError,
          "#{CI_WORKFLOW_PATH}: build-test-coverage must have a steps sequence"
  end

  steps.children.each do |step_node|
    step = yaml_mapping(step_node, "#{CI_WORKFLOW_PATH} step")
    next unless step["name"] && step["run"]

    step_name = yaml_scalar(step["name"], "#{CI_WORKFLOW_PATH} step name")
    next unless step_name.include?(COVERAGE_STEP_NAME)

    if step.key?("if")
      raise CoverageBadgeError,
            "#{CI_WORKFLOW_PATH}: coverage step must not have an if condition " \
            "that could skip it"
    end

    continue_on_error = step["continue-on-error"]
    if continue_on_error &&
       yaml_scalar(continue_on_error, "#{CI_WORKFLOW_PATH} step continue-on-error").strip != "false"
      raise CoverageBadgeError,
            "#{CI_WORKFLOW_PATH}: coverage step must propagate failures"
    end

    return
  end

  raise CoverageBadgeError,
        "#{CI_WORKFLOW_PATH}: coverage step '#{COVERAGE_STEP_NAME}' not found"
end

# Parses ci.yml and verifies ci-gate's needs list includes build-test-coverage
# (issue #211).
def validate_ci_gate_needs_build_test_coverage(ci_text)
  stream = Psych.parse_stream(ci_text, filename: CI_WORKFLOW_PATH.to_s)
  root = yaml_mapping(stream.children.first.root, CI_WORKFLOW_PATH.to_s)
  jobs = yaml_mapping(root["jobs"], "#{CI_WORKFLOW_PATH} jobs")
  ci_gate_node = jobs["ci-gate"]
  unless ci_gate_node
    raise CoverageBadgeError,
          "#{CI_WORKFLOW_PATH}: ci-gate job not found"
  end
  ci_gate = yaml_mapping(ci_gate_node, "#{CI_WORKFLOW_PATH} ci-gate")
  needs_node = ci_gate["needs"]
  unless needs_node && needs_node.is_a?(Psych::Nodes::Sequence)
    raise CoverageBadgeError,
          "#{CI_WORKFLOW_PATH}: ci-gate must have a needs sequence"
  end
  needs = needs_node.children.map do |need|
    yaml_scalar(need, "#{CI_WORKFLOW_PATH} ci-gate needs entry")
  end
  unless needs.include?("build-test-coverage")
    raise CoverageBadgeError,
          "#{CI_WORKFLOW_PATH}: ci-gate must depend on build-test-coverage"
  end
end

check!
