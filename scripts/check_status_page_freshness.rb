#!/usr/bin/env ruby
# frozen_string_literal: true

require "open3"
require "pathname"

class StatusPageFreshnessError < StandardError; end

# Reuse the marker convention scripts/check_roadmap_evidence.rb already
# established for docs/ROADMAP.md instead of reinventing marker parsing.
EVIDENCE_MARKER = /<!--\s*roadmap-evidence:\s*(?<id>[a-z0-9][a-z0-9-]*)\s*-->/
CHECKBOX_PREFIX = /\A\s*-\s*\[(?<mark>.)\]/
# The real docs/ROADMAP.md milestone line is a single physical line whose
# substantive status prose continues past the closing "**" of the bold
# span (e.g. "**Current milestone: v0.2 ... .** All five v0.1 ..."), so the
# signal must compare the whole line, not just the bold span, or an edit to
# that trailing prose alone would go undetected. A malformed/absent line
# (no "**Current milestone:" anywhere in the text) simply yields nil from
# `milestone_line` instead of matching -- nil vs. a real line is itself a
# correct "the milestone line changed" signal, so no special-casing is
# needed in `roadmap_signal?`.
MILESTONE_MARKER = /\*\*Current milestone:/

# A feature-landing paragraph in docs/ROADMAP.md opens with a bold issue
# reference and an em dash, e.g.
#   **[#432](https://github.com/rotnov/pycc/issues/432) — Inheritance, ...:**
# The captured issue number is the stable identity used for set comparison,
# so a text-only edit to an existing paragraph (same issue number, different
# description) does not fire -- only additions and removals do.
FEATURE_PARAGRAPH_MARKER = /\*\*\[#(\d+)\]\(https:\/\/github\.com\/rotnov\/pycc\/issues\/\d+\)\s*—/

ROADMAP_PATH = "docs/ROADMAP.md"
WATCHED_PAGES = %w[site/status/index.html site/index.html].freeze
ISSUE_REFERENCE = "https://github.com/rotnov/pycc/issues/401"
GUIDANCE_DOC = "docs/WEBSITE.md"

def run_git(root, args, context)
  stdout, stderr, status = Open3.capture3("git", *args, chdir: root.to_s)
  raise StatusPageFreshnessError, "#{context}: #{stderr.strip}" unless status.success?

  stdout.force_encoding(Encoding::UTF_8)
end

# Transient failures that a bounded retry can plausibly clear. The shallow
# case is the one actually observed in CI: two concurrent git operations on
# the same shallow checkout race, and the loser aborts with "shallow file
# has changed since we read it" without ever contacting the remote. The
# network cases are the ordinary transport flakes GitHub's git endpoint
# produces under load. Anything else -- a genuinely unresolvable revision,
# a missing `origin` remote -- fails on the first attempt, so a real
# misconfiguration is still reported immediately instead of being retried.
# Git's generic "Could not read from remote repository." line is
# deliberately absent: it accompanies a missing remote just as often as a
# transport flake, and every transport flake worth retrying already
# carries one of the more specific lines above.
TRANSIENT_FETCH_PATTERNS = [
  /shallow file has changed/i,
  /the remote end hung up/i,
  /early EOF/i,
  /connection (?:reset|timed out|refused)/i,
  /unable to access/i,
  /RPC failed/i,
  /HTTP 5\d\d/i
].freeze
FETCH_ATTEMPTS = 3
FETCH_RETRY_DELAYS = [1, 2].freeze

def revision_present?(root, revision)
  _stdout, _stderr, status = Open3.capture3(
    "git", "cat-file", "-e", "#{revision}^{commit}", chdir: root.to_s
  )
  status.success?
end

def transient_fetch_failure?(stderr)
  TRANSIENT_FETCH_PATTERNS.any? { |pattern| pattern.match?(stderr) }
end

# Make sure `revision` is resolvable as a commit in `root`'s repository.
# A shallow CI checkout only has the PR's own history, so the base
# revision usually needs an explicit fetch; a local full-history checkout
# (used by this script's own tests and by manual validation runs) already
# has it and should not need network access.
#
# The fetch is retried a bounded number of times, but only for the
# transient failures listed above -- see TRANSIENT_FETCH_PATTERNS. The
# presence check is repeated before each retry because a concurrent
# operation on the same checkout may have landed the object meanwhile,
# which is precisely the situation the shallow-file race describes.
# `sleeper` is injectable so the tests exercise the retry path without
# paying the real backoff.
def ensure_revision_available(root, revision, attempts: FETCH_ATTEMPTS, sleeper: ->(seconds) { sleep(seconds) })
  return if revision_present?(root, revision)

  last_stderr = ""
  attempts.times do |attempt|
    _stdout, stderr, fetch_status = Open3.capture3(
      "git", "fetch", "--no-tags", "--depth=1", "origin", revision, chdir: root.to_s
    )
    return if fetch_status.success?

    last_stderr = stderr.strip
    # A concurrent operation on the same checkout may have landed the
    # object even though this fetch aborted, which is exactly what the
    # shallow-file race describes.
    return if revision_present?(root, revision)
    break unless transient_fetch_failure?(last_stderr) && attempt < attempts - 1

    sleeper.call(FETCH_RETRY_DELAYS[attempt] || FETCH_RETRY_DELAYS.last)
  end

  raise StatusPageFreshnessError,
        "could not resolve revision #{revision.inspect} locally or via " \
        "'git fetch origin #{revision} --depth=1': #{last_stderr}"
end

def read_file_at_revision(root, revision, relative_path)
  _stdout, _stderr, exists_status = Open3.capture3(
    "git", "cat-file", "-e", "#{revision}:#{relative_path}", chdir: root.to_s
  )
  return "" unless exists_status.success?

  run_git(root, ["show", "#{revision}:#{relative_path}"],
          "could not read #{relative_path} at #{revision}")
end

def diff_name_only(root, base_revision, head_revision)
  # A two-dot diff against the exact base SHA, not a three-dot merge-base
  # diff: a shallow CI checkout does not have enough history to compute a
  # merge base.
  output = run_git(
    root,
    ["diff", "--name-only", base_revision, head_revision],
    "could not compute the base...head diff (#{base_revision}..#{head_revision})"
  )
  output.each_line.map(&:strip).reject(&:empty?)
end

def milestone_line(text)
  text.each_line.map(&:chomp).find { |line| MILESTONE_MARKER.match?(line) }
end

def evidence_checklist_states(text)
  states = {}
  text.each_line do |line|
    marker_ids = []
    line.scan(EVIDENCE_MARKER) { marker_ids << Regexp.last_match[:id] }
    next if marker_ids.empty?

    checkbox_match = CHECKBOX_PREFIX.match(line)
    checked = checkbox_match ? checkbox_match[:mark].strip.downcase == "x" : nil

    marker_ids.each { |id| states[id] = checked }
  end
  states
end

# Collect the sorted issue numbers of every feature-landing paragraph
# (`**[#NNN](...) — ...:**`) in `text`. A sorted array (rather than a Set)
# avoids needing `require "set"`; the existing script only requires `open3`
# and `pathname`. Set membership -- not text -- is the signal, so a
# description-only edit to an existing paragraph (same issue number) leaves
# the array unchanged and does not fire.
def feature_paragraph_ids(text)
  ids = []
  text.each_line do |line|
    line.scan(FEATURE_PARAGRAPH_MARKER) { ids << Regexp.last_match[1] }
  end
  ids.sort
end

def roadmap_signal?(base_text, head_text)
  milestone_line(base_text) != milestone_line(head_text) ||
    evidence_checklist_states(base_text) != evidence_checklist_states(head_text) ||
    feature_paragraph_ids(base_text) != feature_paragraph_ids(head_text)
end

def check_status_page_freshness(root, base_revision, head_revision, diff_fetcher: method(:diff_name_only))
  ensure_revision_available(root, base_revision)
  ensure_revision_available(root, head_revision)

  base_roadmap = read_file_at_revision(root, base_revision, ROADMAP_PATH)
  head_roadmap = read_file_at_revision(root, head_revision, ROADMAP_PATH)

  return "no roadmap milestone, evidence-checklist, or feature-landing-paragraph signal" unless roadmap_signal?(base_roadmap, head_roadmap)

  changed_files = diff_fetcher.call(root, base_revision, head_revision)
  if changed_files.empty?
    raise StatusPageFreshnessError,
          "docs/ROADMAP.md's milestone line, evidence checklist, or feature-landing " \
          "paragraphs changed between #{base_revision} and #{head_revision}, but the " \
          "base...head diff reported no changed files at all -- treating this as a " \
          "diff-base failure rather than silently passing"
  end

  touched = WATCHED_PAGES.select { |page| changed_files.include?(page) }
  if touched.empty?
    raise StatusPageFreshnessError, <<~MESSAGE.strip
      docs/ROADMAP.md's current-milestone line, a roadmap-evidence checklist entry, or a
      feature-landing paragraph changed in this diff, but neither #{WATCHED_PAGES.join(' nor ')}
      was updated in the same diff. See #{ISSUE_REFERENCE} for why the GitHub Pages status
      and landing pages must stay in sync with docs/ROADMAP.md, and #{GUIDANCE_DOC} for how
      those pages are maintained.
    MESSAGE
  end

  "roadmap milestone/evidence/feature-paragraph signal matched by an update to #{touched.join(' and ')}"
end

def main(arguments)
  if arguments.empty? || arguments.length > 3
    raise StatusPageFreshnessError,
          "usage: check_status_page_freshness.rb <base-revision> [head-revision] " \
          "[repository-root]"
  end

  base_revision = arguments[0]
  head_revision = arguments[1] || "HEAD"
  root = Pathname(arguments[2] || ".")

  result = check_status_page_freshness(root, base_revision, head_revision)
  puts "Status page freshness check passed (#{result})."
  0
rescue StatusPageFreshnessError => e
  warn e.message
  1
end

exit(main(ARGV)) if $PROGRAM_NAME == __FILE__
