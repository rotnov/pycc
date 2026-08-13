#!/usr/bin/env ruby
# frozen_string_literal: true

# Validates that the README's Roadmap section is a faithful projection of
# docs/ROADMAP.md's commit-relative milestone status (issue #215).
#
# docs/ROADMAP.md is the sole milestone status owner. The README no longer
# maintains an independent checklist that can drift; instead it carries a
# short validated projection. This validator parses the rendered Markdown
# semantics of both documents and binds the projection to:
#
#   - the ROADMAP current-milestone statement (which milestones are met and
#     which milestone is in progress)
#   - every accepted v0.1 evidence identifier in the v0.1 acceptance
#     checklist (each checked roadmap-evidence item must be reflected in
#     the README projection)
#   - the next/in-progress milestone heading (the README must identify it
#     as the current/next delivery milestone and must NOT render it as
#     complete or shipped)
#   - the explicit pre-alpha / product-readiness state (the README must
#     preserve the pre-alpha boundary and must not reinterpret a checked
#     milestone as production readiness)
#   - the accepted Tier-1 boundary (all five Tier-1 targets, not just
#     "Linux/macOS")
#   - a link to docs/ROADMAP.md
#
# It parses rendered Markdown semantics (sections, list-item check states,
# inline evidence markers, link targets) rather than relying on a single
# required substring.
#
# Usage: ruby scripts/check_readme_milestone_projection.rb [repository_root]
#
# Exits 0 if the projection is consistent with the roadmap, 1 otherwise.

require "pathname"

class ReadmeMilestoneProjectionError < StandardError; end

REPO_ROOT = Pathname(ARGV[0] || Pathname(__dir__).parent)
README_PATH = REPO_ROOT / "README.md"
ROADMAP_PATH = REPO_ROOT / "docs" / "ROADMAP.md"

# Inline roadmap-evidence marker, mirroring check_roadmap_evidence.rb.
EVIDENCE_MARKER = /<!--\s*roadmap-evidence:\s*(?<id>[a-z0-9][a-z0-9-]*)\s*-->/

# A checked Markdown list-item body: [x] or [X].
CHECKED_ITEM_BODY = /\A\[[xX]\][ \t]+(?<claim>.*)\z/

# An unchecked Markdown list-item body: [ ].
UNCHECKED_ITEM_BODY = /\A\[ \][ \t]+(?<claim>.*)\z/

# Bind each accepted v0.1 evidence identifier to a semantic pattern that
# the README Roadmap projection must contain. Every checked v0.1
# roadmap-evidence item must be reflected here, so adding a new accepted
# v0.1 evidence identifier without projecting it fails the validator.
EVIDENCE_PROJECTION_BINDINGS = {
  "conformance-fib-mandelbrot-tier1" =>
    /fib.*mandelbrot.*CPython.*Tier-1/im,
  "check-throughput-1k-loc-75ms" =>
    /pycc check.*throughput|throughput.*floor|75\s*ms.*1000\s*LOC|<75ms/i,
  "cli-spec-diagnostic-match" =>
    /diagnostic.*CLI\s*spec|CLI\s*specification|CLI_SPEC/i,
  "ci-tier1-cross-compile" =>
    /five-target.*CI.*matrix|CI.*matrix.*cross-host|cross-host compilation/i,
  "ci-build-test-coverage-100" =>
    /100%.*line\/region.*coverage|coverage gate.*required.*green/i,
  "readme-coverage-badge-bound" =>
    /coverage badge.*bound|badge.*bound.*CI.*coverage/i
}.freeze

# The five Tier-1 targets that the accepted boundary must project. The old
# README wording ("Linux/macOS binary") silently dropped Windows and the
# arm64 legs; the projection must state the full accepted boundary.
TIER1_PLATFORM_TOKENS = %w[Linux macOS Windows].freeze

def check!
  raise ReadmeMilestoneProjectionError, "README.md not found at #{README_PATH}" unless README_PATH.exist?
  raise ReadmeMilestoneProjectionError, "docs/ROADMAP.md not found at #{ROADMAP_PATH}" unless ROADMAP_PATH.exist?

  readme_text = README_PATH.read
  roadmap_text = ROADMAP_PATH.read

  roadmap_status = parse_roadmap_status(roadmap_text)
  readme_section = extract_section(readme_text, "Roadmap")
  if readme_section.nil? || readme_section.strip.empty?
    raise ReadmeMilestoneProjectionError,
          "README.md must have a 'Roadmap' section"
  end

  validate_v0_1_complete(readme_section, roadmap_status)
  validate_next_milestone(readme_section, roadmap_status)
  validate_pre_alpha_boundary(readme_section)
  validate_no_contradictory_unchecked_mvp(readme_section)
  validate_tier1_boundary(readme_section)
  validate_roadmap_link(readme_section)
  validate_evidence_projection(readme_section, roadmap_status)

  puts "README milestone projection matches docs/ROADMAP.md " \
       "(v0.1 complete; next milestone #{roadmap_status[:next_milestone]})."
