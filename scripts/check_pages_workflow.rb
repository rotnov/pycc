#!/usr/bin/env ruby
# frozen_string_literal: true

# Validates structural invariants of .github/workflows/pages.yml,
# focusing on the IndexNow notification observability requirements
# from issue #205:
#
#   - the notify-indexnow step has a stable id
#   - the step uses continue-on-error: true (deployment is never rolled back)
#   - an if: always() follow-up step inspects the step's outcome
#   - the follow-up step references steps.<id>.outcome (not conclusion)
#   - the notifier job runs only after deploy and only on push to main
#   - the notifier job has unprivileged permissions (contents: read)
#
# Usage: ruby scripts/check_pages_workflow.rb [repository_root]
#
# Exits 0 if all invariants hold, 1 otherwise.

require "pathname"
require "yaml"

class PagesWorkflowError < StandardError; end

REPO_ROOT = Pathname(ARGV[0] || Pathname(__dir__).parent)
WORKFLOW_PATH = REPO_ROOT / ".github" / "workflows" / "pages.yml"

def check!
  raise PagesWorkflowError, "pages.yml not found at #{WORKFLOW_PATH}" unless WORKFLOW_PATH.exist?

  text = WORKFLOW_PATH.read
  workflow = YAML.safe_load(text, aliases: true)
  jobs = workflow["jobs"]

  raise PagesWorkflowError, "pages.yml must define jobs" unless jobs

  # --- notify-indexnow job ---
  notify_job = jobs["notify-indexnow"]
  raise PagesWorkflowError, "pages.yml must have a notify-indexnow job" unless notify_job

  # Must run only on push to main
  if_cond = notify_job["if"]
  unless if_cond == "github.event_name == 'push' && github.ref == 'refs/heads/main'"
    raise PagesWorkflowError,
          "notify-indexnow must run only on push to main, got if: #{if_cond.inspect}"
  end

  # Must need deploy
  needs = notify_job["needs"]
  unless needs&.include?("deploy")
    raise PagesWorkflowError,
          "notify-indexnow must need deploy, got needs: #{needs.inspect}"
  end

  # Must have contents: read only
  perms = notify_job["permissions"]
  unless perms == { "contents" => "read" }
    raise PagesWorkflowError,
          "notify-indexnow must have permissions: contents: read only, got: #{perms.inspect}"
  end

  steps = notify_job["steps"]
  raise PagesWorkflowError, "notify-indexnow must have steps" unless steps

  # Find the Notify IndexNow step
  notify_step = steps.find { |s| s["name"]&.include?("Notify IndexNow") }
  raise PagesWorkflowError, "notify-indexnow must have a 'Notify IndexNow' step" unless notify_step

  # Must have a stable id
  step_id = notify_step["id"]
  unless step_id == "notify-indexnow"
    raise PagesWorkflowError,
          "Notify IndexNow step must have id: notify-indexnow, got: #{step_id.inspect}"
  end

  # Must have continue-on-error: true
  unless notify_step["continue-on-error"] == true
    raise PagesWorkflowError,
          "Notify IndexNow step must have continue-on-error: true"
  end

  # Must have an outcome observer step with if: always()
  observer = steps.find { |s| s["name"]&.include?("Report IndexNow outcome") }
  unless observer
    raise PagesWorkflowError,
          "notify-indexnow must have a 'Report IndexNow outcome' step"
  end

  unless observer["if"] == "always()"
    raise PagesWorkflowError,
          "outcome observer must have if: always(), got: #{observer['if'].inspect}"
  end

  # The observer must reference .outcome, not .conclusion
  observer_text = text
  unless observer_text.include?("steps.notify-indexnow.outcome")
    raise PagesWorkflowError,
          "outcome observer must inspect steps.notify-indexnow.outcome"
  end

  if observer_text.include?("steps.notify-indexnow.conclusion")
    raise PagesWorkflowError,
          "outcome observer must not use .conclusion (normalized by continue-on-error)"
  end

  # The notifier step must have GITHUB_STEP_SUMMARY env for the summary
  step_env = notify_step["env"]
  unless step_env && step_env["GITHUB_STEP_SUMMARY"]
    raise PagesWorkflowError,
          "Notify IndexNow step must set GITHUB_STEP_SUMMARY env for job summary"
  end

  puts "Pages workflow invariants passed."
rescue PagesWorkflowError => e
  warn "Pages workflow check failed: #{e.message}"
  exit 1
end

check!
