#!/usr/bin/env ruby
# frozen_string_literal: true

require "minitest/autorun"
require "tmpdir"
require "fileutils"
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

  def write_policy_input(root, relative, content)
    destination = root / relative
    FileUtils.mkdir_p(destination.dirname)
    destination.binwrite(content)
  end

  def successor_manifest(entries)
    JSON.pretty_generate(
      "manifest_version" => 1,
      "targets" => entries
    ) + "\n"
  end

  def successor_entry(target, content, source: target)
    {
      "path" => target,
      "source_path" => source,
      "sha256" => Digest::SHA256.hexdigest(content)
    }
  end

  def test_accepts_read_only_pull_request_job
    validate_workflow(workflow)
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
    assert_includes script, POLICY_SUCCESSOR_MANIFEST_PATH.inspect
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

  def test_steady_state_policy_accepts_an_unchanged_protected_target
    Dir.mktmpdir do |directory|
      root = Pathname(directory)
      base = root / "base"
      candidate = root / "candidate"
      target = "scripts/trusted.rb"
      content = "trusted\n"
      manifest = successor_manifest([successor_entry(target, content)])
      [base, candidate].each do |tree|
        write_policy_input(tree, target, content)
        write_policy_input(tree, POLICY_SUCCESSOR_MANIFEST_PATH, manifest)
      end

      validate_policy_successor_transition(candidate, base)
    end
  end

  def test_steady_state_policy_wrapper_requires_and_audits_candidate_root
    Dir.mktmpdir do |directory|
      root = Pathname(directory)
      base = root / "base"
      candidate = root / "candidate"
      target = "scripts/trusted.rb"
      content = "trusted\n"
      manifest = successor_manifest([successor_entry(target, content)])
      [base, candidate].each do |tree|
        write_policy_input(tree, target, content)
        write_policy_input(tree, POLICY_SUCCESSOR_MANIFEST_PATH, manifest)
      end
      write_policy_input(
        base,
        SEARCH_ACTIVATED_TRUST_ANCHOR_PATH,
        PROSPECTIVE_SEARCH_LEDGER_TRUST_ANCHOR.binread
      )

      error = assert_raises(PolicyError) do
        validate_steady_state_policy_inputs(
          event_name: "pull_request_target",
          candidate_root: nil,
          repository_root: base
        )
      end
      assert_match(/POLICY_CANDIDATE_ROOT/, error.message)
      validate_steady_state_policy_inputs(
        event_name: "pull_request_target",
        candidate_root: candidate.to_s,
        repository_root: base
      )
    end
  end

  def test_steady_state_policy_rejects_same_pr_self_authorization
    Dir.mktmpdir do |directory|
      root = Pathname(directory)
      base = root / "base"
      candidate = root / "candidate"
      target = "scripts/trusted.rb"
      proposal = "tests/fixtures/policy-successors/trusted.rb"
      trusted = "trusted\n"
      replacement = "no-op\n"
      base_manifest = successor_manifest([successor_entry(target, trusted)])
      candidate_manifest = successor_manifest(
        [successor_entry(target, replacement, source: proposal)]
      )
      write_policy_input(base, target, trusted)
      write_policy_input(base, POLICY_SUCCESSOR_MANIFEST_PATH, base_manifest)
      write_policy_input(candidate, target, replacement)
      write_policy_input(candidate, proposal, replacement)
      write_policy_input(
        candidate, POLICY_SUCCESSOR_MANIFEST_PATH, candidate_manifest
      )

      error = assert_raises(PolicyError) do
        validate_policy_successor_transition(candidate, base)
      end
      assert_match(/lacks a base-staged successor/, error.message)
    end
  end

  def test_steady_state_policy_accepts_a_base_staged_successor
    Dir.mktmpdir do |directory|
      root = Pathname(directory)
      base = root / "base"
      candidate = root / "candidate"
      target = "scripts/trusted.rb"
      proposal = "tests/fixtures/policy-successors/trusted.rb"
      trusted = "trusted\n"
      replacement = "reviewed replacement\n"
      base_manifest = successor_manifest(
        [successor_entry(target, replacement, source: proposal)]
      )
      candidate_manifest = successor_manifest(
        [successor_entry(target, replacement)]
      )
      write_policy_input(base, target, trusted)
      write_policy_input(base, proposal, replacement)
      write_policy_input(base, POLICY_SUCCESSOR_MANIFEST_PATH, base_manifest)
      write_policy_input(candidate, target, replacement)
      write_policy_input(
        candidate, POLICY_SUCCESSOR_MANIFEST_PATH, candidate_manifest
      )

      validate_policy_successor_transition(candidate, base)
    end
  end

  def test_steady_state_policy_rejects_a_shrunk_or_invalid_next_manifest
    Dir.mktmpdir do |directory|
      root = Pathname(directory)
      base = root / "base"
      candidate = root / "candidate"
      first = "scripts/first.rb"
      second = "scripts/second.rb"
      first_content = "first\n"
      second_content = "second\n"
      base_manifest = successor_manifest(
        [
          successor_entry(first, first_content),
          successor_entry(second, second_content)
        ]
      )
      candidate_manifest = successor_manifest(
        [successor_entry(first, first_content)]
      )
      [[base, first, first_content], [base, second, second_content],
       [candidate, first, first_content]].each do |tree, relative, content|
        write_policy_input(tree, relative, content)
      end
      write_policy_input(base, POLICY_SUCCESSOR_MANIFEST_PATH, base_manifest)
      write_policy_input(
        candidate, POLICY_SUCCESSOR_MANIFEST_PATH, candidate_manifest
      )

      error = assert_raises(PolicyError) do
        validate_policy_successor_transition(candidate, base)
      end
      assert_match(/removes protected targets/, error.message)
    end
  end

  def test_steady_state_policy_rejects_a_mismatched_candidate_proposal_digest
    Dir.mktmpdir do |directory|
      root = Pathname(directory)
      base = root / "base"
      candidate = root / "candidate"
      target = "scripts/trusted.rb"
      added = "scripts/future.rb"
      proposal = "tests/fixtures/policy-successors/future.rb"
      trusted = "trusted\n"
      future = "future\n"
      base_manifest = successor_manifest([successor_entry(target, trusted)])
      future_entry = successor_entry(added, future, source: proposal)
      future_entry["sha256"] = "0" * 64
      candidate_manifest = successor_manifest(
        [successor_entry(target, trusted), future_entry]
      )
      [base, candidate].each do |tree|
        write_policy_input(tree, target, trusted)
      end
      write_policy_input(base, POLICY_SUCCESSOR_MANIFEST_PATH, base_manifest)
      write_policy_input(candidate, added, future)
      write_policy_input(candidate, proposal, future)
      write_policy_input(
        candidate, POLICY_SUCCESSOR_MANIFEST_PATH, candidate_manifest
      )

      error = assert_raises(PolicyError) do
        validate_policy_successor_transition(candidate, base)
      end
      assert_match(/source digest does not match/, error.message)
    end
  end

  def test_policy_successor_manifest_rejects_unsafe_sources
    ["../trusted.rb", "scripts/trusted\n.rb", "scripts/путь.rb"].each do |source|
      text = successor_manifest(
        [{
          "path" => "scripts/trusted.rb",
          "source_path" => source,
          "sha256" => "0" * 64
        }]
      )
      error = assert_raises(PolicyError) do
        parse_policy_successor_manifest(text)
      end
      assert_match(/safe repository-relative path/, error.message)
    end
  end

  def test_policy_successor_manifest_rejects_float_version_and_duplicate_keys
    entry = successor_entry("scripts/trusted.rb", "trusted\n")
    float_version = successor_manifest([entry]).sub(
      '"manifest_version": 1', '"manifest_version": 1.0'
    )
    error = assert_raises(PolicyError) do
      parse_policy_successor_manifest(float_version)
    end
    assert_match(/manifest_version must be 1/, error.message)

    duplicate = successor_manifest([entry]).sub(
      '"manifest_version": 1',
      '"manifest_version": 1, "manifest_version": 1'
    )
    error = assert_raises(PolicyError) do
      parse_policy_successor_manifest(duplicate)
    end
    assert_match(/duplicate JSON key/, error.message)
  end

end
