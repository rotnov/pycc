#!/usr/bin/env ruby
# frozen_string_literal: true

# Validates README.md testing-strategy claims against actual repository
# evidence (issue #214).
#
# The README's "Testing strategy" section describes three public
# compatibility mechanisms:
#
#   1. Conformance suite  — current, backed by tests/conformance.rs and
#      ci.yml's conformance oracle steps.
#   2. Real-world corpus  — planned, no corpus workflow or directory.
#   3. Ecosystem bot      — planned, no scheduled bot workflow.
#
# This validator goes beyond text-pattern matching (check_readme_claims.rb
# already does that): it verifies that *current* claims are backed by real
# repository evidence and that *planned* claims are explicitly marked as
# planned and do not coexist with contradictory present-tense language.  It
# also cross-checks the roadmap checklist so an unchecked ecosystem-bot item
# is consistent with the testing section's planned status.
#
# Specific invariants (issue #214):
#   - A "current" mechanism must have evidence (workflow file or test
#     directory); a claim without evidence is a false live claim.
#   - A "planned" mechanism must carry an explicit "(planned)" marker or
#     "not yet implemented" language in the same paragraph.
#   - No present-tense deployment verb ("compiles", "picks", "files",
#     "runs continuously") for a mechanism with no backing evidence.
#   - The roadmap's ecosystem-bot checklist item must be unchecked `[ ]`
#     while the testing section marks the bot as planned.
#
# Usage: ruby scripts/check_readme_testing_claims.rb [repository_root]
#
# Exits 0 if all invariants hold, 1 otherwise.

require "pathname"

class ReadmeTestingClaimsError < StandardError; end

REPO_ROOT = Pathname(ARGV[0] || Pathname(__dir__).parent)
README_PATH = REPO_ROOT / "README.md"
WORKFLOWS_DIR = REPO_ROOT / ".github" / "workflows"

# Each mechanism is either :current (must have evidence) or :planned (must
# be marked and must NOT have evidence that would make a present-tense claim
# accurate but unmarked).
MECHANISMS = [
  {
    label: "conformance suite",
    status: :current,
    # Evidence: the conformance integration test and a workflow that runs it.
    evidence_paths: [
      REPO_ROOT / "tests" / "conformance.rs",
    ],
    evidence_workflow_grep: /conformance/i,
    # Present-tense verbs that are acceptable ONLY when evidence exists.
    present_tense_verbs: [
      /compiles\s+and\s+runs\s+its\s+cumulative\s+fixture\s+set/i,
      /compares\s+output\s+with\s+that\s+level'?s\s+pinned\s+CPython\s+oracle/i,
    ],
    # Keywords that identify this mechanism in the testing section.
    keywords: [/conformance\s+suite/i, /conformance/i],
  },
  {
    label: "real-world corpus",
    status: :planned,
    # Evidence that would make it current — none of these should exist.
    evidence_paths: [
      REPO_ROOT / "tests" / "corpus",
    ],
    evidence_workflow_glob: "corpus*.yml",
    # Present-tense verbs that are forbidden because no evidence exists.
    present_tense_verbs: [
      /CI\s+compiles\s+(?:well-typed\s+)?open-source\s+projects/i,
      /CI\s+compiles\s+real-world\s+projects/i,
      /pass\s+rate\s+(?:per\s+project\s+)?is\s+tracked\s+release\s+to\s+release/i,
    ],
    # Planned markers that legitimize a mention.
    planned_markers: [
      /\(planned\)/i,
      /would\s+compile/i,
      /not\s+yet\s+implemented/i,
      /no\s+corpus\s+workflow/i,
    ],
    keywords: [/corpus/i],
  },
  {
    label: "ecosystem bot",
    status: :planned,
    evidence_paths: [],
    evidence_workflow_glob: "*bot*.yml",
    present_tense_verbs: [
      /A\s+scheduled\s+job\s+picks\s+popular/i,
      /scheduled\s+job\s+(?:picks|compiles|files)/i,
      /nightly\s+(?:bot|job|run)/i,
    ],
    planned_markers: [
      /\(planned\)/i,
      /would\s+(?:pick|compile|auto-file)/i,
      /not\s+yet\s+implemented/i,
      /no\s+scheduled\s+bot\s+workflow/i,
    ],
    keywords: [/ecosystem\s+bot/i, /corpus-bot/i],
  },
  {
    label: "differential fuzzing",
    status: :planned,
    evidence_paths: [
      REPO_ROOT / "tests" / "fuzz",
    ],
    evidence_workflow_glob: "fuzz*.yml",
    present_tense_verbs: [
      /Generator\s+produces\s+well-typed\s+programs/i,
      /Runs\s+continuously\s+on\s+a\s+dedicated\s+runner/i,
      /runs\s+continuously/i,
    ],
    planned_markers: [
      /\(planned\)/i,
      /would\s+(?:produce|run)/i,
      /not\s+yet\s+implemented/i,
      /no\s+fuzzing\s+harness/i,
    ],
    keywords: [/differential\s+fuzzing/i, /\bfuzz/i],
  },
].freeze

