#!/usr/bin/env ruby
# frozen_string_literal: true

# Test suite for scripts/check_source_links_registry.rb — validates
# that the source link registry covers every URL in claims.json
# (issue #202).

require "minitest/autorun"
require "json"
require "pathname"
require "rbconfig"
require "set"
require "tempfile"

REPO_ROOT = Pathname(__dir__).parent
CHECKER = REPO_ROOT / "scripts" / "check_source_links_registry.rb"
CLAIMS = REPO_ROOT / "site" / "python-aot-compilers" / "claims.json"
REGISTRY = REPO_ROOT / "site" / "python-aot-compilers" / "source-link-registry.json"

class TestCheckSourceLinksRegistry < Minitest::Test
  def run_checker(claims_text, registry_text)
    Dir.mktmpdir do |dir|
      root = Pathname(dir)
      claims_dir = root / "site" / "python-aot-compilers"
      claims_dir.mkpath
      (claims_dir / "claims.json").write(claims_text)
      (claims_dir / "source-link-registry.json").write(registry_text)
      output = `#{RbConfig.ruby} #{CHECKER} #{root} 2>&1`
      return $?.exitstatus, output
    end
  end

  def live_claims
    CLAIMS.read
  end

  def live_registry
    REGISTRY.read
  end

  # --- Positive: the live registry passes ---

  def test_live_registry_passes
    status, output = `#{RbConfig.ruby} #{CHECKER} #{REPO_ROOT} 2>&1`
    assert_equal 0, $?.exitstatus, "live registry failed:\n#{output}"
  end

  def test_live_registry_has_entries
    registry = JSON.parse(live_registry)
    assert_kind_of Array, registry["entries"]
    refute_empty registry["entries"]
  end

  def test_live_registry_covers_all_claims_urls
    claims = JSON.parse(live_claims)
    registry = JSON.parse(live_registry)
    claims_urls = Set.new
    claims["entities"].each do |entity|
      entity.each_value do |value|
        case value
        when Array
          value.each do |item|
            if item.is_a?(Hash) && item["url"]
              claims_urls.add(item["url"])
            end
          end
        when Hash
          (value["sources"] || []).each do |src|
            if src.is_a?(Hash) && src["url"]
              claims_urls.add(src["url"])
            end
          end
        end
      end
    end
    registry_urls = registry["entries"].map { |e| e["url"] }.to_set
    assert_equal claims_urls, registry_urls,
                "registry URLs must exactly match claims.json URLs"
  end

  # --- Negative: missing registry entry ---

  def test_rejects_missing_registry_entry
    registry = JSON.parse(live_registry)
    registry["entries"] = registry["entries"].drop(1)
    status, = run_checker(live_claims, JSON.generate(registry))
    refute_equal 0, status, "accepted registry with missing entry"
  end

  # --- Negative: broken status ---

  def test_rejects_broken_status
    registry = JSON.parse(live_registry)
    registry["entries"][0]["status"] = "broken"
    status, = run_checker(live_claims, JSON.generate(registry))
    refute_equal 0, status, "accepted registry with broken status"
  end

  # --- Negative: extra registry entry ---

  def test_rejects_extra_registry_entry
    registry = JSON.parse(live_registry)
    registry["entries"] << {
      "url" => "https://example.com/extra",
      "last_checked" => "2026-08-12",
      "status" => "ok",
      "http_status" => 200,
    }
    status, = run_checker(live_claims, JSON.generate(registry))
    refute_equal 0, status, "accepted registry with extra entry"
  end

  # --- Negative: invalid status ---

  def test_rejects_invalid_status
    registry = JSON.parse(live_registry)
    registry["entries"][0]["status"] = "unknown"
    status, = run_checker(live_claims, JSON.generate(registry))
    refute_equal 0, status, "accepted registry with invalid status"
  end

  # --- Negative: missing last_checked ---

  def test_rejects_missing_last_checked
    registry = JSON.parse(live_registry)
    registry["entries"][0].delete("last_checked")
    status, = run_checker(live_claims, JSON.generate(registry))
    refute_equal 0, status, "accepted registry entry without last_checked"
  end

  # --- Negative: malformed JSON ---

  def test_rejects_malformed_registry
    status, = run_checker(live_claims, "{not valid json")
    refute_equal 0, status, "accepted malformed registry JSON"
  end
end
