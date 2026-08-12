#!/usr/bin/env ruby
# frozen_string_literal: true

# Test suite for scripts/check_citation_cff.rb — validates CITATION.cff
# against issue #212's requirements with positive and negative controls.

require "fileutils"
require "minitest/autorun"
require "pathname"
require "rbconfig"
require "tmpdir"
require "yaml"

REPO_ROOT = Pathname(__dir__).parent
CHECKER = REPO_ROOT / "scripts" / "check_citation_cff.rb"
CITATION_PATH = REPO_ROOT / "CITATION.cff"

class TestCheckCitationCff < Minitest::Test
  def run_checker(cff_text, extra_files = {})
    Dir.mktmpdir do |dir|
      root = Pathname(dir)
      (root / "CITATION.cff").write(cff_text)
      extra_files.each do |rel_path, content|
        path = root / rel_path
        path.dirname.mkpath
        path.write(content)
      end
      output = `#{RbConfig.ruby} #{CHECKER} #{root} 2>&1`
      return $?.exitstatus, output
    end
  end

  def read_live_citation
    CITATION_PATH.read
  end

  # --- Positive: the live CITATION.cff passes ---

  def test_live_citation_passes
    status, output = run_checker(read_live_citation)
    assert_equal 0, status, "live CITATION.cff failed:\n#{output}"
  end

  # --- Required field mutations ---

  def test_rejects_missing_cff_version
    text = read_live_citation.gsub(/^cff-version:.*$/, "")
    status, = run_checker(text)
    refute_equal 0, status, "accepted missing cff-version"
  end

  def test_rejects_wrong_cff_version
    text = read_live_citation.sub("1.2.0", "1.1.0")
    status, = run_checker(text)
    refute_equal 0, status, "accepted wrong cff-version"
  end

  def test_rejects_missing_message
    text = read_live_citation.gsub(/^message:.*$/, "")
    status, = run_checker(text)
    refute_equal 0, status, "accepted missing message"
  end

  def test_rejects_missing_title
    text = read_live_citation.gsub(/^title:.*$/, "")
    status, = run_checker(text)
    refute_equal 0, status, "accepted missing title"
  end

  def test_rejects_missing_authors
    text = read_live_citation.gsub(/^authors:$/, "").gsub(/^  - name:.*$/, "").gsub(/^    description:.*$/, "").gsub(/^      .*$/, "")
    status, = run_checker(text)
    refute_equal 0, status, "accepted missing authors"
  end

  def test_rejects_empty_authors
    text = read_live_citation.sub(/^authors:.*$/, "authors: []")
    status, = run_checker(text)
    refute_equal 0, status, "accepted empty authors"
  end

  def test_rejects_missing_type
    text = read_live_citation.gsub(/^type:.*$/, "")
    status, = run_checker(text)
    refute_equal 0, status, "accepted missing type"
  end

  def test_rejects_wrong_type
    text = read_live_citation.sub("type: software", "type: dataset")
    status, = run_checker(text)
    refute_equal 0, status, "accepted wrong type"
  end

  # --- Repository identity mutations ---

  def test_rejects_wrong_repository_url
    text = read_live_citation.sub(
      "https://github.com/rotnov/pycc",
      "https://github.com/other/pycc"
    )
    status, = run_checker(text)
    refute_equal 0, status, "accepted wrong repository URL"
  end

  def test_rejects_bare_product_name_repository
    text = read_live_citation.sub(
      "https://github.com/rotnov/pycc",
      "https://github.com/pycc/pycc"
    )
    status, = run_checker(text)
    refute_equal 0, status, "accepted bare product name repository"
  end

  # --- URL mutations ---

  def test_rejects_wrong_landing_url
    text = read_live_citation.sub(
      "https://rotnov.github.io/pycc/",
      "https://example.com/pycc/"
    )
    status, = run_checker(text)
    refute_equal 0, status, "accepted wrong landing URL"
  end

  # --- License mutations ---

  def test_rejects_non_mit_license
    text = read_live_citation.sub("license: MIT", "license: Apache-2.0")
    status, = run_checker(text)
    refute_equal 0, status, "accepted non-MIT license"
  end

  # --- Product semantics mutations ---

  def test_rejects_ai_compiler_in_title
    text = read_live_citation.sub(
      "pycc: ahead-of-time compiler for typed Python",
      "pycc: AI compiler for typed Python"
    )
    status, = run_checker(text)
    refute_equal 0, status, "accepted 'AI compiler' in title"
  end

  def test_rejects_ai_compiler_in_abstract
    text = read_live_citation.sub(
      "not an AI or machine-learning compiler",
      "an AI compiler for Python"
    )
    status, = run_checker(text)
    refute_equal 0, status, "accepted 'AI compiler' in abstract"
  end

  def test_rejects_machine_learning_in_keywords
    text = read_live_citation.sub(
      "  - static typing",
      "  - machine learning"
    )
    status, = run_checker(text)
    refute_equal 0, status, "accepted 'machine learning' in keywords"
  end

  # --- Authorship mutations ---

  def test_rejects_human_author_with_given_names
    text = read_live_citation.sub(
      "  - name: \"pycc AI agents\"",
      "  - name: \"Denys Rotnov\"\n    given-names: Denys\n    family-names: Rotnov"
    )
    status, = run_checker(text)
    refute_equal 0, status, "accepted human author with given-names"
  end

  def test_rejects_author_without_description
    # Remove the description field from the author entry
    text = read_live_citation.gsub(/^    description: >-\n.*?\n(?=\n|$)/m, "")
    status, = run_checker(text)
    refute_equal 0, status, "accepted author without description"
  end

  # --- Release-bound field mutations ---

  def test_rejects_version_field
    text = read_live_citation + "version: 0.1.0\n"
    status, = run_checker(text)
    refute_equal 0, status, "accepted version field"
  end

  def test_rejects_date_released_field
    text = read_live_citation + "date-released: 2026-01-01\n"
    status, = run_checker(text)
    refute_equal 0, status, "accepted date-released field"
  end

  def test_rejects_commit_field
    text = read_live_citation + "commit: abc123\n"
    status, = run_checker(text)
    refute_equal 0, status, "accepted commit field"
  end

  def test_rejects_doi_field
    text = read_live_citation + "doi: 10.0000/test\n"
    status, = run_checker(text)
    refute_equal 0, status, "accepted doi field"
  end

  def test_rejects_preferred_citation_field
    text = read_live_citation + "preferred-citation:\n  type: article\n"
    status, = run_checker(text)
    refute_equal 0, status, "accepted preferred-citation field"
  end

  # --- Zenodo mutation ---

  def test_rejects_zenodo_json
    status, = run_checker(read_live_citation, { ".zenodo.json" => "{}" })
    refute_equal 0, status, "accepted .zenodo.json presence"
  end

  # --- Invalid YAML ---

  def test_rejects_invalid_yaml
    status, = run_checker("cff-version: 1.2.0\n  bad: : :\n")
    refute_equal 0, status, "accepted invalid YAML"
  end

  # --- Missing file ---

  def test_rejects_missing_citation_file
    Dir.mktmpdir do |dir|
      output = `#{RbConfig.ruby} #{CHECKER} #{dir} 2>&1`
      assert $?.exitstatus != 0, "accepted missing CITATION.cff"
    end
  end
end
