#!/usr/bin/env ruby
# frozen_string_literal: true

# Validates that sitemap <lastmod> values and JSON-LD dateModified values
# match the last git commit date that modified each canonical page's
# source file (issue #201).
#
# The existing check-site.sh validates internal consistency (sitemap
# lastmod == JSON-LD dateModified == hard-coded date), but does not
# verify that the date actually corresponds to the last content change.
# This validator closes that gap: if a page's source file is modified
# without updating the sitemap lastmod, the git commit date will be
# newer than the sitemap lastmod and this validator will reject it.
#
# Usage: ruby scripts/check_sitemap_lastmod.rb [repository_root]
#
# Exits 0 if all dates match the git history, 1 otherwise.

require "date"
require "pathname"

class SitemapLastmodError < StandardError; end

REPO_ROOT = Pathname(ARGV[0] || Pathname(__dir__).parent)
SITEMAP_PATH = REPO_ROOT / "site" / "sitemap.xml"

# Maps canonical URLs to their source HTML files relative to repo root.
CANONICAL_TO_SOURCE = {
  "https://rotnov.github.io/pycc/" => "site/index.html",
  "https://rotnov.github.io/pycc/status/" => "site/status/index.html",
  "https://rotnov.github.io/pycc/architecture/" => "site/architecture/index.html",
  "https://rotnov.github.io/pycc/python-aot-compilers/" =>
    "site/python-aot-compilers/index.html",
  "https://rotnov.github.io/pycc/ai-native/" => "site/ai-native/index.html",
  "https://rotnov.github.io/pycc/language-support/" => "site/language-support/index.html",
  "https://rotnov.github.io/pycc/diagnostics/" => "site/diagnostics/index.html",
}.freeze

def git_last_commit_date(file_path)
  # Returns the date (YYYY-MM-DD) of the last non-merge commit that
  # modified the given file, or nil if the file is not tracked by git.
  # Uses the author date (when the change was actually written) rather
  # than the committer date (which can be bumped by merge/rebase), and
  # skips merge commits so that CI merge commits don't shadow the real
  # last-content-change date.
  dir = REPO_ROOT.to_s
  output = `git -C "#{dir}" log -1 --no-merges --format=%as -- "#{file_path}" 2>/dev/null`.strip
  return nil if output.empty?
  output
end

def check!
  raise SitemapLastmodError, "sitemap.xml not found at #{SITEMAP_PATH}" unless SITEMAP_PATH.exist?

  sitemap_text = SITEMAP_PATH.read

  # Parse each <url> entry from the sitemap.
  entries = sitemap_text.scan(
    %r{<loc>([^<]+)</loc>\s*<lastmod>([^<]+)</lastmod>}
  )

  raise SitemapLastmodError, "sitemap.xml contains no URL entries" if entries.empty?

  entries.each do |location, lastmod|
    source = CANONICAL_TO_SOURCE[location]
    unless source
      raise SitemapLastmodError,
            "sitemap URL #{location} has no mapped source file"
    end

    source_path = REPO_ROOT / source
    unless source_path.exist?
      raise SitemapLastmodError,
            "source file #{source} for #{location} does not exist"
    end

    # Validate lastmod format.
    begin
      Date.parse(lastmod)
    rescue ArgumentError
      # `Date::Error` only exists on Ruby 3.0+, where it is a subclass of
      # `ArgumentError`; on Ruby 2.x `Date.parse` raises a bare
      # `ArgumentError` and naming `Date::Error` here would itself raise
      # `NameError`, crashing the checker with a backtrace instead of
      # reporting the malformed date. Rescuing `ArgumentError` covers both.
      raise SitemapLastmodError,
            "sitemap lastmod for #{location} is not a valid date: #{lastmod}"
    end

    # Get the last git commit date for the source file.
    git_date = git_last_commit_date(source)
    unless git_date
      # File is not tracked by git — skip (may be a new file in a PR).
      next
    end

    if lastmod != git_date
      raise SitemapLastmodError,
            "sitemap lastmod for #{location} is #{lastmod} but the last " \
            "git commit modifying #{source} was on #{git_date} — the " \
            "lastmod must match the last content-change commit date " \
            "(issue #201)"
    end
  end

  puts "Sitemap lastmod dates match git history."
rescue SitemapLastmodError => e
  warn "Sitemap lastmod check failed: #{e.message}"
  exit 1
end

check!
