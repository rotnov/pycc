#!/usr/bin/env ruby
# frozen_string_literal: true

require "open3"
require "pathname"

class StatusPageFreshnessError < StandardError; end

# Reuse the marker convention scripts/check_roadmap_evidence.rb already
# established for docs/ROADMAP.md instead of reinventing marker parsing.
EVIDENCE_MARKER = /<!--\s*roadmap-evidence:\s*(?<id>[a-z0-9][a-z0-9-]*)\s*-->/
CHECKBOX_PREFIX = /\A\s*-\s*\[(?<mark>.)\]/
# Deliberately not /m: the real docs/ROADMAP.md milestone span never wraps
# past its own physical line, and without /m a malformed span (a stray
# "**Current milestone:" with no closing "**" before EOF) simply fails to
# match instead of `.` running across unrelated later bold text -- nil vs. a
# real span is itself a correct "the milestone line changed" signal.
MILESTONE_SPAN = /\*\*Current milestone:.*?\*\*/

ROADMAP_PATH = "docs/ROADMAP.md"
WATCHED_PAGES = %w[site/status/index.html site/index.html].freeze
ISSUE_REFERENCE = "https://github.com/rotnov/pycc/issues/401"
GUIDANCE_DOC = "docs/WEBSITE.md"

def run_git(root, args, context)
  stdout, stderr, status = Open3.capture3("git", *args, chdir: root.to_s)
  raise StatusPageFreshnessError, "#{context}: #{stderr.strip}" unless status.success?

  stdout.force_encoding(Encoding::UTF_8)
end

# Make sure `revision` is resolvable as a commit in `root`'s repository.
# A shallow CI checkout only has the PR's own history, so the base
# revision usually needs an explicit fetch; a local full-history checkout
# (used by this script's own tests and by manual validation runs) already
# has it and should not need network access.
def ensure_revision_available(root, revision)
  _stdout, _stderr, status = Open3.capture3(
    "git", "cat-file", "-e", "#{revision}^{commit}", chdir: root.to_s
  )
  return if status.success?

  _stdout, stderr, fetch_status = Open3.capture3(
    "git", "fetch", "--no-tags", "--depth=1", "origin", revision, chdir: root.to_s
  )
  return if fetch_status.success?

  raise StatusPageFreshnessError,
        "could not resolve revision #{revision.inspect} locally or via " \
        "'git fetch origin #{revision} --depth=1': #{stderr.strip}"
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

def milestone_span(text)
  match = MILESTONE_SPAN.match(text)
  match && match[0]
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

def roadmap_signal?(base_text, head_text)
  milestone_span(base_text) != milestone_span(head_text) ||
    evidence_checklist_states(base_text) != evidence_checklist_states(head_text)
end

def check_status_page_freshness(root, base_revision, head_revision, diff_fetcher: method(:diff_name_only))
  ensure_revision_available(root, base_revision)
  ensure_revision_available(root, head_revision)

  base_roadmap = read_file_at_revision(root, base_revision, ROADMAP_PATH)
  head_roadmap = read_file_at_revision(root, head_revision, ROADMAP_PATH)

  return "no roadmap milestone or evidence-checklist signal" unless roadmap_signal?(base_roadmap, head_roadmap)

  changed_files = diff_fetcher.call(root, base_revision, head_revision)
  if changed_files.empty?
    raise StatusPageFreshnessError,
          "docs/ROADMAP.md's milestone line or evidence checklist changed between " \
          "#{base_revision} and #{head_revision}, but the base...head diff reported no " \
          "changed files at all -- treating this as a diff-base failure rather than " \
          "silently passing"
  end

  touched = WATCHED_PAGES.select { |page| changed_files.include?(page) }
  if touched.empty?
    raise StatusPageFreshnessError, <<~MESSAGE.strip
      docs/ROADMAP.md's current-milestone line or a roadmap-evidence checklist entry
      changed in this diff, but neither #{WATCHED_PAGES.join(' nor ')} was updated in
      the same diff. See #{ISSUE_REFERENCE} for why the GitHub Pages status and landing
      pages must stay in sync with docs/ROADMAP.md, and #{GUIDANCE_DOC} for how those
      pages are maintained.
    MESSAGE
  end

  "roadmap milestone/evidence signal matched by an update to #{touched.join(' and ')}"
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
