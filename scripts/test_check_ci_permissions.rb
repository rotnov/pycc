#!/usr/bin/env ruby
# frozen_string_literal: true

require "minitest/autorun"
require "tmpdir"
require_relative "check_ci_permissions"

class WorkflowPermissionsTest < Minitest::Test
  ACTIVE_TRUST_ANCHOR = WORKFLOW_DIRECTORY / TRUST_ANCHOR_FILENAME
  REVIEWED_TRUST_ANCHOR_SNAPSHOT =
    Pathname(__dir__).parent / "tests/fixtures/workflow-policy-search-ledger.yml"
  PROSPECTIVE_SEARCH_LEDGER_TRUST_ANCHOR =
    Pathname(__dir__).parent / "tests/fixtures/workflow-policy-search-ledger.yml"
  ACTIVE_TRUST_ANCHOR_SHA256 =
    "f8d60936438c48362d0a5dc11ee709c9dd5354c3f697038bc36b620c266f0688"
  RETIRED_TRUST_ANCHOR_SHA256 =
    "4dc12b9c053dbc94011ba86c32c7a103afe223582cc94e93ff79255dc6e5b2e6"
  PROSPECTIVE_SEARCH_LEDGER_TRUST_ANCHOR_SHA256 =
    "f8d60936438c48362d0a5dc11ee709c9dd5354c3f697038bc36b620c266f0688"

  def workflow(test_job = "runs-on: ubuntu-latest", trigger: "pull_request", extra_jobs: nil)
    lines = [
      "name: Test",
      "on:",
      "  #{trigger}:",
      "permissions:",
      "  contents: read",
      "jobs:",
      "  test:"
    ]
    lines.concat(test_job.lines(chomp: true).map { |line| "    #{line}" })
    lines.concat(extra_jobs.lines(chomp: true).map { |line| "  #{line}" }) if extra_jobs
    "#{lines.join("\n")}\n"
  end

  def with_environment(overrides)
    previous = overrides.to_h { |name, _value| [name, ENV[name]] }
    overrides.each { |name, value| ENV[name] = value }
    yield
  ensure
    previous.each do |name, value|
      value.nil? ? ENV.delete(name) : ENV[name] = value
    end
  end

  def test_accepts_read_only_pull_request_job
    validate_workflow(workflow)
  end

  def test_rejects_mutable_and_short_action_references
    [
      "actions/setup-python@v5",
      "actions/setup-python@main",
      "actions/setup-python@a26af69"
    ].each do |reference|
      text = workflow(
        "runs-on: ubuntu-latest\nsteps:\n  - uses: #{reference}"
      )
      error = assert_raises(PolicyError) { validate_workflow(text) }
      assert_match(/full commit SHA/, error.message)
    end
  end

  def test_accepts_full_sha_and_local_action_references
    validate_workflow(workflow(<<~YAML))
      runs-on: ubuntu-latest
      steps:
        - uses: actions/setup-python@a26af69be951a213d495a4c3e4e4022e16d87065
        - uses: ./local-action
    YAML
  end

  def test_rejects_mutable_reusable_workflow_reference
    error = assert_raises(PolicyError) do
      validate_workflow(
        workflow("uses: owner/repo/.github/workflows/test.yml@main")
      )
    end
    assert_match(/full commit SHA/, error.message)
  end

  def test_checkout_requires_non_persisted_credentials_even_with_case_drift
    [
      "steps:\n  - uses: actions/checkout@d23441a48e516b6c34aea4fa41551a30e30af803",
      "steps:\n  - uses: Actions/Checkout@d23441a48e516b6c34aea4fa41551a30e30af803\n    with:\n      persist-credentials: true"
    ].each do |steps|
      error = assert_raises(PolicyError) do
        validate_workflow(workflow("runs-on: ubuntu-latest\n#{steps}"))
      end
      assert_match(/persist-credentials.*false/, error.message)
    end
  end

  def test_accepts_explicit_empty_baseline
    validate_workflow(
      workflow.sub("permissions:\n  contents: read", "permissions: {}")
    )
  end

  def test_accepts_quoted_safe_value
    validate_workflow(workflow.sub("contents: read", 'contents: "read"'))
  end

  def test_accepts_guarded_privileged_job
    text = workflow(
      extra_jobs: <<~YAML
        deploy:
          if: github.event_name == 'push' && github.ref == 'refs/heads/main'
          permissions:
            pages: write
            id-token: write
          environment: github-pages
      YAML
    )
    validate_workflow(text)
  end

  def test_accepts_event_and_ref_guard_in_pull_request_target_workflow
    text = workflow(
      <<~YAML,
        if: github.event_name == 'push' && github.ref == 'refs/heads/main'
        runs-on: ubuntu-latest
        permissions:
          contents: write
      YAML
      trigger: "pull_request_target"
    )
    validate_workflow(text)
  end

  def test_accepts_guarded_job_privilege_in_push_only_workflow
    text = workflow(
      <<~YAML,
        if: github.event_name == 'push' && github.ref == 'refs/heads/main'
        runs-on: ubuntu-latest
        permissions:
          contents: write
      YAML
      trigger: "push"
    )
    validate_workflow(text)
  end

  def test_rejects_unguarded_job_privilege_in_push_only_workflow
    text = workflow(
      "runs-on: ubuntu-latest\npermissions:\n  contents: write",
      trigger: "push"
    )
    error = assert_raises(PolicyError) { validate_workflow(text) }
    assert_match(/privileged without an exact push-and-main guard/, error.message)
  end

  def test_rejects_privileged_reusable_workflow
    text = workflow(
      "runs-on: ubuntu-latest\nenvironment: production\nenv:\n  TOKEN: ${{ secrets.DEPLOY_TOKEN }}",
      trigger: "workflow_call"
    )
    error = assert_raises(PolicyError) { validate_workflow(text) }
    assert_match(/privileged without an exact push-and-main guard/, error.message)
  end

  def test_rejects_missing_baseline
    error = assert_raises(PolicyError) do
      validate_workflow(workflow.sub("permissions:\n  contents: read\n", ""))
    end
    assert_match(/missing top-level permissions/, error.message)
  end

  def test_rejects_null_baseline
    error = assert_raises(PolicyError) do
      validate_workflow(workflow.sub("  contents: read", ""))
    end
    assert_match(/must be a mapping/, error.message)
  end

  def test_rejects_spaced_duplicate_key
    text = workflow + "permissions : write-all\n"
    error = assert_raises(PolicyError) { validate_workflow(text) }
    assert_match(/duplicate key "permissions"/, error.message)
  end

  def test_rejects_quoted_duplicate_key
    text = workflow + "\"permissions\": write-all\n"
    error = assert_raises(PolicyError) { validate_workflow(text) }
    assert_match(/duplicate key "permissions"/, error.message)
  end

  def test_rejects_scalar_shortcut
    error = assert_raises(PolicyError) do
      validate_workflow(workflow.sub("permissions:\n  contents: read", "permissions: read-all"))
    end
    assert_match(/must be a mapping/, error.message)
  end

  def test_rejects_top_level_write
    error = assert_raises(PolicyError) do
      validate_workflow(workflow.sub("contents: read", "contents: write"))
    end
    assert_match(/privileged workflow-level permission/, error.message)
  end

  def test_rejects_unguarded_job_write
    text = workflow(
      "runs-on: ubuntu-latest\npermissions:\n  checks: write"
    )
    error = assert_raises(PolicyError) { validate_workflow(text) }
    assert_match(/privileged without an exact push-and-main guard/, error.message)
  end

  def test_rejects_unguarded_oidc
    text = workflow(
      "runs-on: ubuntu-latest\npermissions:\n  id-token: write"
    )
    error = assert_raises(PolicyError) { validate_workflow(text) }
    assert_match(/privileged without an exact push-and-main guard/, error.message)
  end

  def test_rejects_unguarded_environment
    text = workflow(
      "runs-on: ubuntu-latest\nenvironment: production"
    )
    error = assert_raises(PolicyError) { validate_workflow(text) }
    assert_match(/privileged without an exact push-and-main guard/, error.message)
  end

  def test_rejects_unguarded_secret_reference
    text = workflow(
      "runs-on: ubuntu-latest\nenv:\n  TOKEN: ${{ secrets.DEPLOY_TOKEN }}"
    )
    error = assert_raises(PolicyError) { validate_workflow(text) }
    assert_match(/privileged without an exact push-and-main guard/, error.message)
  end

  def test_rejects_root_secret_reference
    text = workflow.sub(
      "permissions:",
      "env:\n  TOKEN: ${{ secrets.DEPLOY_TOKEN }}\npermissions:"
    )
    error = assert_raises(PolicyError) { validate_workflow(text) }
    assert_match(/references a secret outside a guarded job/, error.message)
  end

  def test_rejects_bracket_secret_reference
    text = workflow(
      "runs-on: ubuntu-latest\nenv:\n  TOKEN: ${{ secrets['DEPLOY_TOKEN'] }}"
    )
    error = assert_raises(PolicyError) { validate_workflow(text) }
    assert_match(/privileged without an exact push-and-main guard/, error.message)
  end

  def test_rejects_secret_context_serialization
    text = workflow(
      "runs-on: ubuntu-latest\nenv:\n  TOKEN: ${{ toJSON(secrets) }}"
    )
    error = assert_raises(PolicyError) { validate_workflow(text) }
    assert_match(/privileged without an exact push-and-main guard/, error.message)
  end

  def test_rejects_case_insensitive_secret_context
    text = workflow(
      "runs-on: ubuntu-latest\nenv:\n  TOKEN: ${{ SeCrEtS.DEPLOY_TOKEN }}"
    )
    error = assert_raises(PolicyError) { validate_workflow(text) }
    assert_match(/privileged without an exact push-and-main guard/, error.message)
  end

  def test_rejects_yaml_aliases_in_pull_request_workflows
    text = workflow(
      "runs-on: ubuntu-latest\nenv: &shared\n  SAFE: value",
      extra_jobs: "alias:\n  runs-on: ubuntu-latest\n  env: *shared"
    )
    error = assert_raises(PolicyError) { validate_workflow(text) }
    assert_match(/unsupported YAML alias/, error.message)
  end

  def test_rejects_yaml_merge_keys
    text = workflow(
      "runs-on: ubuntu-latest\n<<:\n  permissions:\n    contents: write"
    )
    error = assert_raises(PolicyError) { validate_workflow(text) }
    assert_match(/unsupported YAML merge key/, error.message)
  end

  def test_rejects_privileged_feature_branch_guard
    text = workflow(
      "if: github.ref == 'refs/heads/feature'\nruns-on: ubuntu-latest\npermissions:\n  contents: write"
    )
    error = assert_raises(PolicyError) { validate_workflow(text) }
    assert_match(/privileged without an exact push-and-main guard/, error.message)
  end

  def test_rejects_ref_only_guard
    text = workflow(
      <<~YAML,
        if: github.ref == 'refs/heads/main'
        runs-on: ubuntu-latest
        permissions:
          contents: write
      YAML
      trigger: "push"
    )
    error = assert_raises(PolicyError) { validate_workflow(text) }
    assert_match(/privileged without an exact push-and-main guard/, error.message)
  end

  def test_rejects_or_guard_that_can_be_true_on_pull_requests
    text = workflow(
      "if: github.ref == 'refs/heads/main' || true\nruns-on: ubuntu-latest\npermissions:\n  contents: write"
    )
    error = assert_raises(PolicyError) { validate_workflow(text) }
    assert_match(/privileged without an exact push-and-main guard/, error.message)
  end

  def test_discovers_both_extensions
    Dir.mktmpdir do |directory|
      root = Pathname(directory)
      (root / "a.yml").write("")
      (root / "b.yaml").write("")
      (root / "ignored.txt").write("")
      assert_equal [root / "a.yml", root / "b.yaml"], discover_workflows(root)
    end
  end

  def test_policy_set_requires_trust_anchor
    Dir.mktmpdir do |directory|
      workflow_path = Pathname(directory) / "ci.yml"
      workflow_path.write(workflow)
      error = assert_raises(PolicyError) { validate_policy_set([workflow_path]) }
      assert_match(/exactly one workflow-policy\.yml/, error.message)
    end
  end

  def test_policy_set_rejects_modified_trust_anchor
    Dir.mktmpdir do |directory|
      anchor = Pathname(directory) / TRUST_ANCHOR_FILENAME
      anchor.write(workflow)
      error = assert_raises(PolicyError) { validate_policy_set([anchor]) }
      assert_match(/approved trust-anchor digest/, error.message)
    end
  end

  def test_policy_set_accepts_repository_trust_anchor
    anchor = WORKFLOW_DIRECTORY / TRUST_ANCHOR_FILENAME
    validate_policy_set([anchor])
  end

  def test_active_trust_anchor_matches_the_reviewed_snapshot
    assert ACTIVE_TRUST_ANCHOR.file?, "missing active trust anchor"
    assert REVIEWED_TRUST_ANCHOR_SNAPSHOT.file?,
           "missing reviewed trust-anchor snapshot"
    return unless ACTIVE_TRUST_ANCHOR.file? && REVIEWED_TRUST_ANCHOR_SNAPSHOT.file?

    assert_equal REVIEWED_TRUST_ANCHOR_SNAPSHOT.read, ACTIVE_TRUST_ANCHOR.read
    digest = Digest::SHA256.file(ACTIVE_TRUST_ANCHOR).hexdigest
    assert_equal ACTIVE_TRUST_ANCHOR_SHA256, digest
    assert_includes TRUST_ANCHOR_SHA256_ALLOWLIST, digest
    refute_includes TRUST_ANCHOR_SHA256_ALLOWLIST, RETIRED_TRUST_ANCHOR_SHA256
  end

  def test_active_trust_anchor_audits_head_roadmap_with_base_checker
    return unless ACTIVE_TRUST_ANCHOR.file?

    text = ACTIVE_TRUST_ANCHOR.read
    validate_workflow(text, ACTIVE_TRUST_ANCHOR.to_s)
    anchor = Psych.load(text)
    steps = anchor.fetch("jobs").fetch("audit").fetch("steps")

    checkout = steps.find { |step| step["name"] == "Check out trusted policy implementation" }
    assert_equal "${{ github.sha }}", checkout.fetch("with").fetch("ref")

    download = steps.find do |step|
      step["name"] == "Download head policy inputs as non-executable data"
    end
    script = download.fetch("with").fetch("script")
    assert_includes script, 'const output = "/tmp/pr-policy-input";'
    assert_includes script, '"docs/ROADMAP.md"'

    run_commands = steps
                   .map { |step| step["run"] }
                   .compact
                   .flat_map { |run| run.lines.map(&:strip) }
    assert_includes run_commands, "ruby scripts/test_check_roadmap_evidence.rb"
    assert_includes run_commands,
                    "ruby scripts/check_roadmap_evidence.rb /tmp/pr-policy-input"

    checker_paths = run_commands.map do |command|
      match = command.match(
        /\A(?:ruby|python3)(?:\s+-\S+)*\s+(\S+\.(?:rb|py))\b/
      )
      match[1] if match
    end.compact
    refute_empty checker_paths
    checker_paths.each do |path|
      assert path.start_with?("scripts/"),
             "trust anchor executed checker outside base scripts/: #{path}"
    end
    run_commands.each do |command|
      refute_match(%r{\A(?:ruby|python3).*?/tmp/pr-policy-input/scripts/}, command)
    end
  end

  def test_pull_request_target_main_does_not_consult_retired_successor_state
    status = with_environment(
      "GITHUB_EVENT_NAME" => "pull_request_target",
      "POLICY_CANDIDATE_ROOT" => "/definitely/missing/policy-candidate"
    ) do
      main([WORKFLOW_DIRECTORY.to_s])
    end

    assert_equal 0, status
  end

  def test_prospective_trust_anchor_audits_search_ledger_as_data
    assert PROSPECTIVE_SEARCH_LEDGER_TRUST_ANCHOR.file?,
           "missing prospective search-ledger trust anchor"
    text = PROSPECTIVE_SEARCH_LEDGER_TRUST_ANCHOR.read
    validate_workflow(text, PROSPECTIVE_SEARCH_LEDGER_TRUST_ANCHOR.to_s)
    digest = Digest::SHA256.hexdigest(text)
    assert_equal PROSPECTIVE_SEARCH_LEDGER_TRUST_ANCHOR_SHA256, digest
    assert_includes TRUST_ANCHOR_SHA256_ALLOWLIST, digest

    anchor = Psych.load(text)
    steps = anchor.fetch("jobs").fetch("audit").fetch("steps")
    download = steps.find do |step|
      step["name"] == "Download head policy inputs as non-executable data"
    end
    script = download.fetch("with").fetch("script")
    %w[
      docs/ROADMAP.md
      docs/SEARCH_QUERY_REGISTRY.json
      docs/SEARCH_VISIBILITY.md
      docs/SEARCH_VISIBILITY_CHECKPOINTS.json
    ].each { |path| assert_includes script, path.inspect }
    assert_includes script,
                    'entry.type !== "blob" || entry.mode !== "100644"'
    assert_includes script,
                    "Policy input is not a regular non-executable file"
    assert_includes script, 'entry.path === ".gitattributes"'
    assert_includes script, 'entry.path.endsWith("/.gitattributes")'
    assert_includes script,
                    "Head revision contains checkout-affecting .gitattributes"
    assert_includes script,
                    "tests/fixtures/policy-successor-manifest.json".inspect
    assert_includes script, "Trusted policy successor manifest"
    assert_includes script, "Candidate policy successor manifest"
    refute_includes script, 'entry.type === "blob" && isWorkflow(entry.path)'
    refute_includes script,
                    'candidate.type === "blob" && candidate.path === requiredPath'

    commands = steps
               .map { |step| step["run"] }
               .compact
               .flat_map { |run| run.lines.map(&:strip) }
    assert_includes commands,
                    "python3 -I -B scripts/test_check_search_visibility_audit.py"
    assert_includes commands, "--head-root /tmp/pr-policy-input \\"
    assert_includes commands, '--base-root "$GITHUB_WORKSPACE"'
    assert_includes commands, "python3 -I scripts/check_search_visibility_audit.py \\"
    policy = steps.find { |step| step["name"] == "Audit head workflow permissions" }
    assert_equal "/tmp/pr-policy-input",
                 policy.fetch("env").fetch("POLICY_CANDIDATE_ROOT")
  end

end
