#!/usr/bin/env ruby
# frozen_string_literal: true

# Rejects a pull request whose canonical-page date pins are already stale
# against the date its own squash-merge commit will carry (issue #990).
#
# Why a second date checker exists
# --------------------------------
# scripts/check_sitemap_lastmod.rb validates a *committed* tree: every
# canonical page's sitemap <lastmod> must equal the author date of the last
# non-merge commit that touched that page's source file. That contract is
# correct, but it is structurally incapable of catching the day-N/day-N+1
# defect:
#
#   * On day N a branch edits site/status/index.html and pins every date to
#     day N. The PR leg of check_sitemap_lastmod.rb passes: the branch's own
#     last commit for that file really is dated day N.
#   * The PR is squash-merged on day N+1. A squash commit's author date is
#     the merge instant in the merging identity's timezone, not the branch's
#     last commit time, so on `main` the last commit touching that file is
#     now dated day N+1 while the pins still say day N.
#   * The very same checker, run on `main` by .github/workflows/pages.yml,
#     now fails -- turning Pages red on the default branch after the merge,
#     when it is most expensive to notice and fix.
#
# This checker closes that window by comparing the pins in the *head* tree
# against the date the merge commit is predicted to carry, and it runs on the
# pull-request leg where the problem is still cheap to fix.
#
# What "today" means here
# -----------------------
# The reference is the date `git log --format=%as` would report for the
# prospective squash commit -- an author date rendered in the merging
# identity's UTC offset -- not `date -u +%F`. Those two disagree for part of
# every day whenever the offset is non-zero, and picking either one silently
# would make the checker wrong (in one direction or the other) during that
# window. The offset is estimated from the base revision's own author date:
# the base is `main`'s tip (or the merge-base with it), which is itself a
# prior squash commit written by the same merging identity, so it is the best
# available evidence of the offset the next squash will carry. When that
# offset cannot be parsed the checker falls back to UTC and says so.
#
# When the UTC date and the offset date disagree, the checker fails closed
# rather than choosing one: near a date rollover the prospective merge date is
# genuinely ambiguous, and a wrong pass here is exactly the failure this
# checker exists to prevent.
#
# Ordering note: the rollover guard is applied only after the diff has shown
# that a canonical page source is actually touched. Applying it earlier would
# make every pull request in the repository fail during the rollover window,
# not just the ones that pin dates.
#
# Usage:
#   ruby scripts/check_site_pin_merge_currency.rb <base-revision> [head-revision] [repository-root]
#
# Exits 0 when no canonical page source is touched, or when every touched
# page's sitemap <lastmod> already equals the predicted merge date. Exits 1
# otherwise, naming every pin that must be rotated.

require "open3"
require "pathname"
require "time"

require_relative "site_canonical_pages"

class SitePinMergeCurrencyError < StandardError; end

SITEMAP_RELATIVE_PATH = "site/sitemap.xml"
PERFORMANCE_MANIFEST_PATH = "tests/fixtures/pages-performance-manifest.json"
WEBSITE_GUIDANCE_DOC = "docs/WEBSITE.md"
PIN_CURRENCY_ISSUE_REFERENCE = "https://github.com/rotnov/pycc/issues/990"

def run_git(root, args, context)
  stdout, stderr, status = Open3.capture3("git", *args, chdir: root.to_s)
  raise SitePinMergeCurrencyError, "#{context}: #{stderr.strip}" unless status.success?

  stdout.force_encoding(Encoding::UTF_8)
end

def revision_present?(root, revision)
  _stdout, _stderr, status = Open3.capture3(
    "git", "cat-file", "-e", "#{revision}^{commit}", chdir: root.to_s
  )
  status.success?
end

# A shallow CI checkout only has the pull request's own history, so the base
# revision usually needs an explicit single-commit fetch. A local full-history
# checkout already has it and never touches the network.
def ensure_revision_available(root, revision)
  return if revision_present?(root, revision)

  _stdout, stderr, status = Open3.capture3(
    "git", "fetch", "--no-tags", "--depth=1", "origin", revision, chdir: root.to_s
  )
  return if status.success? || revision_present?(root, revision)

  raise SitePinMergeCurrencyError,
        "could not resolve revision #{revision.inspect} locally or via " \
        "'git fetch --no-tags --depth=1 origin #{revision}': #{stderr.strip}"
end

def read_file_at_revision(root, revision, relative_path)
  _stdout, _stderr, exists_status = Open3.capture3(
    "git", "cat-file", "-e", "#{revision}:#{relative_path}", chdir: root.to_s
  )
  return nil unless exists_status.success?

  run_git(root, ["show", "#{revision}:#{relative_path}"],
          "could not read #{relative_path} at #{revision}")
end

def diff_name_only(root, base_revision, head_revision)
  # A two-dot diff against the exact base SHA, not a three-dot merge-base
  # diff: a shallow CI checkout does not have enough history to compute a
  # merge base. The caller supplies the merge base when it wants one.
  output = run_git(
    root,
    ["diff", "--name-only", base_revision, head_revision],
    "could not compute the base..head diff (#{base_revision}..#{head_revision})"
  )
  output.each_line.map(&:strip).reject(&:empty?)
end

# The UTC offset, in seconds, that the base revision's author date carries.
# Returns nil when it cannot be read or parsed, which the caller reports as a
# fall back to UTC.
def base_author_offset_seconds(root, revision)
  stdout, _stderr, status = Open3.capture3(
    "git", "show", "-s", "--format=%aI", revision, chdir: root.to_s
  )
  return nil unless status.success?

  Time.iso8601(stdout.strip).utc_offset
rescue ArgumentError
  nil
end

