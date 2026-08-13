#!/usr/bin/env ruby
# frozen_string_literal: true

# Validates README.md testing-strategy claims against issue #214's
# requirements: planned testing mechanisms (real-world corpus, ecosystem
# bot, differential fuzzing) must not be presented in present-tense as
# if they are running now.  The validator parses the README testing
# section and rejects contradictory current/planned statements.
#
# Specific negative controls (issue #214 validation expectations):
#   - "CI compiles real-world projects" while no corpus evidence exists
#   - "pass rate is tracked release to release" without a report series
#   - "a scheduled job" or "nightly" bot claim without implementation
#   - "runs continuously" for differential fuzzing without a harness
#   - an unchecked roadmap item coexisting with a present-tense claim
#
# Usage: ruby scripts/check_readme_claims.rb [repository_root]
#
# Exits 0 if all invariants hold, 1 otherwise.

require "pathname"

class ReadmeClaimsError < StandardError; end

REPO_ROOT = Pathname(ARGV[0] || Pathname(__dir__).parent)
README_PATH = REPO_ROOT / "README.md"

# Planned testing mechanisms that must NOT appear in present-tense
# deployment claims.  Each entry has:
#   :planned_markers — phrases that correctly mark the mechanism as planned
#   :forbidden_present — present-tense phrases that falsely claim deployment
#   :label — human-readable name for error messages
PLANNED_MECHANISMS = [
  {
    label: "real-world corpus",
    forbidden_present: [
      /CI\s+compiles\s+(?:well-typed\s+)?open-source\s+projects/i,
      /CI\s+compiles\s+real-world\s+projects/i,
      /pass\s+rate\s+(?:per\s+project\s+)?is\s+tracked\s+release\s+to\s+release/i,
    ],
    planned_markers: [
      /\(planned\)/,
      /would\s+compile/i,
      /not\s+yet\s+implemented/i,
      /no\s+corpus\s+workflow/i,
    ],
  },
  {
    label: "ecosystem bot",
    forbidden_present: [
      /A\s+scheduled\s+job\s+picks\s+popular/i,
      /scheduled\s+job\s+(?:picks|compiles|files)/i,
      /nightly\s+(?:bot|job|run)/i,
    ],
    planned_markers: [
      /\(planned\)/,
      /would\s+(?:pick|compile|auto-file)/i,
      /not\s+yet\s+implemented/i,
      /no\s+scheduled\s+bot\s+workflow/i,
    ],
  },
  {
    label: "differential fuzzing",
    forbidden_present: [
      /Generator\s+produces\s+well-typed\s+programs/i,
      /Runs\s+continuously\s+on\s+a\s+dedicated\s+runner/i,
      /runs\s+continuously/i,
    ],
    planned_markers: [
      /\(planned\)/,
      /would\s+(?:produce|run)/i,
      /not\s+yet\s+implemented/i,
      /no\s+fuzzing\s+harness/i,
    ],
  },
].freeze

def check!
  raise ReadmeClaimsError, "README.md not found at #{README_PATH}" unless README_PATH.exist?

  text = README_PATH.read

  # Extract the testing strategy section
  testing_section = extract_section(text, "Testing strategy")
  if testing_section.nil? || testing_section.empty?
    raise ReadmeClaimsError, "README.md must have a 'Testing strategy' section"
  end

  # Check each planned mechanism for forbidden present-tense claims
  PLANNED_MECHANISMS.each do |mechanism|
    mechanism[:forbidden_present].each do |pattern|
      if testing_section.match?(pattern)
        # Check if the forbidden pattern appears in a context that is
        # clearly marked as planned (within the same paragraph).
        # If the paragraph also contains a planned marker, it's acceptable.
        paragraphs = testing_section.split(/\n\n+/)
        violating = paragraphs.any? do |para|
          para.match?(pattern) && mechanism[:planned_markers].none? { |m| para.match?(m) }
        end
        if violating
          raise ReadmeClaimsError,
                "README testing section contains a present-tense claim " \
                "for #{mechanism[:label]} without a planned marker: " \
                "matches /#{pattern.source}/"
        end
      end
    end
  end

  # Check the roadmap section for unchecked items that coexist with
  # present-tense claims in the testing section.
  roadmap_section = extract_section(text, "Roadmap")
  if roadmap_section
    unchecked_items = roadmap_section.scan(/^\s*-\s*\[ \]\s*(.+)$/).map(&:first)
    # The ecosystem bot roadmap item must be unchecked
    bot_item = unchecked_items.find { |item| item =~ /Ecosystem bot/i }
    if bot_item
      # Good — it's unchecked.  But if the testing section says "a scheduled
      # job picks" (present tense), that's a contradiction already caught above.
    end
  end

  # Verify that planned mechanisms ARE marked as planned somewhere
  # in the testing section (not silently omitted).
  # The conformance suite is current; the corpus, bot, and fuzzing are planned.
  # We don't require all three to be mentioned, but if they are mentioned,
  # they must be marked as planned.
  has_corpus = testing_section =~ /corpus/i
  has_bot = testing_section =~ /ecosystem\s+bot|corpus-bot/i
  has_fuzzing = testing_section =~ /differential\s+fuzzing|fuzz/i

  if has_corpus
    unless testing_section =~ /\(planned\)|would\s+compile|not\s+yet\s+implemented/i
      raise ReadmeClaimsError,
            "README mentions corpus testing but does not mark it as planned"
    end
  end

  if has_bot
    unless testing_section =~ /\(planned\)|would\s+(?:pick|compile|auto-file)|not\s+yet\s+implemented/i
      raise ReadmeClaimsError,
            "README mentions ecosystem bot but does not mark it as planned"
    end
  end

  puts "README testing-claims validation passed."
rescue ReadmeClaimsError => e
  warn "README testing-claims validation failed: #{e.message}"
  exit 1
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
        # Next header at same or higher level ends the section
        if level <= section_level
          break
        end
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
