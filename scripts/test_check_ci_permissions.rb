#!/usr/bin/env ruby
# frozen_string_literal: true

require "minitest/autorun"
require "tmpdir"
require "fileutils"
require_relative "check_ci_permissions"

class WorkflowPermissionsTest < Minitest::Test
  ACTIVE_TRUST_ANCHOR = WORKFLOW_DIRECTORY / TRUST_ANCHOR_FILENAME
  REVIEWED_TRUST_ANCHOR_SNAPSHOT =
    Pathname(__dir__).parent / "tests/fixtures/workflow-policy-roadmap-evidence.yml"
  PROSPECTIVE_SEARCH_LEDGER_TRUST_ANCHOR =
    Pathname(__dir__).parent / "tests/fixtures/workflow-policy-search-ledger.yml"
  ACTIVE_TRUST_ANCHOR_SHA256 =
    "4dc12b9c053dbc94011ba86c32c7a103afe223582cc94e93ff79255dc6e5b2e6"
  RETIRED_TRUST_ANCHOR_SHA256 =
    "3a8b56776e7d44f32759301f0691220800ee6f3184b2702d13c01a28f82ce277"
  PROSPECTIVE_SEARCH_LEDGER_TRUST_ANCHOR_SHA256 =
    "6a0c1c7280d1cadcfeec790662aca0cf5210c76aa4c9f6e2333c57a1e8db3a31"

  def activation_candidate
    root = Pathname(__dir__).parent
    candidate = SEARCH_ACTIVATION_PATHS.to_h do |relative|
      [relative, activated_successor_executable(relative, root)]
    end
    candidate.merge(
      SEARCH_GIT_ATTRIBUTES_KEY => SEARCH_GIT_ATTRIBUTES_MANIFEST,
      SEARCH_TREE_ENTRIES_KEY => SEARCH_ACTIVATION_TREE_ENTRIES
    )
  end

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

    commands = steps
               .map { |step| step["run"] }
               .compact
               .flat_map { |run| run.lines.map(&:strip) }
    assert_includes commands,
                    "python3 -I -B scripts/test_check_search_visibility_audit.py"
    assert_includes commands, "--head-root /tmp/pr-policy-input \\"
    assert_includes commands, '--base-root "$GITHUB_WORKSPACE"'
    assert_includes commands, "python3 -I scripts/check_search_visibility_audit.py \\"
  end

  def test_activation_trust_anchor_preserves_staged_search_assets
    candidate = activation_candidate
    STAGED_SEARCH_ACTIVATION_SHA256.each do |relative, digest|
      content = candidate.fetch(relative)
      assert_equal digest, Digest::SHA256.hexdigest(content)
    end
    Dir.mktmpdir do |directory|
      anchor = Pathname(directory) / TRUST_ANCHOR_FILENAME
      anchor.binwrite(PROSPECTIVE_SEARCH_LEDGER_TRUST_ANCHOR.binread)
      validate_search_activation_transition(
        [anchor],
        event_name: "pull_request_target",
        data_loader: ->(_paths) { candidate }
      )
    end
  end

  def test_activated_policy_retires_transition_and_passes_its_self_tests
    root = Pathname(__dir__).parent
    checker = activated_successor_executable(ACTIVATED_POLICY_CHECKER_PATH, root)
    tests = activated_successor_executable(ACTIVATED_POLICY_TEST_PATH, root)
    retired_anchor =
      "4dc12b9c053dbc94011ba86c32c7a103" \
      "afe223582cc94e93ff79255dc6e5b2e6"

    refute_includes checker, "  #{retired_anchor}\n"
    refute_includes checker,
                    "  validate_search_" + "activation_transition(paths)\n"
    assert_includes checker, "  #{SEARCH_LEDGER_TRUST_ANCHOR_SHA256}\n"
    refute_includes tests, "\n  def activation_candidate\n"
    refute_includes tests,
                    "\n  def test_activation_trust_anchor_preserves_staged_search_assets\n"

    Dir.mktmpdir do |directory|
      activated_root = Pathname(directory)
      FileUtils.mkdir_p(activated_root / "scripts")
      FileUtils.mkdir_p(activated_root / "tests/fixtures")
      FileUtils.mkdir_p(activated_root / ".github/workflows")
      (activated_root / ACTIVATED_POLICY_CHECKER_PATH).binwrite(checker)
      (activated_root / ACTIVATED_POLICY_TEST_PATH).binwrite(tests)
      fixture = PROSPECTIVE_SEARCH_LEDGER_TRUST_ANCHOR.binread
      (activated_root / "tests/fixtures/workflow-policy-search-ledger.yml")
        .binwrite(fixture)
      (activated_root / ".github/workflows/workflow-policy.yml")
        .binwrite(fixture)

      stdout, stderr, status = Open3.capture3(
        "ruby",
        ACTIVATED_POLICY_TEST_PATH,
        chdir: activated_root.to_s
      )
      assert status.success?, "#{stdout}\n#{stderr}"
    end
  end

  def test_activation_compares_successor_executables_as_bytes
    candidate = activation_candidate.transform_values do |content|
      content.is_a?(String) ? content.dup.force_encoding(Encoding::UTF_8) : content
    end
    Dir.mktmpdir do |directory|
      anchor = Pathname(directory) / TRUST_ANCHOR_FILENAME
      anchor.binwrite(PROSPECTIVE_SEARCH_LEDGER_TRUST_ANCHOR.binread)
      validate_search_activation_transition(
        [anchor],
        event_name: "pull_request_target",
        data_loader: ->(_paths) { candidate }
      )
    end
  end

  def test_activation_trust_anchor_rejects_changed_staged_search_assets
    original = activation_candidate
    STAGED_SEARCH_ACTIVATION_SHA256.each_key do |relative|
      candidate = original.transform_values(&:dup)
      candidate[relative] << "\nmutated\n"
      error = nil
      Dir.mktmpdir do |directory|
        anchor = Pathname(directory) / TRUST_ANCHOR_FILENAME
        anchor.binwrite(PROSPECTIVE_SEARCH_LEDGER_TRUST_ANCHOR.binread)
        error = assert_raises(PolicyError) do
          validate_search_activation_transition(
            [anchor],
            event_name: "pull_request_target",
            data_loader: ->(_paths) { candidate }
          )
        end
      end
      assert_match(/preserve staged #{Regexp.escape(relative)} byte-for-byte/,
                   error.message)
    end
  end

  def test_activation_trust_anchor_rejects_changed_successor_executables
    original = activation_candidate
    exact_assets = STAGED_SEARCH_ACTIVATION_SHA256.keys
    (SEARCH_SUCCESSOR_EXECUTABLES - exact_assets).each do |relative|
      candidate = original.transform_values(&:dup)
      candidate[relative] << "\n# mutated\n"
      Dir.mktmpdir do |directory|
        anchor = Pathname(directory) / TRUST_ANCHOR_FILENAME
        anchor.binwrite(PROSPECTIVE_SEARCH_LEDGER_TRUST_ANCHOR.binread)
        error = assert_raises(PolicyError) do
          validate_search_activation_transition(
            [anchor],
            event_name: "pull_request_target",
            data_loader: ->(_paths) { candidate }
          )
        end
        message =
          /preserve trusted successor executable #{Regexp.escape(relative)}/
        assert_match(message, error.message)
      end
    end
  end

  def test_activation_trust_anchor_rejects_changed_or_missing_successor_inputs
    original = activation_candidate
    SEARCH_SUCCESSOR_INPUTS.each do |relative|
      candidates = {
        "changed" => original.transform_values(&:dup).tap do |candidate|
          candidate[relative] << "\nmutated\n"
        end,
        "missing" => original.transform_values(&:dup).tap do |candidate|
          candidate.delete(relative)
        end
      }
      candidates.each do |mutation, candidate|
        Dir.mktmpdir do |directory|
          anchor = Pathname(directory) / TRUST_ANCHOR_FILENAME
          anchor.binwrite(PROSPECTIVE_SEARCH_LEDGER_TRUST_ANCHOR.binread)
          error = assert_raises(PolicyError) do
            validate_search_activation_transition(
              [anchor],
              event_name: "pull_request_target",
              data_loader: ->(_paths) { candidate }
            )
          end
          message =
            /preserve trusted successor input #{Regexp.escape(relative)}/
          assert_match message, error.message, mutation
        end
      end
    end
  end

  def test_activation_trust_anchor_rejects_added_gitattributes_anywhere
    candidate = activation_candidate.merge(
      SEARCH_GIT_ATTRIBUTES_KEY => ".github/.gitattributes\0docs/.gitattributes".b
    )
    Dir.mktmpdir do |directory|
      anchor = Pathname(directory) / TRUST_ANCHOR_FILENAME
      anchor.binwrite(PROSPECTIVE_SEARCH_LEDGER_TRUST_ANCHOR.binread)
      error = assert_raises(PolicyError) do
        validate_search_activation_transition(
          [anchor],
          event_name: "pull_request_target",
          data_loader: ->(_paths) { candidate }
        )
      end
      assert_match(/cannot add checkout-affecting \.gitattributes/, error.message)
    end
  end

  def test_git_attributes_manifest_is_nul_safe_and_complete
    tree = [
      "README.md",
      "docs/.gitattributes",
      "directory\nwith-newline/.gitattributes",
      ".github/.gitattributes",
      ".gitattributes.txt"
    ].join("\0") + "\0"
    expected = [
      ".github/.gitattributes",
      "directory\nwith-newline/.gitattributes",
      "docs/.gitattributes"
    ].join("\0").b
    assert_equal expected, git_attributes_manifest(tree)
  end

  def test_activation_tree_metadata_is_nul_safe_and_preserves_modes
    tree = [
      "100644 blob #{'a' * 40}\t.github/workflows/ci.yml",
      "100644 blob #{'b' * 40}\tdirectory\nwith-newline/.gitattributes",
      "120000 blob #{'c' * 40}\tscripts/check_ci_permissions.rb"
    ].join("\0") + "\0"
    attributes, entries = activation_tree_metadata(tree)
    assert_equal "directory\nwith-newline/.gitattributes".b, attributes
    assert_equal "100644 blob", entries.fetch(".github/workflows/ci.yml")
    assert_equal "120000 blob", entries.fetch("scripts/check_ci_permissions.rb")
  end

  def test_activation_trust_anchor_rejects_changed_tree_entry_modes
    SEARCH_ACTIVATION_PATHS.each do |relative|
      candidate = activation_candidate
      candidate[SEARCH_TREE_ENTRIES_KEY] =
        SEARCH_ACTIVATION_TREE_ENTRIES.merge(relative => "120000 blob")
      Dir.mktmpdir do |directory|
        anchor = Pathname(directory) / TRUST_ANCHOR_FILENAME
        anchor.binwrite(PROSPECTIVE_SEARCH_LEDGER_TRUST_ANCHOR.binread)
        error = assert_raises(PolicyError) do
          validate_search_activation_transition(
            [anchor],
            event_name: "pull_request_target",
            data_loader: ->(_paths) { candidate }
          )
        end
        assert_match(/preserve Git tree entry 100644 blob for #{Regexp.escape(relative)}/,
                     error.message)
      end
    end
  end

  def test_activation_trust_anchor_rejects_changed_roadmap_projection
    candidate = activation_candidate
    roadmap = (Pathname(__dir__).parent / SEARCH_ROADMAP_PATH).binread
    mutations = [
      roadmap.sub("#{SEARCH_ROADMAP_CHECKPOINTS.first}\n", ""),
      roadmap.sub(SEARCH_ROADMAP_CHECKPOINTS.last,
                  SEARCH_ROADMAP_CHECKPOINTS.last.sub(" 130 ", " 129 ")),
      roadmap.sub(SEARCH_ROADMAP_CHECKPOINTS.last,
                  "#{SEARCH_ROADMAP_CHECKPOINTS.last}\n" \
                  "#{SEARCH_ROADMAP_CHECKPOINTS.first}")
    ]
    mutations.each do |mutated_roadmap|
      changed = candidate.merge(SEARCH_ROADMAP_PATH => mutated_roadmap)
      Dir.mktmpdir do |directory|
        anchor = Pathname(directory) / TRUST_ANCHOR_FILENAME
        anchor.binwrite(PROSPECTIVE_SEARCH_LEDGER_TRUST_ANCHOR.binread)
        error = assert_raises(PolicyError) do
          validate_search_activation_transition(
            [anchor],
            event_name: "pull_request_target",
            data_loader: ->(_paths) { changed }
          )
        end
        assert_match(/preserve the staged roadmap checkpoint projection/,
                     error.message)
      end
    end
  end

  def test_non_activation_event_does_not_fetch_staged_search_assets
    Dir.mktmpdir do |directory|
      anchor = Pathname(directory) / TRUST_ANCHOR_FILENAME
      anchor.binwrite(PROSPECTIVE_SEARCH_LEDGER_TRUST_ANCHOR.binread)
      validate_search_activation_transition(
        [anchor],
        event_name: "pull_request",
        data_loader: ->(_paths) { flunk "regular PR validation must not fetch" }
      )
    end
  end

  def test_activation_trust_anchor_rejects_changed_trusted_base_assets
    candidate = activation_candidate
    Dir.mktmpdir do |directory|
      root = Pathname(directory)
      anchor = root / TRUST_ANCHOR_FILENAME
      anchor.binwrite(PROSPECTIVE_SEARCH_LEDGER_TRUST_ANCHOR.binread)
      STAGED_SEARCH_ACTIVATION_SHA256.each_key do |relative|
        destination = root / relative
        FileUtils.mkdir_p(destination.dirname)
        destination.binwrite(candidate.fetch(relative))
      end
      changed = root / "docs/SEARCH_VISIBILITY.md"
      changed.binwrite(changed.binread + "\nmutated base\n")
      error = assert_raises(PolicyError) do
        validate_search_activation_transition(
          [anchor],
          event_name: "pull_request_target",
          repository_root: root,
          data_loader: ->(_paths) { candidate }
        )
      end
      assert_match(/disagrees with trusted base docs\/SEARCH_VISIBILITY.md/,
                   error.message)
    end
  end
end
