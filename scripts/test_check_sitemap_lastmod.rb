#!/usr/bin/env ruby
# frozen_string_literal: true

# Test suite for scripts/check_sitemap_lastmod.rb — validates that
# sitemap lastmod dates are bound to the git history of each page's
# source file (issue #201).

require "minitest/autorun"
require "pathname"
require "rbconfig"
require "tempfile"

REPO_ROOT = Pathname(__dir__).parent
CHECKER = REPO_ROOT / "scripts" / "check_sitemap_lastmod.rb"

class TestCheckSitemapLastmod < Minitest::Test
  def run_checker(sitemap_text)
    Dir.mktmpdir do |dir|
      root = Pathname(dir)
      site_dir = root / "site"
      site_dir.mkpath
      (site_dir / "sitemap.xml").write(sitemap_text)
      # Create minimal source files so they exist.
      (site_dir / "index.html").write("<html></html>")
      (site_dir / "status").mkpath
      (site_dir / "status" / "index.html").write("<html></html>")
      (site_dir / "architecture").mkpath
      (site_dir / "architecture" / "index.html").write("<html></html>")
      (site_dir / "python-aot-compilers").mkpath
      (site_dir / "python-aot-compilers" / "index.html").write("<html></html>")
      (site_dir / "ai-native").mkpath
      (site_dir / "ai-native" / "index.html").write("<html></html>")
      # Init a git repo so git log works.
      `git -C "#{root}" init -q 2>/dev/null`
      `git -C "#{root}" add -A 2>/dev/null`
      `git -C "#{root}" -c user.email=test@test -c user.name=test commit -q -m init 2>/dev/null`
      output = `#{RbConfig.ruby} #{CHECKER} #{root} 2>&1`
      return $?.exitstatus, output
    end
  end

  def live_sitemap
    (REPO_ROOT / "site" / "sitemap.xml").read
  end

  # --- Positive: the live sitemap passes ---

  def test_live_sitemap_passes
    status, output = `#{RbConfig.ruby} #{CHECKER} #{REPO_ROOT} 2>&1`
    assert_equal 0, $?.exitstatus, "live sitemap failed:\n#{output}"
  end

  # --- Negative: stale lastmod ---

  def test_rejects_stale_lastmod
    text = live_sitemap.sub(
      %r{<loc>https://rotnov\.github\.io/pycc/</loc>\s*<lastmod>2026-08-13</lastmod>},
      "<loc>https://rotnov.github.io/pycc/</loc>\n    <lastmod>2020-01-01</lastmod>"
    )
    status, output = run_checker(text)
    refute_equal 0, status, "accepted stale lastmod:\n#{output}"
  end

  # --- Negative: malformed lastmod ---

  def test_rejects_malformed_lastmod
    text = live_sitemap.sub(
      /<lastmod>2026-08-13<\/lastmod>/,
      "<lastmod>not-a-date</lastmod>"
    )
    status, output = run_checker(text)
    refute_equal 0, status, "accepted malformed lastmod:\n#{output}"
  end

  # --- Negative: unknown URL ---

  def test_rejects_unknown_url
    text = live_sitemap.sub(
      %r{<loc>https://rotnov\.github\.io/pycc/</loc>},
      "<loc>https://example.com/unknown/</loc>"
    )
    status, output = run_checker(text)
    refute_equal 0, status, "accepted unknown URL:\n#{output}"
  end

  # --- Negative: empty sitemap ---

  def test_rejects_empty_sitemap
    status, output = run_checker("<?xml version=\"1.0\"?>\n<urlset></urlset>")
    refute_equal 0, status, "accepted empty sitemap:\n#{output}"
  end
end