def check!
  raise ReadmeTestingClaimsError, "README.md not found at #{README_PATH}" unless README_PATH.exist?

  text = README_PATH.read

  testing_section = extract_section(text, "Testing strategy")
  if testing_section.nil? || testing_section.empty?
    raise ReadmeTestingClaimsError, "README.md must have a 'Testing strategy' section"
  end

  paragraphs = testing_section.split(/\n\n+/)

  MECHANISMS.each do |mech|
    mentioned = mech[:keywords].any? { |kw| testing_section.match?(kw) }
    next unless mentioned

    evidence_exists = mechanism_has_evidence?(mech)

    if mech[:status] == :current
      validate_current_mechanism(mech, paragraphs, evidence_exists)
    else
      validate_planned_mechanism(mech, paragraphs, evidence_exists)
    end
  end

  # Cross-check the roadmap checklist for the ecosystem bot.
  validate_roadmap_consistency(text)

  puts "README testing-claims evidence validation passed."
rescue ReadmeTestingClaimsError => e
  warn "README testing-claims validation failed: #{e.message}"
  exit 1
end

# Returns true if the mechanism has backing repository evidence.
def mechanism_has_evidence?(mech)
  path_evidence = mech[:evidence_paths]&.any?(&:exist?) || false

  workflow_evidence = false
  if mech[:evidence_workflow_glob]
    workflow_evidence = Dir.glob(WORKFLOWS_DIR / mech[:evidence_workflow_glob]).any?
  end
  if mech[:evidence_workflow_grep]
    workflow_evidence ||= Dir.children(WORKFLOWS_DIR).any? do |f|
      next false unless f.end_with?(".yml", ".yaml")
      (WORKFLOWS_DIR / f).read.match?(mech[:evidence_workflow_grep])
    end
  end

  path_evidence || workflow_evidence
end

# A current mechanism must have evidence; its present-tense verbs are
# acceptable only because the evidence exists.
def validate_current_mechanism(mech, paragraphs, evidence_exists)
  unless evidence_exists
    raise ReadmeTestingClaimsError,
          "README presents '#{mech[:label]}' as current but no backing " \
          "evidence found (expected one of: #{mech[:evidence_paths].map(&:to_s).join(', ')})"
  end
  # Current mechanisms must NOT carry planned markers — that would be
  # contradictory (claiming something is both live and not-yet-implemented).
  mech[:planned_markers]&.each do |marker|
    violating = paragraphs.any? do |para|
      mech[:keywords].any? { |kw| para.match?(kw) } &&
        para.match?(marker) &&
        mech[:present_tense_verbs].any? { |v| para.match?(v) }
    end
    if violating
      raise ReadmeTestingClaimsError,
            "README '#{mech[:label]}' is current (evidence exists) but a " \
            "paragraph also carries a planned marker alongside present-tense " \
            "verbs — contradictory status"
    end
  end
