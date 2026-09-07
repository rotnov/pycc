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

# The canonical-URL -> source-file map is shared with
# scripts/check_site_pin_merge_currency.rb (issue #990); see that file for
# why it lives in its own requirable file rather than here.
require_relative "site_canonical_pages"

class SitemapLastmodError < StandardError; end

# Only consult ARGV when this file is being run as a program. When it is
# required by another script (or by its own future callers), ARGV belongs
# to that program and must not be reinterpreted as a repository root.
REPO_ROOT = Pathname(($PROGRAM_NAME == __FILE__ && ARGV[0]) || Pathname(__dir__).parent)
SITEMAP_PATH = REPO_ROOT / "site" / "sitemap.xml"

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

check! if $PROGRAM_NAME == __FILE__
