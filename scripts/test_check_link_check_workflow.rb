#!/usr/bin/env ruby
# frozen_string_literal: true

# Mutation tests for scripts/check_link_check_workflow.rb (issue #202).
#
# Each test mutates the link-check.yml workflow in a temporary directory
# and verifies that the structural checker rejects the mutation.

require "minitest/autorun"
require "pathname"
require "rbconfig"
require "tempfile"
require "yaml"

REPO_ROOT = Pathname(__dir__).parent
CHECKER = REPO_ROOT / "scripts" / "check_link_check_workflow.rb"
WORKFLOW = REPO_ROOT / ".github" / "workflows" / "link-check.yml"

class TestCheckLinkCheckWorkflow < Minitest::Test
  def run_checker(workflow_text)
    Dir.mktmpdir do |dir|
      root = Pathname(dir)
      workflow_dir = root / ".github" / "workflows"
      workflow_dir.mkpath
      (workflow_dir / "link-check.yml").write(workflow_text)
      output = `#{RbConfig.ruby} #{CHECKER} #{root} 2>&1`
      return $?.exitstatus, output
    end
  end

  def live_workflow
    WORKFLOW.read
  end

  # --- Positive: the live workflow passes ---

  def load_workflow(text)
    # YAML 1.1 parses bare "on" as boolean true; normalize back to "on"
    w = YAML.safe_load(text, aliases: true)
    if w.key?(true) && !w.key?("on")
      w["on"] = w.delete(true)
    end
    w
  end

  def dump_workflow(workflow)
    # Ensure "on" key is serialized correctly
    if workflow.key?("on")
      workflow[true] = workflow.delete("on")
    end
    YAML.dump(workflow)
  end

  def test_live_workflow_passes
    status, output = `#{RbConfig.ruby} #{CHECKER} #{REPO_ROOT} 2>&1`
    assert_equal 0, $?.exitstatus, "live workflow failed:\n#{output}"
  end

  # --- Negative: missing workflow file ---

  def test_rejects_missing_workflow
    Dir.mktmpdir do |dir|
      output = `#{RbConfig.ruby} #{CHECKER} #{dir} 2>&1`
      refute_equal 0, $?.exitstatus, "accepted missing workflow file"
    end
  end

  # --- Negative: pull_request trigger added ---

  def test_rejects_pull_request_trigger
    workflow = load_workflow(live_workflow)
    workflow["on"]["pull_request"] = nil
    status, = run_checker(dump_workflow(workflow))
    refute_equal 0, status, "accepted pull_request trigger"
  end

  # --- Negative: pull_request_target trigger added ---

  def test_rejects_pull_request_target_trigger
    workflow = load_workflow(live_workflow)
    workflow["on"]["pull_request_target"] = nil
    status, = run_checker(dump_workflow(workflow))
    refute_equal 0, status, "accepted pull_request_target trigger"
  end

  # --- Negative: schedule trigger removed ---

  def test_rejects_missing_schedule
    workflow = load_workflow(live_workflow)
    workflow["on"].delete("schedule")
    status, = run_checker(dump_workflow(workflow))
    refute_equal 0, status, "accepted workflow without schedule trigger"
  end

  # --- Negative: workflow_dispatch removed ---

  def test_rejects_missing_workflow_dispatch
    workflow = load_workflow(live_workflow)
    workflow["on"].delete("workflow_dispatch")
    status, = run_checker(dump_workflow(workflow))
    refute_equal 0, status, "accepted workflow without workflow_dispatch"
  end

  # --- Negative: write permissions ---

  def test_rejects_write_permissions
    workflow = load_workflow(live_workflow)
    workflow["permissions"]["contents"] = "write"
    status, = run_checker(dump_workflow(workflow))
    refute_equal 0, status, "accepted write permissions"
  end

  # --- Negative: OIDC permission ---

  def test_rejects_oidc_permission
    workflow = load_workflow(live_workflow)
    workflow["permissions"]["id-token"] = "write"
    status, = run_checker(dump_workflow(workflow))
    refute_equal 0, status, "accepted OIDC write permission"
  end

  # --- Negative: missing live check invocation ---

  def test_rejects_missing_live_check_invocation
    workflow = load_workflow(live_workflow)
    steps = workflow["jobs"]["live-link-check"]["steps"]
    steps.reject! { |s| s["run"]&.include?("check_source_links_live.py") && !s["run"]&.include?("test_") }
    status, = run_checker(dump_workflow(workflow))
    refute_equal 0, status, "accepted workflow without live check invocation"
  end

  # --- Negative: missing hermetic test invocation ---

  def test_rejects_missing_hermetic_test
    workflow = load_workflow(live_workflow)
    steps = workflow["jobs"]["live-link-check"]["steps"]
    steps.reject! { |s| s["run"]&.include?("test_check_source_links_live.py") }
    status, = run_checker(dump_workflow(workflow))
    refute_equal 0, status, "accepted workflow without hermetic test invocation"
  end

  # --- Negative: continue-on-error removed ---

  def test_rejects_missing_continue_on_error
    workflow = load_workflow(live_workflow)
    steps = workflow["jobs"]["live-link-check"]["steps"]
    check_step = steps.find { |s| s["run"]&.include?("check_source_links_live.py") && !s["run"]&.include?("test_") }
    check_step.delete("continue-on-error")
    status, = run_checker(dump_workflow(workflow))
    refute_equal 0, status, "accepted live check without continue-on-error"
  end

  # --- Negative: persist-credentials not false ---

  def test_rejects_persist_credentials_true
    workflow = load_workflow(live_workflow)
    steps = workflow["jobs"]["live-link-check"]["steps"]
    checkout = steps.find { |s| s["uses"]&.include?("actions/checkout") }
    checkout["with"]["persist-credentials"] = true
    status, = run_checker(dump_workflow(workflow))
    refute_equal 0, status, "accepted checkout with persist-credentials: true"
  end

  # --- Negative: outcome observer removed ---

  def test_rejects_missing_outcome_observer
    workflow = load_workflow(live_workflow)
    steps = workflow["jobs"]["live-link-check"]["steps"]
    steps.reject! { |s| s["name"]&.include?("Report") && s["name"]&.include?("outcome") }
    status, = run_checker(dump_workflow(workflow))
    refute_equal 0, status, "accepted workflow without outcome observer"
  end

  # --- Negative: outcome observer uses .conclusion instead of .outcome ---

  def test_rejects_conclusion_in_observer
    workflow_text = live_workflow.sub(".outcome", ".conclusion")
    status, = run_checker(workflow_text)
    refute_equal 0, status, "accepted .conclusion in outcome observer"
  end

  # --- Negative: missing job ---

  def test_rejects_missing_job
    workflow = load_workflow(live_workflow)
    workflow["jobs"].delete("live-link-check")
    status, = run_checker(dump_workflow(workflow))
    refute_equal 0, status, "accepted workflow without live-link-check job"
  end
end