# Parse the sitemap's <loc>/<lastmod> pairs into a canonical URL => lastmod
# hash. Mirrors the pattern scripts/check_sitemap_lastmod.rb uses; that
# checker owns validating the sitemap's shape, so a sitemap this one cannot
# parse is reported as an unusable input rather than re-validated here.
def sitemap_lastmods(sitemap_text)
  sitemap_text.scan(%r{<loc>([^<]+)</loc>\s*<lastmod>([^<]+)</lastmod>}).to_h
end

def remediation_message(stale, predicted_date, offset_description)
  lines = stale.map do |source, lastmod|
    "  #{source}: pinned #{lastmod}, but this pull request's squash-merge " \
      "commit is predicted to be dated #{predicted_date}"
  end

  <<~MESSAGE.strip
    Canonical page date pins are stale against the predicted merge date
    (#{predicted_date}, #{offset_description}):

    #{lines.join("\n")}

    A squash-merge commit's author date is the merge instant, not the
    branch's last commit time, so these pins will be stale on `main` the
    moment this pull request lands -- and scripts/check_sitemap_lastmod.rb
    will then fail the Pages workflow on the default branch.

    Rotate all four pins for each page listed above to #{predicted_date},
    then recompute the manifest digest:

      1. <lastmod> for the page's <loc> in #{SITEMAP_RELATIVE_PATH}
      2. the JSON-LD "dateModified" in the page's own HTML source
      3. PAGE_SPECS["<page>"]["date_modified"] in scripts/check-site.sh
      4. "source_artifact_sha256" for the page in
         #{PERFORMANCE_MANIFEST_PATH} -- this digest covers the page HTML,
         so it must be recomputed after steps 2 and 3, e.g.
         `shasum -a 256 site/status/index.html`

    Then re-run `ruby scripts/check_pages_performance_budget.rb --skip-lighthouse`
    and `bash scripts/check-site.sh`. See #{WEBSITE_GUIDANCE_DOC} and
    #{PIN_CURRENCY_ISSUE_REFERENCE}.
  MESSAGE
end

def check_site_pin_merge_currency(root, base_revision, head_revision,
                                  now: Time.now, diff_fetcher: method(:diff_name_only))
  ensure_revision_available(root, base_revision)
  ensure_revision_available(root, head_revision)

  changed_files = diff_fetcher.call(root, base_revision, head_revision)
  touched_sources = CANONICAL_TO_SOURCE.values.select { |source| changed_files.include?(source) }

  # Deliberately before any date reasoning: a pull request that touches no
  # canonical page source can never have a stale pin, and must not be exposed
  # to the fail-closed rollover guard below.
  return "no canonical page source touched" if touched_sources.empty?

  utc_now = now.getutc
  utc_date = utc_now.strftime("%F")
  offset_seconds = base_author_offset_seconds(root, base_revision)
  if offset_seconds.nil?
    offset_date = utc_date
    offset_description = "UTC; the base revision's author-date offset could not be read"
  else
    offset_date = (utc_now + offset_seconds).strftime("%F")
    offset_description = "UTC offset #{format_offset(offset_seconds)}, taken from the base revision's author date"
  end

  if utc_date != offset_date
    raise SitePinMergeCurrencyError, <<~MESSAGE.strip
      The prospective merge date is ambiguous right now: it is #{utc_date} in UTC
      but #{offset_date} at the merging identity's offset (#{format_offset(offset_seconds)}),
      and this pull request touches #{touched_sources.join(', ')}.

      Failing closed rather than guessing: a squash-merge landing within this
      rollover window could carry either date, so no pin value can be verified
      as correct. Re-run this check once the two dates agree, or land the pull
      request outside the rollover window.
    MESSAGE
  end

  predicted_date = offset_date

  sitemap_text = read_file_at_revision(root, head_revision, SITEMAP_RELATIVE_PATH)
  if sitemap_text.nil?
    raise SitePinMergeCurrencyError,
          "#{SITEMAP_RELATIVE_PATH} does not exist at #{head_revision}, so the pins of " \
          "#{touched_sources.join(', ')} cannot be verified"
  end

  lastmods = sitemap_lastmods(sitemap_text)
  stale = {}
  touched_sources.each do |source|
    canonical = SOURCE_TO_CANONICAL[source]
    lastmod = lastmods[canonical]
    # A page absent from the sitemap is check_sitemap_lastmod.rb's and
    # check-site.sh's business, not this checker's; skip rather than
    # duplicating (and disagreeing with) their diagnostics.
    next if lastmod.nil?

    stale[source] = lastmod if lastmod != predicted_date
  end

  unless stale.empty?
    raise SitePinMergeCurrencyError, remediation_message(stale, predicted_date, offset_description)
  end

  "#{touched_sources.length} touched canonical page source(s) pinned to the " \
    "predicted merge date #{predicted_date} (#{offset_description})"
end

def format_offset(seconds)
  return "+00:00" if seconds.nil?

  sign = seconds.negative? ? "-" : "+"
  magnitude = seconds.abs
  format("%s%02d:%02d", sign, magnitude / 3600, (magnitude % 3600) / 60)
end

def main(arguments)
  if arguments.empty? || arguments.length > 3
    raise SitePinMergeCurrencyError,
          "usage: check_site_pin_merge_currency.rb <base-revision> [head-revision] " \
          "[repository-root]"
  end

  base_revision = arguments[0]
  head_revision = arguments[1] || "HEAD"
  root = Pathname(arguments[2] || ".")
  result = check_site_pin_merge_currency(root, base_revision, head_revision)
  puts "Site pin merge-currency check passed (#{result})."
  0
rescue SitePinMergeCurrencyError => e
  warn e.message
  1
end

exit(main(ARGV)) if $PROGRAM_NAME == __FILE__
