#!/usr/bin/env ruby
# frozen_string_literal: true

# Test suite for scripts/check_readme_testing_claims.rb — mutation tests
# that verify the validator catches contradictory current/planned testing
# claims and cross-checks them against actual repository evidence
# (issue #214).

require "minitest/autorun"
require "pathname"
require "rbconfig"
require "tmpdir"

REPO_ROOT = Pathname(__dir__).parent
CHECKER = REPO_ROOT / "scripts" / "check_readme_testing_claims.rb"
README_PATH = REPO_ROOT / "README.md"

class TestCheckReadmeTestingClaims < Minitest::Test
  # Run the checker against a synthetic repository root containing the
  # given README text and optional extra files (as a hash of relative
  # path => content).
  def run_checker(readme_text, extra_files = {})
    Dir.mktmpdir do |dir|
      root = Pathname(dir)
      (root / ".github" / "workflows").mkpath
      (root / "tests").mkpath
      (root / "README.md").write(readme_text)
      extra_files.each do |rel, content|
        path = root / rel
        path.parent.mkpath
        path.write(content)
      end
      output = `#{RbConfig.ruby} #{CHECKER} #{root} 2>&1`
      return $?.exitstatus, output
    end
  end

  def read_live_readme
    README_PATH.read
  end

  # A valid testing section matching the live README's structure.
  def valid_testing_section
    <<~MARKDOWN
      ## Testing strategy

      The complete internal test architecture has seven layers, defined in
      [`docs/TESTING.md`](./docs/TESTING.md). Three public compatibility mechanisms
      are especially important:

      1. **Conformance suite.** Every language standard pycc supports maps to a PEP, and every PEP has its own test in `tests/conformance/`. Each supported language level compiles and runs its cumulative fixture set, then compares output with that level's pinned CPython oracle.
      2. **Real-world corpus (planned).** CI would compile well-typed open-source projects and run their own test suites against the compiled artifacts. This is not yet implemented; no corpus workflow exists on current `main`.
      3. **Ecosystem bot (planned).** A scheduled job would pick popular PyPI/GitHub projects, compile them with the latest pycc, and auto-file a structured issue for every new incompatibility. This is not yet implemented; no scheduled bot workflow exists on current `main`.

      ## Roadmap

      - [x] MVP
      - [ ] Ecosystem bot: compile top PyPI packages nightly, auto-file incompatibility issues
    MARKDOWN
  end

  # --- Positive: the live README passes (with live evidence files) ---

  def test_live_readme_passes
    extra = {
      "tests/conformance.rs" => (REPO_ROOT / "tests" / "conformance.rs").read,
      ".github/workflows/ci.yml" => (REPO_ROOT / ".github" / "workflows" / "ci.yml").read,
    }
    status, output = run_checker(read_live_readme, extra)
    assert_equal 0, status, "live README failed:\n#{output}"
  end

  # --- Positive: a valid synthetic README with evidence passes ---

  def test_valid_readme_with_evidence_passes
    extra = {
      "tests/conformance.rs" => "// conformance test\n",
      ".github/workflows/ci.yml" => "name: CI\non: [push]\njobs:\n  build:\n    runs-on: ubuntu-latest\n    steps:\n      - run: cargo test --conformance\n",
    }
    status, output = run_checker(valid_testing_section, extra)
    assert_equal 0, status, "valid README with evidence failed:\n#{output}"
  end

  # --- Negative: missing testing section ---

  def test_rejects_missing_testing_section
    readme = "# pycc\n\n## Roadmap\n\n- [x] MVP\n"
    status, = run_checker(readme)
    refute_equal 0, status, "accepted missing testing section"
  end

  # --- Negative: current mechanism without evidence ---

  def test_rejects_conformance_claimed_current_without_evidence
    readme = valid_testing_section
    # No tests/conformance.rs, no workflow mentioning conformance
    status, output = run_checker(readme, {})
    refute_equal 0, status, "accepted conformance as current with no evidence"
    assert_match(/conformance.*current.*no backing evidence/i, output)
  end

  # --- Negative: present-tense corpus claim without planned marker ---

  def test_rejects_present_tense_corpus_without_planned_marker
    readme = <<~MARKDOWN
      ## Testing strategy

      1. **Conformance suite.** Tests in `tests/conformance/`.
      2. **Real-world corpus.** CI compiles well-typed open-source projects and runs their own test suites.

      ## Roadmap

      - [x] MVP
      - [ ] Ecosystem bot
    MARKDOWN
    extra = { "tests/conformance.rs" => "//\n" }
    status, output = run_checker(readme, extra)
    refute_equal 0, status, "accepted present-tense corpus claim without planned marker"
    assert_match(/present-tense claim for.*corpus.*without.*planned marker/i, output)
  end

  # --- Negative: present-tense bot claim without planned marker ---

  def test_rejects_present_tense_bot_without_planned_marker
    readme = <<~MARKDOWN
      ## Testing strategy

      1. **Conformance suite.** Tests in `tests/conformance/`.
      2. **Ecosystem bot.** A scheduled job picks popular PyPI projects and auto-files issues.

      ## Roadmap

      - [x] MVP
      - [ ] Ecosystem bot
    MARKDOWN
    extra = { "tests/conformance.rs" => "//\n" }
    status, output = run_checker(readme, extra)
    refute_equal 0, status, "accepted present-tense bot claim without planned marker"
    assert_match(/present-tense claim for.*ecosystem bot.*without.*planned marker/i, output)
  end

  # --- Negative: present-tense fuzzing claim without planned marker ---

  def test_rejects_present_tense_fuzzing_without_planned_marker
    readme = <<~MARKDOWN
      ## Testing strategy

      1. **Conformance suite.** Tests in `tests/conformance/`.
      2. **Differential fuzzing.** Generator produces well-typed programs. Runs continuously on a dedicated runner.

      ## Roadmap

      - [x] MVP
      - [ ] Ecosystem bot
    MARKDOWN
    extra = { "tests/conformance.rs" => "//\n" }
    status, output = run_checker(readme, extra)
    refute_equal 0, status, "accepted present-tense fuzzing claim without planned marker"
    assert_match(/present-tense claim for.*fuzzing.*without.*planned marker/i, output)
  end

  # --- Negative: corpus mentioned but never marked planned ---

  def test_rejects_corpus_mentioned_without_any_planned_marker
    readme = <<~MARKDOWN
      ## Testing strategy

      1. **Conformance suite.** Tests in `tests/conformance/`.
      2. **Real-world corpus.** A tiered approach to testing open-source projects.

      ## Roadmap

      - [x] MVP
      - [ ] Ecosystem bot
    MARKDOWN
    extra = { "tests/conformance.rs" => "//\n" }
    status, output = run_checker(readme, extra)
    refute_equal 0, status, "accepted corpus mention with no planned marker at all"
    assert_match(/corpus.*never marks.*planned/i, output)
  end

  # --- Negative: bot mentioned but never marked planned ---

  def test_rejects_bot_mentioned_without_any_planned_marker
    readme = <<~MARKDOWN
      ## Testing strategy

      1. **Conformance suite.** Tests in `tests/conformance/`.
      2. **Ecosystem bot.** Files issues automatically for incompatibilities.

      ## Roadmap

      - [x] MVP
      - [ ] Ecosystem bot
    MARKDOWN
    extra = { "tests/conformance.rs" => "//\n" }
    status, output = run_checker(readme, extra)
    refute_equal 0, status, "accepted bot mention with no planned marker at all"
    assert_match(/ecosystem bot.*never marks.*planned/i, output)
  end

  # --- Positive: planned markers make present-tense-adjacent text OK ---

  def test_planned_corpus_with_marker_passes
    readme = <<~MARKDOWN
      ## Testing strategy

      1. **Conformance suite.** Tests in `tests/conformance/`.
      2. **Real-world corpus (planned).** CI would compile open-source projects. Not yet implemented.

      ## Roadmap

      - [x] MVP
      - [ ] Ecosystem bot
    MARKDOWN
    extra = { "tests/conformance.rs" => "//\n" }
    status, = run_checker(readme, extra)
    assert_equal 0, status, "planned corpus with marker was rejected"
  end

  def test_planned_bot_with_marker_passes
    readme = <<~MARKDOWN
      ## Testing strategy

      1. **Conformance suite.** Tests in `tests/conformance/`.
      2. **Ecosystem bot (planned).** A scheduled job would pick popular projects. Not yet implemented.

      ## Roadmap

      - [x] MVP
      - [ ] Ecosystem bot
    MARKDOWN
    extra = { "tests/conformance.rs" => "//\n" }
    status, = run_checker(readme, extra)
    assert_equal 0, status, "planned bot with marker was rejected"
  end

  # --- Negative: roadmap inconsistency — bot checked but testing says planned ---

  def test_rejects_roadmap_bot_checked_but_testing_says_planned
    readme = <<~MARKDOWN
      ## Testing strategy

      1. **Conformance suite.** Tests in `tests/conformance/`.
      2. **Ecosystem bot (planned).** Not yet implemented.

      ## Roadmap

      - [x] MVP
      - [x] Ecosystem bot: compile top PyPI packages nightly
    MARKDOWN
    extra = { "tests/conformance.rs" => "//\n" }
    status, output = run_checker(readme, extra)
    refute_equal 0, status, "accepted roadmap bot checked while testing says planned"
    assert_match(/ecosystem bot.*inconsistent/i, output)
  end

  # --- Positive: no ecosystem bot in testing section — roadmap check skipped ---

  def test_no_bot_in_testing_section_passes
    readme = <<~MARKDOWN
      ## Testing strategy

      1. **Conformance suite.** Tests in `tests/conformance/`.

      ## Roadmap

      - [x] MVP
      - [ ] Ecosystem bot
    MARKDOWN
    extra = { "tests/conformance.rs" => "//\n" }
    status, = run_checker(readme, extra)
    assert_equal 0, status, "README without bot mention was rejected"
  end

  # --- Negative: missing README ---

  def test_rejects_missing_readme
    Dir.mktmpdir do |dir|
      output = `#{RbConfig.ruby} #{CHECKER} #{dir} 2>&1`
      assert $?.exitstatus != 0, "accepted missing README.md"
    end
  end

  # --- Negative: conformance claimed current but evidence file removed ---

  def test_rejects_conformance_current_when_evidence_path_missing
    # README claims conformance is current, but no conformance.rs and no
    # workflow mentioning conformance.
    readme = valid_testing_section
    status, output = run_checker(readme, {
      ".github/workflows/ci.yml" => "name: CI\non: [push]\n",
    })
    refute_equal 0, status, "accepted conformance as current with no evidence file"
    assert_match(/conformance.*current.*no backing evidence/i, output)
  end

  # --- Positive: conformance current with workflow grep evidence ---

  def test_conformance_current_with_workflow_grep_evidence
    readme = valid_testing_section
    extra = {
      ".github/workflows/ci.yml" => "name: CI\non: [push]\njobs:\n  build:\n    steps:\n      - run: cargo test --conformance\n",
    }
    status, = run_checker(readme, extra)
    assert_equal 0, status, "conformance with workflow grep evidence was rejected"
  end

  # --- Negative: present-tense pass-rate claim without planned marker ---

  def test_rejects_pass_rate_tracked_without_planned_marker
    readme = <<~MARKDOWN
      ## Testing strategy

      1. **Conformance suite.** Tests in `tests/conformance/`.
      2. **Real-world corpus.** Pass rate per project is tracked release to release.

      ## Roadmap

      - [x] MVP
      - [ ] Ecosystem bot
    MARKDOWN
    extra = { "tests/conformance.rs" => "//\n" }
    status, output = run_checker(readme, extra)
    refute_equal 0, status, "accepted 'pass rate is tracked' without planned marker"
  end

  # --- Negative: nightly bot claim without planned marker ---

  def test_rejects_nightly_bot_without_planned_marker
    readme = <<~MARKDOWN
      ## Testing strategy

      1. **Conformance suite.** Tests in `tests/conformance/`.
      2. **Ecosystem bot.** A nightly bot compiles top PyPI packages.

      ## Roadmap

      - [x] MVP
      - [ ] Ecosystem bot
    MARKDOWN
    extra = { "tests/conformance.rs" => "//\n" }
    status, output = run_checker(readme, extra)
    refute_equal 0, status, "accepted 'nightly bot' without planned marker"
  end

  # --- Positive: validator is present in pages.yml ---

  def test_live_pages_yml_contains_validator
    pages = (REPO_ROOT / ".github" / "workflows" / "pages.yml").read
    assert_match(/check_readme_testing_claims\.rb/, pages,
                 "pages.yml must run the README testing-claims validator")
  end

  def test_live_pages_yml_contains_validator_tests
    pages = (REPO_ROOT / ".github" / "workflows" / "pages.yml").read
    assert_match(/test_check_readme_testing_claims\.rb/, pages,
                 "pages.yml must run the README testing-claims validator tests")
  end
end
