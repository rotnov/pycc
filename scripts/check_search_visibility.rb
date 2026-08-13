#!/usr/bin/env ruby
# frozen_string_literal: true

# Validates the structured external-mention evidence record and launch
# readiness gate (issue #209).
#
# This validator checks structure, classification, and projection
# consistency only. It does NOT crawl third parties, sign into external
# services, post content, or send outreach.
#
# Usage: ruby scripts/check_search_visibility.rb [repository_root]
#
# Exits 0 if the evidence is well-formed and consistent, 1 otherwise.

require "json"
require "pathname"

class SearchVisibilityError < StandardError; end

REPO_ROOT = Pathname(ARGV[0] || Pathname(__dir__).parent)
EVIDENCE_PATH = REPO_ROOT / "site" / "search-visibility" / "evidence.json"
DOCS_PATH = REPO_ROOT / "docs" / "SEARCH_VISIBILITY.md"

VALID_CLASSIFICATIONS = %w[
  owned
  self_authored_external
  independent_editorial
  community
  directory
  automated_mirror
  spam
  unknown
].freeze

VALID_RESULT_TYPES = %w[rows empty_window processing unavailable].freeze

def check!
  raise SearchVisibilityError, "evidence.json not found at #{EVIDENCE_PATH}" unless EVIDENCE_PATH.exist?
  raise SearchVisibilityError, "SEARCH_VISIBILITY.md not found at #{DOCS_PATH}" unless DOCS_PATH.exist?

  evidence = JSON.parse(EVIDENCE_PATH.read)
  docs = DOCS_PATH.read

  # --- Launch gate ---
  gate = evidence["launch_gate"]
  unless gate.is_a?(Hash)
    raise SearchVisibilityError, "evidence.json must have a 'launch_gate' object"
  end

  gate_status = gate["status"]
  unless %w[closed open].include?(gate_status.to_s)
    raise SearchVisibilityError,
          "launch_gate.status must be 'closed' or 'open', got '#{gate_status}'"
  end

  # An automated agent must not open the gate without human authorization.
  if gate_status == "open" && gate["human_authorized"] != true
    raise SearchVisibilityError,
          "launch_gate is open but human_authorized is not true — " \
          "an automated agent must not open the launch gate"
  end

  # If the gate is closed, the closed_reason must be present.
  if gate_status == "closed"
    unless gate["closed_reason"] && !gate["closed_reason"].strip.empty?
      raise SearchVisibilityError,
            "launch_gate is closed but has no closed_reason"
    end
  end

  # Gate criteria must be present and non-empty.
  criteria = gate["criteria"]
  unless criteria.is_a?(Array) && !criteria.empty?
    raise SearchVisibilityError,
          "launch_gate must have a non-empty 'criteria' array"
  end

  # --- Observations ---
  observations = evidence["observations"]
  unless observations.is_a?(Array) && !observations.empty?
    raise SearchVisibilityError,
          "evidence.json must have a non-empty 'observations' array"
  end

  observed_timestamps = []
  observations.each_with_index do |obs, i|
    unless obs.is_a?(Hash)
      raise SearchVisibilityError, "observation #{i} must be an object"
    end

    observed_at = obs["observed_at"]
    unless observed_at && observed_at.is_a?(String) && !observed_at.strip.empty?
      raise SearchVisibilityError, "observation #{i} must have an 'observed_at' timestamp"
    end
    observed_timestamps << observed_at

    surface = obs["discovery_surface"]
    unless surface && surface.is_a?(String) && !surface.strip.empty?
      raise SearchVisibilityError, "observation #{i} must have a 'discovery_surface'"
    end

    query = obs["query"]
    unless query && query.is_a?(String) && !query.strip.empty?
      raise SearchVisibilityError, "observation #{i} must have a 'query'"
    end

    result_type = obs["result_type"]
    unless VALID_RESULT_TYPES.include?(result_type.to_s)
      raise SearchVisibilityError,
            "observation #{i} has invalid result_type '#{result_type}' — " \
            "must be one of #{VALID_RESULT_TYPES.join(', ')}"
    end

    result_count = obs["result_count"]
    unless result_count.is_a?(Integer) && result_count >= 0
      raise SearchVisibilityError,
            "observation #{i} result_count must be a non-negative integer"
    end

    # If result_type is 'rows', entries must be a non-empty array.
    entries = obs["entries"]
    if result_type == "rows"
      unless entries.is_a?(Array) && !entries.empty?
        raise SearchVisibilityError,
              "observation #{i} has result_type 'rows' but no entries"
      end
      if entries.length != result_count
        raise SearchVisibilityError,
              "observation #{i} result_count (#{result_count}) does not match " \
              "entries.length (#{entries.length})"
      end
    elsif result_type == "empty_window"
      if entries && !entries.empty?
        raise SearchVisibilityError,
              "observation #{i} has result_type 'empty_window' but has entries"
      end
    end

    # Validate each entry's classification.
    (entries || []).each_with_index do |entry, j|
      unless entry.is_a?(Hash)
        raise SearchVisibilityError, "observation #{i} entry #{j} must be an object"
      end

      classification = entry["classification"]
      unless VALID_CLASSIFICATIONS.include?(classification.to_s)
        raise SearchVisibilityError,
              "observation #{i} entry #{j} has invalid classification " \
              "'#{classification}' — must be one of " \
              "#{VALID_CLASSIFICATIONS.join(', ')}"
      end

      # A self-authored external reference must not be classified as
      # independent_editorial.
      if classification == "independent_editorial"
        author_rel = entry["author_relationship"]
        if author_rel == "repository_owner"
          raise SearchVisibilityError,
                "observation #{i} entry #{j} is classified as " \
                "independent_editorial but author_relationship is " \
                "repository_owner — self-authored references must not " \
                "count as independent"
        end
      end

      # An owned surface must not be classified as independent.
      if classification == "independent_editorial"
        source_url = entry["source_url"].to_s
        if source_url.include?("github.com/rotnov/pycc") ||
           source_url.include?("rotnov.github.io")
          raise SearchVisibilityError,
                "observation #{i} entry #{j} is classified as " \
                "independent_editorial but source_url is a project-owned " \
                "surface — owned surfaces must not count as independent"
        end
      end

      # first_seen and last_verified must be present.
      unless entry["first_seen"]
        raise SearchVisibilityError,
              "observation #{i} entry #{j} must have a 'first_seen' timestamp"
      end
      unless entry["last_verified"]
        raise SearchVisibilityError,
              "observation #{i} entry #{j} must have a 'last_verified' timestamp"
      end
    end
  end

  # --- Projection consistency ---
  # The docs must not claim zero backlinks as a universal fact
  # (empty windows are bounded results, not universal zeros).  Sentences
  # that say "does not prove zero backlinks" are fine — they say the
  # opposite.
  if docs =~ /there are (?:zero|0) backlinks|backlinks.*(?:do not|don't) exist|no backlinks (?:exist|remain)/i
    raise SearchVisibilityError,
          "SEARCH_VISIBILITY.md must not claim zero backlinks as fact — " \
          "empty windows are bounded results, not universal zeros"
  end

  # The docs must not describe pycc as an AI compiler.
  if docs =~ /pycc is an AI compiler|pycc is an? ML compiler/i
    raise SearchVisibilityError,
          "SEARCH_VISIBILITY.md must not describe pycc as an AI or ML compiler"
  end

  puts "Search visibility evidence and launch gate validated."
rescue SearchVisibilityError => e
  warn "Search visibility check failed: #{e.message}"
  exit 1
rescue JSON::ParserError => e
  warn "Search visibility check failed: invalid JSON: #{e.message}"
  exit 1
end

check!
