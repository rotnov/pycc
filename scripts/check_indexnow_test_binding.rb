#!/usr/bin/env ruby
# frozen_string_literal: true

# Validates that .github/workflows/pages.yml effectively executes the
# hermetic IndexNow HTTP test (scripts/test-notify-indexnow.py) on every
# relevant pull request with fail-closed exit propagation.
#
# This binds issue #160: removing or weakening the only real HTTP-fixture
# invocation must cause a CI failure.  Unlike a raw substring check, this
# validator rejects every recorded bypass mutation:
#
#   - deleting or renaming the fixture invocation;
#   - a push-only job- or step-level `if` condition that excludes PRs;
#   - job- or step-level `continue-on-error` that absorbs fixture failure;
#   - shell control flow that absorbs the exit status (`|| true`, `; true`,
#     `&& true`, background `&`, or piping into `true`);
#   - removal of the `pull_request` trigger or its path coverage of the
#     fixture script.
#
# Usage: ruby scripts/check_indexnow_test_binding.rb [repository_root]
#
# Exits 0 if all invariants hold, 1 otherwise.

require "pathname"
require "yaml"

class IndexNowBindingError < StandardError; end

REPO_ROOT = Pathname(ARGV[0] || Pathname(__dir__).parent)
WORKFLOW_PATH = REPO_ROOT / ".github" / "workflows" / "pages.yml"
FIXTURE_SCRIPT = "scripts/test-notify-indexnow.py"

# Shell tokens that, when they appear on the same logical line as the
# fixture invocation, absorb or mask its non-zero exit status.
EXIT_ABSORBING_SUFFIXES = [
  /\|\|\s*true\b/,
  /;\s*true\b/,
  /&&\s*true\b/,
  /\|\s*true\b/,
  /&&\s*:/,
  /\|\|\s*:/,
  /\s&\s*$/,
  /\s&&\s*$/,
].freeze

# An `if:` condition that restricts execution to push events only,
# excluding pull requests from the validation path.
PUSH_ONLY_IF_PATTERN =
  /\Agithub\.event_name\s*==\s*(['"])push\1/.freeze

def check!
  raise IndexNowBindingError, "pages.yml not found at #{WORKFLOW_PATH}" unless WORKFLOW_PATH.exist?

  text = WORKFLOW_PATH.read
  workflow = YAML.safe_load(text, aliases: true)
  raise IndexNowBindingError, "pages.yml must be a YAML mapping" unless workflow.is_a?(Hash)

  # --- 1. pull_request trigger must cover the fixture script ---
  # YAML 1.1 parses the bare key `on:` as the boolean `true`.
  triggers = workflow["on"] || workflow[true]
  raise IndexNowBindingError, "pages.yml must define triggers (on:)" unless triggers

  pr_paths = pull_request_paths(triggers)
  unless pr_paths
    raise IndexNowBindingError,
          "pages.yml must have a pull_request trigger so the fixture runs on PRs"
  end

  unless pr_paths.include?(FIXTURE_SCRIPT)
    raise IndexNowBindingError,
          "pull_request path filter must cover #{FIXTURE_SCRIPT} " \
          "(issue #160)"
  end

  # --- 2. build job must exist and be PR-reachable ---
  jobs = workflow["jobs"]
  raise IndexNowBindingError, "pages.yml must define jobs" unless jobs

  build_job = jobs["build"]
  raise IndexNowBindingError, "pages.yml must have a build job" unless build_job

  check_pr_reachable(build_job, "build job")

  # --- 3. a build step must effectively execute the fixture ---
  steps = build_job["steps"]
  raise IndexNowBindingError, "build job must have steps" unless steps

  fixture_step = find_fixture_step(steps)
  unless fixture_step
    raise IndexNowBindingError,
          "build job must have a step whose run command invokes " \
          "#{FIXTURE_SCRIPT} (issue #160)"
  end

  check_pr_reachable(fixture_step, "fixture step")

  # --- 4. the fixture invocation must not absorb its exit status ---
  run_text = fixture_step["run"].to_s
  check_exit_propagation(run_text)

  puts "IndexNow test binding invariants passed."
rescue IndexNowBindingError => e
  warn "IndexNow test binding check failed: #{e.message}"
  exit 1
rescue Psych::SyntaxError => e
  warn "IndexNow test binding check failed: pages.yml is not valid YAML: #{e.message}"
  exit 1
end

def pull_request_paths(triggers)
  case triggers
  when String
    return nil unless triggers == "pull_request"
    []
  when Array
    return nil unless triggers.include?("pull_request")
    []
  when Hash
    pr = triggers["pull_request"]
    return nil unless pr
    paths = pr.is_a?(Hash) ? pr["paths"] : nil
    paths || []
  else
    nil
  end
end

def check_pr_reachable(node, label)
  if_cond = node["if"]
  if if_cond && PUSH_ONLY_IF_PATTERN.match?(if_cond.to_s)
    raise IndexNowBindingError,
          "#{label} has a push-only if condition (#{if_cond.inspect}) " \
          "that excludes pull requests (issue #160)"
  end

  if node["continue-on-error"] == true
    raise IndexNowBindingError,
          "#{label} has continue-on-error: true which absorbs fixture " \
          "failure (issue #160)"
  end
end

def find_fixture_step(steps)
  steps.find do |step|
    run = step["run"]
    next false unless run.is_a?(String)
    # The run block may contain multiple lines; check each logical line.
    run.each_line.any? { |line| line.include?(FIXTURE_SCRIPT) }
  end
end

def check_exit_propagation(run_text)
  run_text.each_line do |line|
    next unless line.include?(FIXTURE_SCRIPT)
    stripped = line.strip
    # Skip comment-only lines.
    next if stripped.start_with?("#")

    EXIT_ABSORBING_SUFFIXES.each do |pattern|
      if pattern.match?(stripped)
        raise IndexNowBindingError,
              "fixture invocation line absorbs exit status " \
              "(#{stripped.inspect}) — failure would be masked " \
              "(issue #160)"
      end
    end
  end
end

check!
