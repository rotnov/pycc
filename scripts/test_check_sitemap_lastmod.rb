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

# The canonical URL of the landing page, used to anchor every single-page
# mutation below. Substituting a bare date literal is not safe: several pages
# legitimately share a lastmod, so a bare `sub` silently retargets onto
# whichever page happens to come first in document order, and the mutation
# then tests a page the change under test never touched.
HOME_LOC = "https://rotnov.github.io/pycc/"

# The author date every temp-repository commit is stamped with. The checker
# requires each page's lastmod to equal the last commit date of its source
# file exactly, so mutation fixtures are built by setting every lastmod to
# this date (see `synced_sitemap`) and then changing exactly one of them. A
# fixture that is in sync everywhere passes (`test_synced_sitemap_passes`),
# which is what makes each single-page mutation attributable: without that
# positive control every mutation test would pass simply because some other
# page's date never matched the temp repository's commit date.
SYNC_DATE = "2026-01-02"

class TestCheckSitemapLastmod < Minitest::Test
  def run_checker(sitemap_text, commit_date = SYNC_DATE)
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
      %w[language-support diagnostics].each do |slug|
        (site_dir / slug).mkpath
        (site_dir / slug / "index.html").write("<html></html>")
      end
      # Init a git repo so git log works.
      `git -C "#{root}" init -q 2>/dev/null`
      `git -C "#{root}" add -A 2>/dev/null`
      `git -C "#{root}" -c user.email=test@test -c user.name=test \
        -c committer.date="#{commit_date}T00:00:00" \
        commit -q --date="#{commit_date}T00:00:00" -m init 2>/dev/null`
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
      %w[language-support diagnostics].each do |slug|
        (site_dir / slug).mkpath
        (site_dir / slug / "index.html").write("<html></html>")
      end
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

  # The live sitemap with every page's lastmod set to +date+, matching a temp
  # repository whose only commit carries that author date.
  def synced_sitemap(date = SYNC_DATE)
    live_sitemap.gsub(%r{<lastmod>[^<]+</lastmod>}, "<lastmod>#{date}</lastmod>")
  end

  # Replace the lastmod of exactly one page, located by its own <loc>, so the
  # mutation cannot drift onto a different entry that shares the same date.
  def replace_lastmod(text, loc, value)
    pattern = %r{(<loc>#{Regexp.escape(loc)}</loc>\s*<lastmod>)[^<]+(</lastmod>)}
    unless text.match?(pattern)
      raise "sitemap fixture has no <lastmod> entry for #{loc}"
    end
    text.sub(pattern) { "#{Regexp.last_match(1)}#{value}#{Regexp.last_match(2)}" }
  end

  # --- Positive: the live sitemap passes ---

  def test_live_sitemap_passes
    status, output = `#{RbConfig.ruby} #{CHECKER} #{REPO_ROOT} 2>&1`
    assert_equal 0, $?.exitstatus, "live sitemap failed:\n#{output}"
  end

  # --- Positive control: an in-sync fixture passes ---
  #
  # Every mutation below is only meaningful because this passes: it proves the
  # unmutated fixture and the temp repository's commit date agree, so a
  # non-zero exit afterwards is attributable to the single mutated page.

  def test_synced_sitemap_passes
    status, output = run_checker(synced_sitemap)
    assert_equal 0, status, "in-sync temp sitemap failed:\n#{output}"
  end

  # --- Negative: stale lastmod ---

  def test_rejects_stale_lastmod
    text = replace_lastmod(synced_sitemap, HOME_LOC, "2020-01-01")
    status, output = run_checker(text)
    refute_equal 0, status, "accepted stale lastmod:\n#{output}"
    assert_match(/#{Regexp.escape(HOME_LOC)}/, output,
                 "error should name the mutated page")
  end

  # --- Negative: malformed lastmod ---

  def test_rejects_malformed_lastmod
    text = replace_lastmod(synced_sitemap, HOME_LOC, "not-a-date")
    status, output = run_checker(text)
    refute_equal 0, status, "accepted malformed lastmod:\n#{output}"
    assert_match(/not a valid date/, output,
                 "error should identify the malformed value")
  end

  # --- Negative: unknown URL ---

  def test_rejects_unknown_url
    text = synced_sitemap.sub(
      %r{<loc>#{Regexp.escape(HOME_LOC)}</loc>},
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
    # Every page starts in sync with the initial commit, then a subsequent
    # commit modifies one source file with a newer author date. The validator
    # must reject that page's now-stale lastmod, and only that page's.
    text = synced_sitemap
    status, output = run_checker_after_content_change(text, SYNC_DATE, "2026-01-03")
    refute_equal 0, status, "accepted stale lastmod after content change:\n#{output}"
    assert_match(/status/, output, "error should name the stale page")
  end

  # --- Negative: future-dated lastmod ---

  def test_rejects_future_lastmod
    text = replace_lastmod(synced_sitemap, HOME_LOC, "9999-12-31")
    status, output = run_checker(text)
    refute_equal 0, status, "accepted future-dated lastmod:\n#{output}"
    assert_match(/#{Regexp.escape(HOME_LOC)}/, output,
                 "error should name the mutated page")
  end

  # --- Negative: missing source file ---

  def test_rejects_missing_source_file
    Dir.mktmpdir do |dir|
      root = Pathname(dir)
      site_dir = root / "site"
      site_dir.mkpath
      (site_dir / "sitemap.xml").write(synced_sitemap)
      # Create all source files except the landing page.
      (site_dir / "status").mkpath
      (site_dir / "status" / "index.html").write("<html></html>")
      (site_dir / "architecture").mkpath
      (site_dir / "architecture" / "index.html").write("<html></html>")
      (site_dir / "python-aot-compilers").mkpath
      (site_dir / "python-aot-compilers" / "index.html").write("<html></html>")
      (site_dir / "ai-native").mkpath
      (site_dir / "ai-native" / "index.html").write("<html></html>")
      %w[language-support diagnostics].each do |slug|
        (site_dir / slug).mkpath
        (site_dir / slug / "index.html").write("<html></html>")
      end
      `git -C "#{root}" init -q 2>/dev/null`
      `git -C "#{root}" add -A 2>/dev/null`
      `git -C "#{root}" -c user.email=test@test -c user.name=test commit -q -m init 2>/dev/null`
      output = `#{RbConfig.ruby} #{CHECKER} #{root} 2>&1`
      refute_equal 0, $?.exitstatus, "accepted missing source file:\n#{output}"
    end
  end
end
