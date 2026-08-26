#!/usr/bin/env ruby
# frozen_string_literal: true

# Test suite for scripts/check_markdown_landing.rb — validates that the
# Markdown landing page (site/index.html.md) stays a faithful projection of
# the HTML landing page (site/index.html) so one-sided semantic drift is
# detected (issue #206).

require "minitest/autorun"
require "pathname"
require "rbconfig"
require "tempfile"

REPO_ROOT = Pathname(__dir__).parent
CHECKER = REPO_ROOT / "scripts" / "check_markdown_landing.rb"

class TestCheckMarkdownLanding < Minitest::Test
  # Run the checker against a temporary repository root containing the given
  # HTML and Markdown text. Returns [exit_status, combined_output].
  def run_checker(html_text, markdown_text)
    Dir.mktmpdir do |dir|
      root = Pathname(dir)
      site_dir = root / "site"
      site_dir.mkpath
      (site_dir / "index.html").write(html_text)
      (site_dir / "index.html.md").write(markdown_text)
      output = `#{RbConfig.ruby} #{CHECKER} #{root} 2>&1`
      return $?.exitstatus, output
    end
  end

  def live_html
    (REPO_ROOT / "site" / "index.html").read
  end

  def live_markdown
    (REPO_ROOT / "site" / "index.html.md").read
  end

  # --- Positive: the live files pass ---

  def test_live_files_pass
    output = `#{RbConfig.ruby} #{CHECKER} #{REPO_ROOT} 2>&1`
    assert_equal 0, $?.exitstatus, "live Markdown landing failed:\n#{output}"
  end

  # --- Negative: missing HTML anchor (contract drift on the HTML side) ---

  def test_rejects_missing_html_anchor
    # Remove a visible section heading from the HTML so the contract anchor
    # no longer matches the live page.
    html = live_html.sub("One tool should finish the whole job.", "")
    status, output = run_checker(html, live_markdown)
    refute_equal 0, status, "accepted HTML missing a contracted anchor:\n#{output}"
    assert_includes output, "HTML anchor"
  end

  # --- Negative: Markdown drops a section ---

  def test_rejects_markdown_missing_design_contract
    md = live_markdown.sub("## Design contract\n", "## Removed\n")
    status, output = run_checker(live_html, md)
    refute_equal 0, status, "accepted Markdown without design contract:\n#{output}"
  end

  def test_rejects_markdown_missing_why_pycc
    md = live_markdown.sub("## Why pycc\n", "## Removed\n")
    status, output = run_checker(live_html, md)
    refute_equal 0, status, "accepted Markdown without why-pycc section:\n#{output}"
  end

  def test_rejects_markdown_missing_comparison
    md = live_markdown.sub("## The intended position\n", "## Removed\n")
    status, output = run_checker(live_html, md)
    refute_equal 0, status, "accepted Markdown without comparison section:\n#{output}"
  end

  def test_rejects_markdown_missing_pipeline
    md = live_markdown.sub("## Compiler pipeline\n", "## Removed\n")
    status, output = run_checker(live_html, md)
    refute_equal 0, status, "accepted Markdown without pipeline section:\n#{output}"
  end

  def test_rejects_markdown_missing_final_cta
    md = live_markdown.sub("## Follow the project\n", "## Removed\n")
    status, output = run_checker(live_html, md)
    refute_equal 0, status, "accepted Markdown without final CTA:\n#{output}"
  end

  # --- Negative: Markdown drops a specific marker ---

  def test_rejects_markdown_missing_contract_pillar
    md = live_markdown.sub("Standard Python target", "Removed target")
    status, output = run_checker(live_html, md)
    refute_equal 0, status, "accepted Markdown without a contract pillar:\n#{output}"
  end

  def test_rejects_markdown_missing_comparison_tool
    md = live_markdown.gsub("LPython", "RemovedTool")
    status, output = run_checker(live_html, md)
    refute_equal 0, status, "accepted Markdown without a comparison tool:\n#{output}"
  end

  def test_rejects_markdown_missing_pipeline_step
    md = live_markdown.sub("From source file to machine code.", "Removed step.")
    status, output = run_checker(live_html, md)
    refute_equal 0, status, "accepted Markdown without a pipeline heading:\n#{output}"
  end

  def test_rejects_missing_v0_4_unstarted_claim
    # Removing "is unstarted" leaves the "Honest status" bullet claiming only
    # that v0.4 is next, silently dropping the fact that it has not started
    # yet — the exact one-sided drift issue #206's contract exists to catch.
    md = live_markdown.sub(
      "v0.4 (multi-file projects, imports, incremental compilation) is unstarted",
      "v0.4 (multi-file projects, imports, incremental compilation) is next"
    )
    status, output = run_checker(live_html, md)
    refute_equal 0, status, "accepted Markdown that dropped the v0.4-unstarted claim:\n#{output}"
  end

  # --- Negative: wrong title ---

  def test_rejects_wrong_title
    md = live_markdown.sub(
      "# pycc — AOT compiler for typed Python to native binaries",
      "# pycc — a different title"
    )
    status, output = run_checker(live_html, md)
    refute_equal 0, status, "accepted Markdown with wrong title:\n#{output}"
  end

  # --- Negative: production-ready claim ---

  def test_rejects_production_ready_claim
    md = live_markdown.sub(
      "pycc is pre-alpha and is not ready for production.",
      "pycc is pre-alpha and is not ready for production. It is production-ready."
    )
    status, output = run_checker(live_html, md)
    refute_equal 0, status, "accepted Markdown with production-ready claim:\n#{output}"
  end

  # --- Negative: missing files ---

  def test_rejects_missing_html
    Dir.mktmpdir do |dir|
      root = Pathname(dir)
      (root / "site").mkpath
      (root / "site" / "index.html.md").write(live_markdown)
      output = `#{RbConfig.ruby} #{CHECKER} #{root} 2>&1`
      refute_equal 0, $?.exitstatus, "accepted missing HTML:\n#{output}"
    end
  end

  def test_rejects_missing_markdown
    Dir.mktmpdir do |dir|
      root = Pathname(dir)
      (root / "site").mkpath
      (root / "site" / "index.html").write(live_html)
      output = `#{RbConfig.ruby} #{CHECKER} #{root} 2>&1`
      refute_equal 0, $?.exitstatus, "accepted missing Markdown:\n#{output}"
    end
  end
end