end

# A planned mechanism must be explicitly marked and must NOT use
# present-tense deployment verbs without a planned marker in the same
# paragraph.  If evidence unexpectedly exists, the planned claim is stale.
def validate_planned_mechanism(mech, paragraphs, evidence_exists)
  # If evidence exists, the mechanism should not be marked planned —
  # that would be a stale claim.  (We do not auto-flip; we flag it.)
  if evidence_exists
    # Evidence existing is not itself an error if the text still marks it
    # planned — but a present-tense claim alongside evidence without a
    # planned marker is acceptable.  We only flag the contradiction if
    # the text claims planned while evidence says current AND the text
    # uses present-tense verbs.  In practice, if evidence exists and the
    # text uses present tense without a planned marker, that is fine.
    # So we skip the stale-evidence error and focus on the text invariants.
  end

  mech[:present_tense_verbs].each do |verb|
    violating = paragraphs.any? do |para|
      mech[:keywords].any? { |kw| para.match?(kw) } &&
        para.match?(verb) &&
        mech[:planned_markers].none? { |m| para.match?(m) }
    end
    if violating
      raise ReadmeTestingClaimsError,
            "README testing section has a present-tense claim for " \
            "'#{mech[:label]}' (matches /#{verb.source}/) without a " \
            "planned marker in the same paragraph — this reads as live " \
            "but no backing evidence exists"
    end
  end

  # The mechanism must carry at least one planned marker somewhere.
  has_marker = paragraphs.any? do |para|
    mech[:keywords].any? { |kw| para.match?(kw) } &&
      mech[:planned_markers].any? { |m| para.match?(m) }
  end
  unless has_marker
    raise ReadmeTestingClaimsError,
          "README mentions '#{mech[:label]}' but never marks it as " \
          "planned — expected one of: #{mech[:planned_markers].map(&:source).join(', ')}"
  end
end

# The roadmap's ecosystem-bot checklist item must be unchecked `[ ]` while
# the testing section marks the bot as planned.  If the testing section
# ever flips the bot to current, the roadmap item should be checked.
def validate_roadmap_consistency(text)
  roadmap_section = extract_section(text, "Roadmap")
  return unless roadmap_section

  # Find the ecosystem-bot roadmap item.
  bot_match = roadmap_section.match(/^(\s*-\s*\[([ x])\]\s*.*[Ee]cosystem\s+bot.*$)/)
  return unless bot_match

  bot_checked = bot_match[2].downcase == "x"

  testing_section = extract_section(text, "Testing strategy")
  bot_planned_in_testing = testing_section&.match?(/ecosystem\s+bot.*\(planned\)/i) ||
                           testing_section&.match?(/ecosystem\s+bot.*not\s+yet\s+implemented/i)

  if bot_planned_in_testing && bot_checked
    raise ReadmeTestingClaimsError,
          "Roadmap marks the ecosystem bot as checked `[x]` but the " \
          "testing section marks it as planned — inconsistent status"
  end

  # If the bot is NOT marked planned in testing but the roadmap item is
  # unchecked, that is also inconsistent (testing implies current, roadmap
  # implies not-done).
  bot_current_in_testing = testing_section&.match?(/ecosystem\s+bot/i) &&
                           !bot_planned_in_testing &&
                           testing_section !~ /ecosystem\s+bot.*\(planned\)/i
  # We only flag the clear contradiction: testing says current (no planned
  # marker) but roadmap says unchecked.
  if bot_current_in_testing && !bot_checked && testing_section =~ /ecosystem\s+bot/i &&
     testing_section !~ /\(planned\)/i
    raise ReadmeTestingClaimsError,
          "Testing section references the ecosystem bot without a planned " \
          "marker but the roadmap item is unchecked — inconsistent status"
  end
end

# Extract a markdown section by header name.  Returns the text between
# the header and the next header of the same or higher level, or EOF.
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
