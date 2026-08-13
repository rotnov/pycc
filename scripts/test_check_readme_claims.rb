#!/usr/bin/env ruby
# frozen_string_literal: true

# Test suite for scripts/check_readme_claims.rb — validates README.md
# testing-strategy claims against issue #214's requirements with
# positive and negative controls.

require "minitest/autorun"
require "pathname"
require "rbconfig"
require "tmpdir"

REPO_ROOT = Pathname(__dir__).parent
CHECKER = REPO_ROOT / "scripts" / "check_readme_claims.rb"
README_PATH = REPO_ROOT / "README.md"

class TestCheckReadmeClaims < Minitest::Test
  def run_checker(readme_text)
    Dir.mktmpdir do |dir|
      root = Pathname(dir)
      (root / "README.md").write(readme_text)
      output = `#{RbConfig.ruby} #{CHECKER} #{root} 2>&1`
      return $?.exitstatus, output
    end
  end

  def read_live_readme
    README_PATH.read
  end

  # A minimal valid README with the testing section using planned tense
  VALID_TESTING_SECTION = <<~MARKDOWN.freeze

    ## Testing strategy

    The complete internal test architecture has seven layers, defined in
    [`docs/TESTING.md`](./docs/TESTING.md). Three public compatibility mechanisms
    are especially important:

    1. **Conformance suite.** Every language standard pycc supports maps to a PEP, and every PEP has its own test in `tests/conformance/`. Each supported language level compiles and runs its cumulative fixture set, then compares output with that level's pinned CPython oracle.
    2. **Real-world corpus (planned).** CI would compile well-typed open-source projects and run their own test suites against the compiled artifacts. This is not yet implemented; no corpus workflow exists on current `main`.
    3. **Ecosystem bot (planned).** A scheduled job would pick popular PyPI/GitHub projects, compile them with the latest pycc, and auto-file a structured issue for every new incompatibility. This is not yet implemented; no scheduled bot workflow exists on current `main`.

    ## Roadmap

    - [x] MVP
    - [ ] Ecosystem bot: compile top PyPI packages nightly
  MARKDOWN

  # --- Positive: the live README passes ---

  def test_live_readme_passes
    status, output = run_checker(read_live_readme)
    assert_equal 0, status, "live README failed:\n#{output}"
  end

  # --- Positive: a minimal valid README passes ---

  def test_minimal_valid_readme_passes
    readme = "# pycc\n\n## Testing strategy\n\nConformance suite is active.\n\n## Roadmap\n\n- [x] MVP\n"
    status, = run_checker(readme)
    assert_equal 0, status, "minimal valid README failed"
  end

  # --- Negative: missing testing section ---

  def test_rejects_missing_testing_section
    readme = "# pycc\n\n## Roadmap\n\n- [x] MVP\n"
    status, = run_checker(readme)
    refute_equal 0, status, "accepted missing testing section"
  end

  # --- Negative: present-tense corpus claim ---

  def test_rejects_present_tense_ci_compiles
    readme = "# pycc\n\n## Testing strategy\n\nCI compiles well-typed open-source projects and runs their own test suites.\n\n## Roadmap\n\n- [x] MVP\n"
    status, = run_checker(readme)
    refute_equal 0, status, "accepted present-tense 'CI compiles' corpus claim"
  end

  def test_rejects_present_tense_pass_rate_tracked
    readme = "# pycc\n\n## Testing strategy\n\nPass rate per project is tracked release to release.\n\n## Roadmap\n\n- [x] MVP\n"
    status, = run_checker(readme)
    refute_equal 0, status, "accepted present-tense 'pass rate is tracked' claim"
  end

  # --- Negative: present-tense ecosystem bot claim ---

  def test_rejects_present_tense_scheduled_job
    readme = "# pycc\n\n## Testing strategy\n\nA scheduled job picks popular PyPI/GitHub projects and auto-files issues.\n\n## Roadmap\n\n- [x] MVP\n"
    status, = run_checker(readme)
    refute_equal 0, status, "accepted present-tense 'scheduled job picks' bot claim"
  end

  def test_rejects_nightly_bot_claim
    readme = "# pycc\n\n## Testing strategy\n\nA nightly bot compiles top PyPI packages.\n\n## Roadmap\n\n- [x] MVP\n"
    status, = run_checker(readme)
    refute_equal 0, status, "accepted 'nightly bot' claim"
  end

  # --- Negative: present-tense differential fuzzing claim ---

  def test_rejects_present_tense_fuzzing
    readme = "# pycc\n\n## Testing strategy\n\nGenerator produces well-typed programs. Runs continuously on a dedicated runner.\n\n## Roadmap\n\n- [x] MVP\n"
    status, = run_checker(readme)
    refute_equal 0, status, "accepted present-tense differential fuzzing claim"
  end

  def test_rejects_runs_continuously
    readme = "# pycc\n\n## Testing strategy\n\nDifferential fuzzing runs continuously.\n\n## Roadmap\n\n- [x] MVP\n"
    status, = run_checker(readme)
    refute_equal 0, status, "accepted 'runs continuously' fuzzing claim"
  end

  # --- Positive: planned markers make the same claims acceptable ---

  def test_planned_corpus_claim_passes
    readme = "# pycc\n\n## Testing strategy\n\n**Real-world corpus (planned).** CI would compile well-typed open-source projects. This is not yet implemented.\n\n## Roadmap\n\n- [x] MVP\n"
    status, = run_checker(readme)
    assert_equal 0, status, "planned corpus claim was rejected"
  end

  def test_planned_bot_claim_passes
    readme = "# pycc\n\n## Testing strategy\n\n**Ecosystem bot (planned).** A scheduled job would pick popular projects. Not yet implemented.\n\n## Roadmap\n\n- [x] MVP\n"
    status, = run_checker(readme)
    assert_equal 0, status, "planned bot claim was rejected"
  end

  # --- Negative: corpus mentioned without planned marker ---

  def test_rejects_corpus_without_planned_marker
    readme = "# pycc\n\n## Testing strategy\n\nThe corpus testing layer compiles open-source projects.\n\n## Roadmap\n\n- [x] MVP\n"
    status, = run_checker(readme)
    refute_equal 0, status, "accepted corpus mention without planned marker"
  end

  def test_rejects_bot_without_planned_marker
    readme = "# pycc\n\n## Testing strategy\n\nThe ecosystem bot files issues automatically.\n\n## Roadmap\n\n- [x] MVP\n"
    status, = run_checker(readme)
    refute_equal 0, status, "accepted bot mention without planned marker"
  end

  # --- Missing file ---

  def test_rejects_missing_readme
    Dir.mktmpdir do |dir|
      output = `#{RbConfig.ruby} #{CHECKER} #{dir} 2>&1`
      assert $?.exitstatus != 0, "accepted missing README.md"
    end
  end
end