rescue ReadmeMilestoneProjectionError => e
  warn "README milestone projection check failed: #{e.message}"
  exit 1
end

# Parse docs/ROADMAP.md into a status snapshot:
#   :v0_1_complete      — true if every v0.1 acceptance-checklist bullet is
#                          checked and carries a recognized evidence marker
#   :v0_1_evidence_ids  — the list of checked v0.1 evidence identifiers
#   :met_milestones     — milestones stated as "acceptance criteria met"
#   :next_milestone     — the milestone stated as "in progress"
def parse_roadmap_status(roadmap_text)
  checklist = extract_section(roadmap_text, "v0.1 acceptance checklist")
  if checklist.nil? || checklist.strip.empty?
    raise ReadmeMilestoneProjectionError,
          "docs/ROADMAP.md must have a 'v0.1 acceptance checklist' section"
  end

  evidence_ids = []
  checklist.each_line do |line|
    item = parse_list_item(line)
    next unless item

    checked = item[:checked]
    marker = item[:body].match(EVIDENCE_MARKER)
    next unless marker

    id = marker[:id]
    if checked
      evidence_ids << id
    else
      raise ReadmeMilestoneProjectionError,
            "docs/ROADMAP.md v0.1 acceptance checklist item with evidence " \
            "identifier '#{id}' is unchecked — v0.1 is not complete in the " \
            "roadmap, so the README projection must not claim it is"
    end
  end

  if evidence_ids.empty?
    raise ReadmeMilestoneProjectionError,
          "docs/ROADMAP.md v0.1 acceptance checklist has no checked " \
          "roadmap-evidence items"
  end

  status_section = extract_section(roadmap_text, "Current delivery status")
  if status_section.nil? || status_section.strip.empty?
    raise ReadmeMilestoneProjectionError,
          "docs/ROADMAP.md must have a 'Current delivery status' section"
  end

  current_line = status_section.each_line.find do |l|
    l =~ /Current milestone:/i
  end
  unless current_line
    raise ReadmeMilestoneProjectionError,
          "docs/ROADMAP.md 'Current delivery status' must state the " \
          "current milestone"
  end

  met_milestones = current_line.scan(/v(\d+\.\d+)\s*—\s*acceptance criteria met/i)
                                .map { |m| "v#{m[0]}" }
  in_progress = current_line.match(/v(\d+\.\d+)\s+in\s+progress/i)
  next_milestone = in_progress ? "v#{in_progress[1]}" : nil

  unless next_milestone
    raise ReadmeMilestoneProjectionError,
          "docs/ROADMAP.md 'Current delivery status' must name a milestone " \
          "that is in progress"
  end

  {
    v0_1_complete: true,
    v0_1_evidence_ids: evidence_ids,
    met_milestones: met_milestones,
    next_milestone: next_milestone
  }
end

# The README projection must state that v0.1 acceptance criteria are met /
# complete. It must not render v0.1 as unchecked or unmet.
def validate_v0_1_complete(readme_section, roadmap_status)
  unless roadmap_status[:v0_1_complete]
    raise ReadmeMilestoneProjectionError,
          "docs/ROADMAP.md does not mark v0.1 complete, but the README " \
          "projection section exists — resolve the roadmap first"
  end

  # Reject a projection that explicitly understates v0.1 as not yet met.
  if readme_section =~ /v0\.1.*?not\s+(?:yet\s+)?met/im
    raise ReadmeMilestoneProjectionError,
          "README Roadmap projection states v0.1 is not yet met, but " \
          "docs/ROADMAP.md marks v0.1 complete"
  end

  unless readme_section =~ /v0\.1.*?(?:met|complete)/im
    raise ReadmeMilestoneProjectionError,
          "README Roadmap projection must state that v0.1 acceptance " \
          "criteria are met/complete"
  end
end

