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
# Exits 0 when no canonical page source is touched, or when every date pin a
# touched page actually carries already matches the predicted merge date.
# Exits 1 otherwise, naming every pin that must be rotated.
#
# All four pins, not just the sitemap
# -----------------------------------
# Each canonical page is pinned in four places: the sitemap <lastmod>, the
# JSON-LD WebPage "dateModified" in the page's own HTML, the PAGE_SPECS
# "date_modified" in scripts/check-site.sh, and the "source_artifact_sha256"
# digest of the page HTML in the performance manifest. Validating only the
# first would let a partial rotation pass: updating just the sitemap and
# recomputing the digest keeps every required `ci-gate` job green, because only
# the non-required Pages workflow runs check-site.sh -- so this checker would
# approve a merge that immediately leaves Pages red. All four are therefore
# validated at the head revision.
#
# A pin that is *absent* is another checker's diagnostic (check-site.sh and
# check_pages_performance_budget.rb own "this page is missing from my inputs"),
# so absence is a recorded skip rather than a competing error here. A pin that
# is *present but stale* is exactly this checker's business.

require "digest"
require "json"
require "open3"
require "pathname"
require "time"

require_relative "site_canonical_pages"

class SitePinMergeCurrencyError < StandardError; end

SITEMAP_RELATIVE_PATH = "site/sitemap.xml"
PERFORMANCE_MANIFEST_PATH = "tests/fixtures/pages-performance-manifest.json"
SITE_CHECKER_PATH = "scripts/check-site.sh"
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

