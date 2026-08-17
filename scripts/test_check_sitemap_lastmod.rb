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

# The date embedded in the live sitemap after issue #201's fix.
# Tests that derive from the live sitemap use this to locate the
# original lastmod value before mutating it.
LIVE_LASTMOD = "2026-08-14"

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

  # Run the checker against a temp repo where a source file receives a
  # second commit with a controlled author date, simulating a content
  # change that is not reflected in the sitemap lastmod. The initial
  # commit uses +initial_date+ so that all pages start in sync with the
  # sitemap; the second commit uses +change_date+ to advance one page's
  # git history past the sitemap lastmod.
  def run_checker_after_content_change(sitemap_text, initial_date, change_date)
    Dir.mktmpdir do |dir|
      root = Pathname(dir)
      site_dir = root / "site"
      site_dir.mkpath
      (site_dir / "sitemap.xml").write(sitemap_text)
      (site_dir / "index.html").write("<html></html>")
      (site_dir / "status").mkpath
      (site_dir / "status" / "index.html").write("<html></html>")
      (site_dir / "architecture").mkpath
      (site_dir / "architecture" / "index.html").write("<html></html>")
      (site_dir / "python-aot-compilers").mkpath
      (site_dir / "python-aot-compilers" / "index.html").write("<html></html>")
      (site_dir / "ai-native").mkpath
      (site_dir / "ai-native" / "index.html").write("<html></html>")
      `git -C "#{root}" init -q 2>/dev/null`
      `git -C "#{root}" add -A 2>/dev/null`
      `git -C "#{root}" -c user.email=test@test -c user.name=test \
        -c committer.date="#{initial_date}T00:00:00" \
        commit -q --date="#{initial_date}T00:00:00" -m init 2>/dev/null`
      # Simulate a content change to the status page with a controlled
      # author date that is newer than the sitemap lastmod.
      (site_dir / "status" / "index.html").write("<html><body>updated</body></html>")
      `git -C "#{root}" add -A 2>/dev/null`
      `git -C "#{root}" -c user.email=test@test -c user.name=test \
        -c committer.date="#{change_date}T00:00:00" \
        commit -q --date="#{change_date}T00:00:00" -m "content change" 2>/dev/null`
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
      %r{<loc>https://rotnov\.github\.io/pycc/</loc>\s*<lastmod>#{LIVE_LASTMOD}</lastmod>},
      "<loc>https://rotnov.github.io/pycc/</loc>\n    <lastmod>2020-01-01</lastmod>"
    )
    status, output = run_checker(text)
    refute_equal 0, status, "accepted stale lastmod:\n#{output}"
  end

  # --- Negative: malformed lastmod ---

  def test_rejects_malformed_lastmod
    text = live_sitemap.sub(
      /<lastmod>#{LIVE_LASTMOD}<\/lastmod>/,
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

  # --- Negative: content change without sitemap update (issue #201 core scenario) ---

  def test_rejects_lastmod_after_content_change
    # The sitemap has the original lastmod, but a subsequent commit
    # modifies a source file with a newer author date. The validator
    # must reject the now-stale lastmod. The change date must be newer
    # than the status page's live lastmod (2026-08-15) to create a real
    # staleness gap.
    text = live_sitemap
    status, output = run_checker_after_content_change(text, LIVE_LASTMOD, "2026-08-16")
    refute_equal 0, status, "accepted stale lastmod after content change:\n#{output}"
    assert_match(/status/, output, "error should name the stale page")
  end

  # --- Negative: future-dated lastmod ---

  def test_rejects_future_lastmod
    text = live_sitemap.sub(
      /<lastmod>#{LIVE_LASTMOD}<\/lastmod>/,
      "<lastmod>9999-12-31</lastmod>"
    )
    status, output = run_checker(text)
    refute_equal 0, status, "accepted future-dated lastmod:\n#{output}"
  end

  # --- Negative: missing source file ---

  def test_rejects_missing_source_file
    Dir.mktmpdir do |dir|
      root = Pathname(dir)
      site_dir = root / "site"
      site_dir.mkpath
      (site_dir / "sitemap.xml").write(live_sitemap)
      # Create all source files except the landing page.
      (site_dir / "status").mkpath
      (site_dir / "status" / "index.html").write("<html></html>")
      (site_dir / "architecture").mkpath
      (site_dir / "architecture" / "index.html").write("<html></html>")
      (site_dir / "python-aot-compilers").mkpath
      (site_dir / "python-aot-compilers" / "index.html").write("<html></html>")
      (site_dir / "ai-native").mkpath
      (site_dir / "ai-native" / "index.html").write("<html></html>")
      `git -C "#{root}" init -q 2>/dev/null`
      `git -C "#{root}" add -A 2>/dev/null`
      `git -C "#{root}" -c user.email=test@test -c user.name=test commit -q -m init 2>/dev/null`
      output = `#{RbConfig.ruby} #{CHECKER} #{root} 2>&1`
      refute_equal 0, $?.exitstatus, "accepted missing source file:\n#{output}"
    end
  end
end
