#!/usr/bin/env ruby
# frozen_string_literal: true

# Validates structural invariants of .github/workflows/link-check.yml
# (issue #202).
#
# The scheduled live link audit must:
#   - have schedule and workflow_dispatch triggers, NOT pull_request
#   - have read-only permissions (contents: read only — no write, no
#     OIDC, no secrets)
#   - invoke the hermetic test for the live checker
#   - invoke the live checker script
#   - use continue-on-error so a failure is a warning, not a blocker
#   - have an if: always() outcome observer
#
# This structural test ensures that removing the live-audit invocation
# from its declared workflow causes a CI failure (issue #202 validation
# expectation).
#
# Usage: ruby scripts/check_link_check_workflow.rb [repository_root]
#
# Exits 0 if all invariants hold, 1 otherwise.

require "pathname"
require "yaml"

class LinkCheckWorkflowError < StandardError; end

REPO_ROOT = Pathname(ARGV[0] || Pathname(__dir__).parent)
WORKFLOW_PATH = REPO_ROOT / ".github" / "workflows" / "link-check.yml"

def check!
  raise LinkCheckWorkflowError, "link-check.yml not found at #{WORKFLOW_PATH}" unless WORKFLOW_PATH.exist?

  text = WORKFLOW_PATH.read
  workflow = YAML.safe_load(text, aliases: true)

  # --- Triggers ---
  # In YAML 1.1, the bare key "on" is parsed as boolean true, so we
  # need to check both "on" and true keys.
  triggers = workflow["on"] || workflow[true]
  if triggers.nil?
    raise LinkCheckWorkflowError, "link-check.yml must define triggers (on:)"
  end

  # Normalize: "on" can be a string (single trigger) or a hash
  if triggers.is_a?(String)
    trigger_keys = [triggers]
  elsif triggers.is_a?(Hash)
    trigger_keys = triggers.keys.map(&:to_s)
  else
    raise LinkCheckWorkflowError, "link-check.yml has unexpected trigger format"
  end

  unless trigger_keys.include?("schedule")
    raise LinkCheckWorkflowError,
          "link-check.yml must have a 'schedule' trigger"
  end
  unless trigger_keys.include?("workflow_dispatch")
    raise LinkCheckWorkflowError,
          "link-check.yml must have a 'workflow_dispatch' trigger"
  end
  if trigger_keys.include?("pull_request") || trigger_keys.include?("pull_request_target")
    raise LinkCheckWorkflowError,
          "link-check.yml must NOT have a pull_request or pull_request_target " \
          "trigger — the live audit must not execute PR-controlled code"
  end

  # --- Permissions ---
  perms = workflow["permissions"]
  if perms.nil?
    raise LinkCheckWorkflowError,
          "link-check.yml must declare permissions at the workflow level"
  end
  # Must be contents: read only — no write, no OIDC, no secrets
  unless perms.is_a?(Hash)
    raise LinkCheckWorkflowError,
          "link-check.yml permissions must be a hash, got #{perms.inspect}"
  end
  unless perms == { "contents" => "read" }
    raise LinkCheckWorkflowError,
          "link-check.yml must have permissions: contents: read only, " \
          "got #{perms.inspect}"
  end
  # Explicitly reject write scopes, OIDC, or secrets
  forbidden_keys = %w[packages write id-token secrets]
  forbidden_found = perms.keys.select { |k| forbidden_keys.include?(k.to_s) }
  unless forbidden_found.empty?
    raise LinkCheckWorkflowError,
          "link-check.yml must not grant #{forbidden_found.join(', ')} — " \
          "the live audit must not receive write/OIDC/secret permissions"
  end

  # --- Jobs ---
  jobs = workflow["jobs"]
  raise LinkCheckWorkflowError, "link-check.yml must define jobs" unless jobs

  check_job = jobs["live-link-check"]
  unless check_job
    raise LinkCheckWorkflowError,
          "link-check.yml must have a 'live-link-check' job"
  end

  # Job must NOT have write permissions or secrets
  job_perms = check_job["permissions"]
  if job_perms && job_perms != { "contents" => "read" } && job_perms != "contents: read"
    raise LinkCheckWorkflowError,
          "live-link-check job must not have elevated permissions: #{job_perms.inspect}"
  end

  steps = check_job["steps"]
  raise LinkCheckWorkflowError, "live-link-check must have steps" unless steps

  # Must check out with persist-credentials: false
  checkout_step = steps.find { |s| s["uses"]&.include?("actions/checkout") }
  unless checkout_step
    raise LinkCheckWorkflowError,
          "live-link-check must have an actions/checkout step"
  end
  checkout_with = checkout_step["with"] || {}
  unless checkout_with["persist-credentials"] == false
    raise LinkCheckWorkflowError,
          "checkout step must use persist-credentials: false"
  end

  # Must invoke the hermetic test for the live checker
  test_step = steps.find { |s| s["run"]&.include?("test_check_source_links_live.py") }
  unless test_step
    raise LinkCheckWorkflowError,
          "live-link-check must invoke scripts/test_check_source_links_live.py"
  end

  # Must invoke the live checker script
  check_step = steps.find { |s| s["run"]&.include?("check_source_links_live.py") && !s["run"]&.include?("test_") }
  unless check_step
    raise LinkCheckWorkflowError,
          "live-link-check must invoke scripts/check_source_links_live.py"
  end

  # The live check step must have continue-on-error: true
  unless check_step["continue-on-error"] == true
    raise LinkCheckWorkflowError,
          "live check step must have continue-on-error: true"
  end

  # Must have a stable id
  step_id = check_step["id"]
  unless step_id
    raise LinkCheckWorkflowError,
          "live check step must have a stable id"
  end

  # Must have an outcome observer with if: always()
  observer = steps.find { |s| s["name"]&.include?("Report") && s["name"]&.include?("outcome") }
  unless observer
    raise LinkCheckWorkflowError,
          "live-link-check must have a 'Report ... outcome' step"
  end
  unless observer["if"] == "always()"
    raise LinkCheckWorkflowError,
          "outcome observer must have if: always(), got #{observer['if'].inspect}"
  end

  # The observer must reference .outcome, not .conclusion
  unless text.include?("#{step_id}.outcome")
    raise LinkCheckWorkflowError,
          "outcome observer must inspect steps.#{step_id}.outcome"
  end
  if text.include?("#{step_id}.conclusion")
    raise LinkCheckWorkflowError,
          "outcome observer must not use .conclusion (normalized by continue-on-error)"
  end

  puts "Link-check workflow invariants passed."
rescue LinkCheckWorkflowError => e
  warn "Link-check workflow check failed: #{e.message}"
  exit 1
rescue YAML::SyntaxError => e
  warn "Link-check workflow check failed: invalid YAML: #{e.message}"
  exit 1
end

check!