# PAGE_SPECS in scripts/check-site.sh is a Python dict literal embedded in a
# heredoc, so it cannot be parsed as JSON or evaluated. It is read the way it
# is written there: the dict opens with a line ending in `PAGE_SPECS = {`, its
# slug keys sit at four-space indentation (`    "status": {`), each spec's
# entries sit at eight (`        "date_modified": "2026-09-07",`), and the dict
# closes with a `}` in column zero. Bounding the scan by that column-zero close
# keeps the scan from bleeding into the Python code that follows.
# Returns a slug => date hash, or nil when the dict is not found at all --
# scripts/check-site.sh owns diagnosing its own shape.
def page_specs_dates(check_site_text)
  dates = {}
  slug = nil
  inside = false
  check_site_text.each_line do |line|
    stripped = line.chomp
    if !inside
      inside = true if stripped =~ /PAGE_SPECS\s*=\s*\{\s*\z/
      next
    end
    break if stripped == "}"

    if (match = stripped.match(/\A {4}"([^"]+)":\s*\{\s*\z/))
      slug = match[1]
    elsif (match = stripped.match(/\A {8}"date_modified":\s*"([^"]+)",?\s*\z/))
      dates[slug] = match[1] unless slug.nil?
    end
  end
  return nil unless inside

  dates
end

# The PAGE_SPECS key for a canonical page source, or nil when the page has no
# entry. The mapping is mechanical -- `site/<slug>/index.html` keys on
# `<slug>` -- with one genuine exception: `site/index.html`, the landing page,
# has no PAGE_SPECS entry at all. scripts/check-site.sh validates the landing
# page in a separate block with its own assertions, so there is no third date
# pin for it to be stale against; it is a skip, not a failure.
def page_specs_key(source)
  match = source.match(%r{\Asite/([^/]+)/index\.html\z})
  match && match[1]
end

# The JSON-LD "dateModified" of the page's WebPage node, mirroring how
# scripts/check-site.sh reads it: parse the page's single
# `application/ld+json` block and take the "@graph" entry whose "@type" is
# "WebPage". Returns nil when the block is missing or unparseable -- that page
# shape is check-site.sh's diagnostic, not this checker's.
def json_ld_date_modified(html_text)
  block = html_text[%r{<script[^>]*type=["']application/ld\+json["'][^>]*>(.*?)</script>}m, 1]
  return nil if block.nil?

  document = JSON.parse(block)
  graph = document["@graph"]
  return nil unless graph.is_a?(Array)

  web_page = graph.find { |node| node.is_a?(Hash) && node["@type"] == "WebPage" }
  web_page && web_page["dateModified"]
rescue JSON::ParserError
  nil
end

# source path => source_artifact_sha256, taken only from the manifest's
# "canonical_pages" cohort. The manifest also carries non-canonical entries
# (the error page), which have no canonical-page date pins and must not
# participate. Returns nil when the manifest cannot be parsed --
# check_pages_performance_budget.rb owns that diagnostic.
def manifest_source_digests(manifest_text)
  document = JSON.parse(manifest_text)
  pages = document["canonical_pages"]
  return nil unless pages.is_a?(Array)

  pages.each_with_object({}) do |page, digests|
    next unless page.is_a?(Hash)

    artifact = page["source_artifact"]
    digest = page["source_artifact_sha256"]
    digests[artifact] = digest if artifact && digest
  end
rescue JSON::ParserError
  nil
end

def remediation_message(stale, predicted_date, offset_description)
  lines = stale.flat_map do |source, pins|
    ["  #{source}:"] + pins.map { |pin| "    #{pin}" }
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

  # The other three pins live in whole files that may legitimately be absent
  # from a given revision. A missing scripts/check-site.sh or performance
  # manifest reds every other website gate in the repository already, so
  # emitting a competing diagnostic here would add nothing; skip those pins and
  # say so in the summary instead.
  check_site_text = read_file_at_revision(root, head_revision, SITE_CHECKER_PATH)
  page_specs = check_site_text && page_specs_dates(check_site_text)
  manifest_text = read_file_at_revision(root, head_revision, PERFORMANCE_MANIFEST_PATH)
  manifest_digests = manifest_text && manifest_source_digests(manifest_text)

  stale = {}
  skipped = []
  touched_sources.each do |source|
    pins = []

    canonical = SOURCE_TO_CANONICAL[source]
    lastmod = lastmods[canonical]
    # A page absent from the sitemap is check_sitemap_lastmod.rb's and
    # check-site.sh's business, not this checker's; skip rather than
    # duplicating (and disagreeing with) their diagnostics. The same principle
    # governs every skip below: a pin that is *absent* belongs to another
    # checker, a pin that is *present but stale* belongs to this one.
    if lastmod.nil?
      skipped << "#{source}: <lastmod> not verified (absent from #{SITEMAP_RELATIVE_PATH})"
    elsif lastmod != predicted_date
      pins << "<lastmod> for #{canonical} in #{SITEMAP_RELATIVE_PATH}: " \
              "pinned #{lastmod}, predicted merge date #{predicted_date}"
    end

    page_html = read_file_at_revision(root, head_revision, source)
    date_modified = page_html && json_ld_date_modified(page_html)
    if date_modified.nil?
      skipped << "#{source}: JSON-LD dateModified not verified (no parseable WebPage node)"
    elsif date_modified != predicted_date
      pins << "JSON-LD WebPage \"dateModified\" in #{source}: " \
              "pinned #{date_modified}, predicted merge date #{predicted_date}"
    end

    slug = page_specs_key(source)
    spec_date = slug && page_specs && page_specs[slug]
    if spec_date.nil?
      skipped << "#{source}: PAGE_SPECS date_modified not verified " \
                 "(no entry in #{SITE_CHECKER_PATH} at this revision)"
    elsif spec_date != predicted_date
      pins << "PAGE_SPECS[#{slug.inspect}][\"date_modified\"] in #{SITE_CHECKER_PATH}: " \
              "pinned #{spec_date}, predicted merge date #{predicted_date}"
    end

    pinned_digest = manifest_digests && manifest_digests[source]
    if pinned_digest.nil? || page_html.nil?
      skipped << "#{source}: source_artifact_sha256 not verified " \
                 "(no entry in #{PERFORMANCE_MANIFEST_PATH} at this revision)"
    else
      actual_digest = Digest::SHA256.hexdigest(page_html)
      if pinned_digest != actual_digest
        pins << "\"source_artifact_sha256\" for #{source} in #{PERFORMANCE_MANIFEST_PATH}: " \
                "pinned #{pinned_digest}, but the page HTML at this revision hashes to " \
                "#{actual_digest}"
      end
    end

    stale[source] = pins unless pins.empty?
  end

  unless stale.empty?
    raise SitePinMergeCurrencyError, remediation_message(stale, predicted_date, offset_description)
  end

  summary = "#{touched_sources.length} touched canonical page source(s) pinned to the " \
            "predicted merge date #{predicted_date} (#{offset_description})"
  summary += "; skipped: #{skipped.join('; ')}" unless skipped.empty?
  summary
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
