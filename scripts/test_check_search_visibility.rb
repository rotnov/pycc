#!/usr/bin/env ruby
# frozen_string_literal: true

# Test suite for scripts/check_search_visibility.rb — validates the
# structured external-mention evidence record and launch readiness gate
# (issue #209).

require "minitest/autorun"
require "json"
require "pathname"
require "rbconfig"
require "tempfile"

REPO_ROOT = Pathname(__dir__).parent
CHECKER = REPO_ROOT / "scripts" / "check_search_visibility.rb"
EVIDENCE = REPO_ROOT / "site" / "search-visibility" / "evidence.json"
DOCS = REPO_ROOT / "docs" / "SEARCH_VISIBILITY.md"

class TestCheckSearchVisibility < Minitest::Test
  def run_checker(evidence_text, docs_text)
    Dir.mktmpdir do |dir|
      root = Pathname(dir)
      ev_dir = root / "site" / "search-visibility"
      ev_dir.mkpath
      docs_dir = root / "docs"
      docs_dir.mkpath
      (ev_dir / "evidence.json").write(evidence_text)
      (docs_dir / "SEARCH_VISIBILITY.md").write(docs_text)
      output = `#{RbConfig.ruby} #{CHECKER} #{root} 2>&1`
      return $?.exitstatus, output
    end
  end

  def live_evidence
    EVIDENCE.read
  end

  def live_docs
    DOCS.read
  end

  # --- Positive: the live evidence passes ---

  def test_live_evidence_passes
    status, output = `#{RbConfig.ruby} #{CHECKER} #{REPO_ROOT} 2>&1`
    assert_equal 0, $?.exitstatus, "live evidence failed:\n#{output}"
  end

  def test_live_evidence_has_observations
    evidence = JSON.parse(live_evidence)
    assert_kind_of Array, evidence["observations"]
    refute_empty evidence["observations"]
  end

  def test_live_gate_is_closed
    evidence = JSON.parse(live_evidence)
    assert_equal "closed", evidence["launch_gate"]["status"]
  end

  # --- Negative: open gate without human authorization ---

  def test_rejects_open_gate_without_human_auth
    evidence = JSON.parse(live_evidence)
    evidence["launch_gate"]["status"] = "open"
    evidence["launch_gate"]["human_authorized"] = false
    status, = run_checker(JSON.generate(evidence), live_docs)
    refute_equal 0, status, "accepted open gate without human authorization"
  end

  # --- Negative: closed gate without reason ---

  def test_rejects_closed_gate_without_reason
    evidence = JSON.parse(live_evidence)
    evidence["launch_gate"].delete("closed_reason")
    status, = run_checker(JSON.generate(evidence), live_docs)
    refute_equal 0, status, "accepted closed gate without reason"
  end

  # --- Negative: self-authored reference classified as independent ---

  def test_rejects_self_authored_as_independent
    evidence = JSON.parse(live_evidence)
    # Find the GitHub issue search observation and change classification
    evidence["observations"].each do |obs|
      next unless obs["discovery_surface"] == "github_issue_search"
      obs["entries"].each do |entry|
        entry["classification"] = "independent_editorial"
      end
    end
    status, = run_checker(JSON.generate(evidence), live_docs)
    refute_equal 0, status, "accepted self-authored reference as independent"
  end

  # --- Negative: owned surface classified as independent ---

  def test_rejects_owned_surface_as_independent
    evidence = JSON.parse(live_evidence)
    evidence["observations"].each do |obs|
      next unless obs["discovery_surface"] == "github_traffic_referrers"
      obs["entries"].each do |entry|
        entry["classification"] = "independent_editorial"
      end
    end
    status, = run_checker(JSON.generate(evidence), live_docs)
    refute_equal 0, status, "accepted owned surface as independent"
  end

  # --- Negative: invalid classification ---

  def test_rejects_invalid_classification
    evidence = JSON.parse(live_evidence)
    evidence["observations"][4]["entries"][0]["classification"] = "viral_marketing"
    status, = run_checker(JSON.generate(evidence), live_docs)
    refute_equal 0, status, "accepted invalid classification"
  end

  # --- Negative: result_count mismatch ---

  def test_rejects_result_count_mismatch
    evidence = JSON.parse(live_evidence)
    evidence["observations"][4]["result_count"] = 99
    status, = run_checker(JSON.generate(evidence), live_docs)
    refute_equal 0, status, "accepted result_count mismatch"
  end

  # --- Negative: empty_window with entries ---

  def test_rejects_empty_window_with_entries
    evidence = JSON.parse(live_evidence)
    evidence["observations"][0]["entries"] = [
      { "classification" => "owned", "first_seen" => "2026-07-29", "last_verified" => "2026-07-29" }
    ]
    status, = run_checker(JSON.generate(evidence), live_docs)
    refute_equal 0, status, "accepted empty_window with entries"
  end

  # --- Negative: missing first_seen ---

  def test_rejects_missing_first_seen
    evidence = JSON.parse(live_evidence)
    evidence["observations"][4]["entries"][0].delete("first_seen")
    status, = run_checker(JSON.generate(evidence), live_docs)
    refute_equal 0, status, "accepted entry without first_seen"
  end

  # --- Negative: docs claiming zero backlinks ---

  def test_rejects_docs_claiming_zero_backlinks
    docs = live_docs + "\nThere are zero backlinks to pycc.\n"
    status, = run_checker(live_evidence, docs)
    refute_equal 0, status, "accepted docs claiming zero backlinks"
  end

  # --- Negative: docs describing pycc as AI compiler ---

  def test_rejects_docs_describing_ai_compiler
    docs = live_docs + "\npycc is an AI compiler.\n"
    status, = run_checker(live_evidence, docs)
    refute_equal 0, status, "accepted docs describing pycc as AI compiler"
  end

  # --- Negative: malformed JSON ---

  def test_rejects_malformed_evidence
    status, = run_checker("{not valid json", live_docs)
    refute_equal 0, status, "accepted malformed evidence JSON"
  end
end
