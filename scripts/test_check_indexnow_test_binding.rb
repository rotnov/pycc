#!/usr/bin/env ruby
# frozen_string_literal: true

# Test suite for scripts/check_indexnow_test_binding.rb — validates that
# the hermetic IndexNow HTTP test (scripts/test-notify-indexnow.py) is
# effectively executed on every relevant pull request with fail-closed
# exit propagation (issue #160).
#
# Mutation tests cover every recorded bypass:
#   - deleting or renaming the fixture invocation;
#   - a push-only job- or step-level `if` condition;
#   - job- or step-level `continue-on-error`;
#   - shell control flow that absorbs the exit status (`|| true`, `; true`,
#     `&& true`, background `&`, piping into `true`);
#   - removal of the pull_request path coverage for the fixture script.

require "minitest/autorun"
require "pathname"
require "rbconfig"
require "tempfile"
require "yaml"

REPO_ROOT = Pathname(__dir__).parent
CHECKER = REPO_ROOT / "scripts" / "check_indexnow_test_binding.rb"
PAGES_WORKFLOW = REPO_ROOT / ".github" / "workflows" / "pages.yml"
FIXTURE_SCRIPT = "scripts/test-notify-indexnow.py"

class TestCheckIndexNowTestBinding < Minitest::Test
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

  def test_live_build_job_invokes_fixture
    workflow = YAML.load_file(PAGES_WORKFLOW)
    build_steps = workflow["jobs"]["build"]["steps"]
    validate_step = build_steps.find { |s| s["name"]&.include?("Validate website") }
    assert validate_step, "build job must have a Validate website step"
    assert_includes validate_step["run"], FIXTURE_SCRIPT,
                    "Validate website step must invoke #{FIXTURE_SCRIPT}"
  end

  def test_live_pull_request_paths_cover_fixture
    workflow = YAML.load_file(PAGES_WORKFLOW)
    # YAML 1.1 parses `on:` as boolean true.
    triggers = workflow["on"] || workflow[true]
    pr = triggers["pull_request"]
    assert pr, "pages.yml must have a pull_request trigger"
    paths = pr["paths"]
    assert_includes paths, FIXTURE_SCRIPT,
                    "pull_request paths must cover #{FIXTURE_SCRIPT}"
  end

  # --- Negative: deleting the fixture invocation ---

  def test_rejects_deleted_fixture_invocation
    text = read_live_workflow.sub(
      "python3 ./scripts/test-notify-indexnow.py\n",
      "echo 'indexnow test removed'\n"
    )
    status, output = run_checker(text)
    refute_equal 0, status,
                 "accepted build job without #{FIXTURE_SCRIPT} invocation\n#{output}"
  end

  def test_rejects_renamed_fixture_invocation
    text = read_live_workflow.sub(
      "python3 ./scripts/test-notify-indexnow.py\n",
      "python3 ./scripts/test_notify_indexnow.py\n"
    )
    status, = run_checker(text)
    refute_equal 0, status, "accepted renamed fixture invocation"
  end

  # --- Negative: push-only step-level if condition ---

  def test_rejects_push_only_step_if
    text = read_live_workflow.sub(
      "      - name: Validate website\n        run: |",
      "      - name: Validate website\n        if: github.event_name == 'push'\n        run: |"
    )
    status, output = run_checker(text)
    refute_equal 0, status,
                 "accepted push-only if on fixture step\n#{output}"
  end

  # --- Negative: step-level continue-on-error ---

  def test_rejects_step_continue_on_error
    text = read_live_workflow.sub(
      "      - name: Validate website\n        run: |",
      "      - name: Validate website\n        continue-on-error: true\n        run: |"
    )
    status, output = run_checker(text)
    refute_equal 0, status,
                 "accepted continue-on-error on fixture step\n#{output}"
  end

  # --- Negative: job-level continue-on-error ---

  def test_rejects_job_continue_on_error
    text = read_live_workflow.sub(
      "  build:\n    runs-on: ubuntu-latest",
      "  build:\n    runs-on: ubuntu-latest\n    continue-on-error: true"
    )
    status, output = run_checker(text)
    refute_equal 0, status,
                 "accepted continue-on-error on build job\n#{output}"
  end

  # --- Negative: job-level push-only if condition ---

  def test_rejects_job_push_only_if
    text = read_live_workflow.sub(
      "  build:\n    runs-on: ubuntu-latest",
      "  build:\n    runs-on: ubuntu-latest\n    if: github.event_name == 'push'"
    )
    status, output = run_checker(text)
    refute_equal 0, status,
                 "accepted push-only if on build job\n#{output}"
  end

  # --- Negative: exit-status absorption via shell control flow ---

  def test_rejects_or_true_absorption
    text = read_live_workflow.sub(
      "python3 ./scripts/test-notify-indexnow.py\n",
      "python3 ./scripts/test-notify-indexnow.py || true\n"
    )
    status, output = run_checker(text)
    refute_equal 0, status,
                 "accepted || true exit absorption\n#{output}"
  end

  def test_rejects_semicolon_true_absorption
    text = read_live_workflow.sub(
      "python3 ./scripts/test-notify-indexnow.py\n",
      "python3 ./scripts/test-notify-indexnow.py ; true\n"
    )
    status, output = run_checker(text)
    refute_equal 0, status,
                 "accepted ; true exit absorption\n#{output}"
  end

  def test_rejects_and_true_absorption
    text = read_live_workflow.sub(
      "python3 ./scripts/test-notify-indexnow.py\n",
      "python3 ./scripts/test-notify-indexnow.py && true\n"
    )
    status, output = run_checker(text)
    refute_equal 0, status,
                 "accepted && true exit absorption\n#{output}"
  end

  def test_rejects_pipe_true_absorption
    text = read_live_workflow.sub(
      "python3 ./scripts/test-notify-indexnow.py\n",
      "python3 ./scripts/test-notify-indexnow.py | true\n"
    )
    status, output = run_checker(text)
    refute_equal 0, status,
                 "accepted pipe-to-true exit absorption\n#{output}"
  end

  def test_rejects_background_ampersand
    text = read_live_workflow.sub(
      "python3 ./scripts/test-notify-indexnow.py\n",
      "python3 ./scripts/test-notify-indexnow.py &\n"
    )
    status, output = run_checker(text)
    refute_equal 0, status,
                 "accepted background & exit absorption\n#{output}"
  end

  # --- Negative: removal of pull_request path coverage ---

  def test_rejects_missing_fixture_in_pull_request_paths
    text = read_live_workflow.gsub(
      "      - \"#{FIXTURE_SCRIPT}\"\n",
      ""
    )
    status, output = run_checker(text)
    refute_equal 0, status,
                 "accepted pull_request paths without #{FIXTURE_SCRIPT}\n#{output}"
  end

  # --- Negative: missing pull_request trigger entirely ---

  def test_rejects_missing_pull_request_trigger
    text = read_live_workflow.sub(
      "  pull_request:\n    paths:",
      "  # pull_request:\n    # paths:"
    )
    status, output = run_checker(text)
    refute_equal 0, status,
                 "accepted workflow without pull_request trigger\n#{output}"
  end

  # --- Negative: missing build job ---

  def test_rejects_missing_build_job
    text = read_live_workflow.sub("  build:\n", "  website-build:\n")
    status, output = run_checker(text)
    refute_equal 0, status, "accepted workflow without build job\n#{output}"
  end

  # --- Negative: invalid YAML ---

  def test_rejects_invalid_yaml
    status, output = run_checker("this: is: not: valid: yaml: [")
    refute_equal 0, status, "accepted invalid YAML\n#{output}"
  end

  # --- Positive: a minimal valid workflow passes ---

  def test_minimal_valid_workflow_passes
    minimal = <<~YAML
      name: Pages
      on:
        pull_request:
          paths:
            - "#{FIXTURE_SCRIPT}"
            - ".github/workflows/pages.yml"
      permissions:
        contents: read
      jobs:
        build:
          runs-on: ubuntu-latest
          steps:
            - name: Check out repository
              uses: actions/checkout@v6
            - name: Validate website
              run: |
                python3 ./scripts/test-notify-indexnow.py
    YAML
    status, output = run_checker(minimal)
    assert_equal 0, status, "minimal valid workflow failed:\n#{output}"
  end
end