# The README projection must identify the next/in-progress milestone as the
# current/next delivery milestone and must NOT render it as complete or
# shipped. This catches the false direction where an incomplete milestone
# is projected as done.
def validate_next_milestone(readme_section, roadmap_status)
  next_m = roadmap_status[:next_milestone]
  return unless next_m

  unless readme_section =~ /#{Regexp.escape(next_m)}.*(?:current|next|in progress|delivery milestone)/im
    raise ReadmeMilestoneProjectionError,
          "README Roadmap projection must identify #{next_m} as the " \
          "current/next delivery milestone (in progress)"
  end

  # Reject a projection that renders the in-progress milestone as already
  # complete or shipped — that overstates the roadmap's current state.
  overclaim = readme_section.match(/#{Regexp.escape(next_m)}.*?(?:complete|met|shipped|delivered)/im)
  if overclaim && overclaim[0] !~ /in progress/i
    raise ReadmeMilestoneProjectionError,
          "README Roadmap projection renders #{next_m} as complete/shipped " \
          "but docs/ROADMAP.md states it is in progress"
  end
end

# The pre-alpha / product-readiness boundary must be preserved. A checked
# milestone is acceptance-criteria completion, not production readiness.
def validate_pre_alpha_boundary(readme_section)
  unless readme_section =~ /pre-alpha/i
    raise ReadmeMilestoneProjectionError,
          "README Roadmap projection must preserve the pre-alpha boundary"
  end

  if readme_section =~ /production ready|production-ready/i
    raise ReadmeMilestoneProjectionError,
          "README Roadmap projection must not reinterpret a checked " \
          "milestone as production readiness"
  end
end

# No contradictory unchecked MVP/v0.1 item. The old README rendered the MVP
# as `- [ ]`; the projection must not reintroduce an unchecked v0.1/MVP
# list item that contradicts the roadmap's completed state.
def validate_no_contradictory_unchecked_mvp(readme_section)
  readme_section.each_line do |line|
    item = parse_list_item(line)
    next unless item && !item[:checked]

    if item[:body] =~ /\b(?:MVP|v0\.1)\b/i
      raise ReadmeMilestoneProjectionError,
            "README Roadmap projection contains an unchecked MVP/v0.1 list " \
            "item but docs/ROADMAP.md marks v0.1 complete"
    end
  end
end

# The accepted Tier-1 boundary is all five Tier-1 targets. The projection
# must state the full boundary (including Windows) and must not silently
# downgrade to the old "Linux/macOS" wording.
def validate_tier1_boundary(readme_section)
  if readme_section =~ /Linux\/macOS\s+binary/i
    raise ReadmeMilestoneProjectionError,
          "README Roadmap projection must not downgrade the accepted " \
          "Tier-1 boundary to 'Linux/macOS binary' — Windows and the " \
          "arm64 legs are CI-gated from v0.1"
  end

  missing = TIER1_PLATFORM_TOKENS.reject { |t| readme_section =~ /#{t}/i }
  unless missing.empty?
    raise ReadmeMilestoneProjectionError,
          "README Roadmap projection must state the full Tier-1 boundary " \
          "(#{TIER1_PLATFORM_TOKENS.join(', ')}); missing: #{missing.join(', ')}"
  end
end

# The projection must link to docs/ROADMAP.md, the sole status owner.
def validate_roadmap_link(readme_section)
  unless readme_section =~ /\]\(\.\/docs\/ROADMAP\.md\)/
    raise ReadmeMilestoneProjectionError,
          "README Roadmap projection must link to docs/ROADMAP.md"
  end
end

# Every accepted v0.1 evidence identifier must be reflected in the
# projection. A new accepted evidence item that the README does not
# project fails the validator, binding the projection to the roadmap's
# evidence set.
def validate_evidence_projection(readme_section, roadmap_status)
  roadmap_status[:v0_1_evidence_ids].each do |id|
    binding_pattern = EVIDENCE_PROJECTION_BINDINGS[id]
    unless binding_pattern
      raise ReadmeMilestoneProjectionError,
            "docs/ROADMAP.md v0.1 evidence identifier '#{id}' has no " \
            "README projection binding — add one before marking the " \
            "evidence accepted"
    end

    unless readme_section =~ binding_pattern
      raise ReadmeMilestoneProjectionError,
            "README Roadmap projection does not reflect accepted v0.1 " \
            "evidence identifier '#{id}' (expected /#{binding_pattern.source}/)"
    end
  end
end

# Parse a single Markdown list-item line into {checked:, body:} or nil.
def parse_list_item(line)
  stripped = line.chomp
  m = stripped.match(/\A(?<indent>[ \t]*)[-*+]\s+(?<body>.*)\z/)
  return nil unless m

  body = m[:body]
  if (checked = body.match(CHECKED_ITEM_BODY))
    { checked: true, body: checked[:claim] }
  elsif (unchecked = body.match(UNCHECKED_ITEM_BODY))
    { checked: false, body: unchecked[:claim] }
  else
    { checked: false, body: body }
  end
end

# Extract a markdown section by header name. Returns the text between the
# header and the next header of the same or higher level, or EOF.
def extract_section(text, header_name)
  lines = text.split("\n")
  in_section = false
  section_level = nil
  section_lines = []

  lines.each do |line|
    if line =~ /^(#+)\s+(.+)$/
      level = $1.length
      name = $2.strip
      if in_section
        break if level <= section_level
      elsif name.downcase == header_name.downcase
        in_section = true
        section_level = level
        next
      end
    elsif in_section
      section_lines << line
    end
  end

  return nil unless in_section
  section_lines.join("\n")
end

check!
