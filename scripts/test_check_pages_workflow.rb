#!/usr/bin/env ruby
# frozen_string_literal: true

# Test suite for scripts/check_pages_workflow.rb — validates the
# structural invariants of .github/workflows/pages.yml, focusing on
# the IndexNow notification observability requirements from issue #205:
#   - the notify-indexnow step has a stable id
#   - the step uses continue-on-error: true (deployment is never rolled back)
#   - an if: always() follow-up step inspects the step's outcome
#   - the follow-up step references steps.<id>.outcome (not conclusion)
#   - the notifier job runs only after deploy and only on push to main
#   - the notifier job has unprivileged permissions (contents: read)

require "minitest/autorun"
require "pathname"
require "rbconfig"
require "tempfile"
require "yaml"

REPO_ROOT = Pathname(__dir__).parent
CHECKER = REPO_ROOT / "scripts" / "check_pages_workflow.rb"
PAGES_WORKFLOW = REPO_ROOT / ".github" / "workflows" / "pages.yml"

class TestCheckPagesWorkflow < Minitest::Test
  def run_checker(workflow_text)
    Dir.mktmpdir do |dir|
      root = Pathname(dir)
      wf_dir = root / ".github" / "workflows"
      wf_dir.mkpath
      (wf_dir / "pages.yml").write(workflow_text)
      output = `#{RbConfig.ruby} #{CHECKER} #{root} 2>&1`
      return $?.exitstatus, output
    end
  end

  def read_live_workflow
    PAGES_WORKFLOW.read
  end

  # --- Positive: the live pages.yml passes ---

  def test_live_pages_workflow_passes
    status, output = run_checker(read_live_workflow)
    assert_equal 0, status, "live pages.yml failed:\n#{output}"
  end

  # --- Structural invariants of the notify-indexnow job ---

  def test_live_workflow_has_notify_indexnow_job
    workflow = YAML.load_file(PAGES_WORKFLOW)
    jobs = workflow["jobs"]
    assert jobs.key?("notify-indexnow"),
           "pages.yml must have a notify-indexnow job"
  end

  def test_live_notify_job_runs_only_on_push_to_main
    workflow = YAML.load_file(PAGES_WORKFLOW)
    job = workflow["jobs"]["notify-indexnow"]
    assert_equal "github.event_name == 'push' && github.ref == 'refs/heads/main'",
                 job["if"],
                 "notify-indexnow must run only on push to main"
  end

  def test_live_notify_job_needs_deploy
    workflow = YAML.load_file(PAGES_WORKFLOW)
    job = workflow["jobs"]["notify-indexnow"]
    assert_includes job["needs"], "deploy",
                    "notify-indexnow must need deploy"
  end

  def test_live_notify_job_has_contents_read_only
    workflow = YAML.load_file(PAGES_WORKFLOW)
    job = workflow["jobs"]["notify-indexnow"]
    perms = job["permissions"]
    assert_equal({ "contents" => "read" }, perms,
                 "notify-indexnow must have contents: read only")
  end

  def test_live_notify_step_has_id
    workflow = YAML.load_file(PAGES_WORKFLOW)
    steps = workflow["jobs"]["notify-indexnow"]["steps"]
    notify_step = steps.find { |s| s["name"]&.include?("Notify IndexNow") }
    assert notify_step, "must have a Notify IndexNow step"
    assert_equal "notify-indexnow", notify_step["id"],
                 "Notify IndexNow step must have id: notify-indexnow"
  end

  def test_live_notify_step_has_continue_on_error
    workflow = YAML.load_file(PAGES_WORKFLOW)
    steps = workflow["jobs"]["notify-indexnow"]["steps"]
    notify_step = steps.find { |s| s["name"]&.include?("Notify IndexNow") }
    assert_equal true, notify_step["continue-on-error"],
                 "Notify IndexNow step must have continue-on-error: true"
  end

  def test_live_workflow_has_outcome_observer_step
    workflow = YAML.load_file(PAGES_WORKFLOW)
    steps = workflow["jobs"]["notify-indexnow"]["steps"]
    observer = steps.find { |s| s["name"]&.include?("Report IndexNow outcome") }
    assert observer, "must have a Report IndexNow outcome step"
    assert_equal "always()", observer["if"],
                 "outcome observer must have if: always()"
  end

  def test_live_observer_references_outcome_not_conclusion
    text = read_live_workflow
    assert_includes text, "steps.notify-indexnow.outcome",
                    "observer must inspect .outcome (not .conclusion)"
    refute_includes text, "steps.notify-indexnow.conclusion",
                    "observer must not use .conclusion (normalized by continue-on-error)"
  end

  # --- Negative controls: mutations that must be rejected ---

  def test_rejects_missing_step_id
    text = read_live_workflow.gsub("id: notify-indexnow", "# id removed")
    status, = run_checker(text)
    refute_equal 0, status, "accepted notify step without id"
  end

  def test_rejects_missing_continue_on_error
    text = read_live_workflow.gsub("continue-on-error: true", "# continue-on-error removed")
    status, = run_checker(text)
    refute_equal 0, status, "accepted notify step without continue-on-error"
  end

  def test_rejects_missing_outcome_observer
    # Remove the entire "Report IndexNow outcome" step block
    text = read_live_workflow.gsub(
      /\n      - name: Report IndexNow outcome\n.*?(?=\n      - name:|\Z)/m,
      ""
    )
    status, = run_checker(text)
    refute_equal 0, status, "accepted workflow without outcome observer"
  end

  def test_rejects_observer_without_if_always
    text = read_live_workflow.sub("if: always()\n        run: |\n          echo \"IndexNow notification outcome",
                                  "if: success()\n        run: |\n          echo \"IndexNow notification outcome")
    status, = run_checker(text)
    refute_equal 0, status, "accepted outcome observer without if: always()"
  end

  def test_rejects_observer_using_conclusion
    text = read_live_workflow.gsub("steps.notify-indexnow.outcome",
                                   "steps.notify-indexnow.conclusion")
    status, = run_checker(text)
    refute_equal 0, status, "accepted observer using .conclusion instead of .outcome"
  end

  def test_rejects_notify_job_with_write_permissions
    text = read_live_workflow.gsub(
      /notify-indexnow:\n    if:.*?\n    permissions:\n      contents: read/,
      "notify-indexnow:\n    if: github.event_name == 'push' && github.ref == 'refs/heads/main'\n    permissions:\n      contents: write"
    )
    status, = run_checker(text)
    refute_equal 0, status, "accepted notify job with contents: write"
  end

  def test_rejects_notify_job_running_on_pull_request
    text = read_live_workflow.sub(
      "if: github.event_name == 'push' && github.ref == 'refs/heads/main'\n    permissions:\n      contents: read\n    runs-on: ubuntu-latest\n    needs: deploy",
      "if: github.event_name == 'pull_request'\n    permissions:\n      contents: read\n    runs-on: ubuntu-latest\n    needs: deploy"
    )
    status, = run_checker(text)
    refute_equal 0, status, "accepted notify job running on pull_request"
  end

  def test_rejects_notify_job_without_deploy_dependency
    text = read_live_workflow.sub("needs: deploy", "needs: build")
    status, = run_checker(text)
    refute_equal 0, status, "accepted notify job without deploy dependency"
  end

  # --- Issue #160: hermetic IndexNow test must be bound to CI ---

  def test_live_build_job_invokes_test_notify_indexnow
    workflow = YAML.load_file(PAGES_WORKFLOW)
    build_steps = workflow["jobs"]["build"]["steps"]
    validate_step = build_steps.find { |s| s["name"]&.include?("Validate website") }
    assert validate_step, "build job must have a Validate website step"
    assert_includes validate_step["run"], "test-notify-indexnow.py",
                    "Validate website step must invoke test-notify-indexnow.py"
  end

  def test_rejects_build_job_without_test_notify_indexnow
    text = read_live_workflow.sub(
      "python3 ./scripts/test-notify-indexnow.py\n",
      "echo 'indexnow test removed'\n"
    )
    status, = run_checker(text)
    refute_equal 0, status,
                 "accepted build job without test-notify-indexnow.py invocation"
  end

  def test_rejects_build_job_without_validate_website_step
    text = read_live_workflow.sub(
      "- name: Validate website",
      "- name: Check website"
    )
    status, = run_checker(text)
    refute_equal 0, status, "accepted build job without Validate website step"
  end
end
