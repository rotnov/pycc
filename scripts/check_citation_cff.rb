#!/usr/bin/env ruby
# frozen_string_literal: true

# Validates the root CITATION.cff against issue #212's requirements:
#   - valid YAML conforming to CFF 1.2.0
#   - required fields: cff-version, message, title, authors, type
#   - repository-code must be the exact rotnov/pycc GitHub URL
#   - url must be the canonical project landing page
#   - license must be MIT
#   - type must be "software"
#   - no "AI compiler" in title, abstract, or keywords
#   - authors must be a collective entity, not a fabricated human author
#   - no version, date-released, commit, doi, or preferred-citation
#     (release lifecycle is incoherent per #196)
#   - no .zenodo.json present (Zenodo is a separate approved step)
#
# Usage: ruby scripts/check_citation_cff.rb [repository_root]
#
# Exits 0 if all invariants hold, 1 otherwise.

require "pathname"
require "yaml"

class CitationError < StandardError; end

REPO_ROOT = Pathname(ARGV[0] || Pathname(__dir__).parent)
CITATION_PATH = REPO_ROOT / "CITATION.cff"
ZENODO_PATH = REPO_ROOT / ".zenodo.json"

EXPECTED_REPO = "https://github.com/rotnov/pycc"
EXPECTED_URL = "https://rotnov.github.io/pycc/"
EXPECTED_LICENSE = "MIT"
EXPECTED_CFF_VERSION = "1.2.0"
EXPECTED_TYPE = "software"

# Fields that must NOT be present until the release lifecycle is coherent (#196).
FORBIDDEN_RELEASE_FIELDS = %w[
  version
  date-released
  commit
  doi
  preferred-citation
].freeze

def check!
  raise CitationError, "CITATION.cff not found at #{CITATION_PATH}" unless CITATION_PATH.exist?

  text = CITATION_PATH.read
  cff = YAML.safe_load(text, aliases: true)

  raise CitationError, "CITATION.cff must be valid YAML" unless cff.is_a?(Hash)

  # --- Required fields ---
  unless cff["cff-version"] == EXPECTED_CFF_VERSION
    raise CitationError,
          "cff-version must be #{EXPECTED_CFF_VERSION}, got: #{cff['cff-version'].inspect}"
  end

  unless cff["message"].is_a?(String) && !cff["message"].empty?
    raise CitationError, "message must be a non-empty string"
  end

  unless cff["title"].is_a?(String) && !cff["title"].empty?
    raise CitationError, "title must be a non-empty string"
  end

  unless cff["type"] == EXPECTED_TYPE
    raise CitationError,
          "type must be #{EXPECTED_TYPE}, got: #{cff['type'].inspect}"
  end

  authors = cff["authors"]
  unless authors.is_a?(Array) && !authors.empty?
    raise CitationError, "authors must be a non-empty array"
  end

  # --- Repository identity ---
  unless cff["repository-code"] == EXPECTED_REPO
    raise CitationError,
          "repository-code must be #{EXPECTED_REPO}, got: #{cff['repository-code'].inspect}"
  end

  # --- Landing URL ---
  unless cff["url"] == EXPECTED_URL
    raise CitationError,
          "url must be #{EXPECTED_URL}, got: #{cff['url'].inspect}"
  end

  # --- License ---
  unless cff["license"] == EXPECTED_LICENSE
    raise CitationError,
          "license must be #{EXPECTED_LICENSE}, got: #{cff['license'].inspect}"
  end

  # --- No "AI compiler" in product semantics ---
  full_text = text.downcase
  if full_text.include?("ai compiler")
    raise CitationError,
          "CITATION.cff must not describe pycc as an 'AI compiler'"
  end

  # --- Authorship: collective entity, not a fabricated human ---
  authors.each do |author|
    if author.is_a?(Hash)
      name = author["name"].to_s
      if name.empty?
        raise CitationError, "each author must have a name"
      end
      # A human person author would have given-names/family-names.
      # The collective entity should not.
      if author["given-names"] || author["family-names"]
        raise CitationError,
              "author #{name.inspect} has given-names/family-names — " \
              "human software authorship is not attributed without an " \
              "accepted project decision (issue #212)"
      end
      # The author must have a description explaining the collective entity.
      unless author["description"].is_a?(String) && !author["description"].empty?
        raise CitationError,
              "author #{name.inspect} must have a description explaining " \
              "the collective entity (issue #212)"
      end
    else
      raise CitationError, "each author must be a mapping with a name"
    end
  end

  # --- No release-bound fields until #196 resolves ---
  FORBIDDEN_RELEASE_FIELDS.each do |field|
    if cff.key?(field)
      raise CitationError,
            "CITATION.cff must not include '#{field}' until the release " \
            "lifecycle is coherent (issue #196)"
    end
  end

  # --- No .zenodo.json (Zenodo is a separate approved step) ---
  if ZENODO_PATH.exist?
    raise CitationError,
          ".zenodo.json must not be present — Zenodo integration is a " \
          "separate, explicitly approved step (issue #212)"
  end

  # --- Keywords must describe a typed-Python AOT compiler ---
  keywords = cff["keywords"]
  if keywords
    unless keywords.is_a?(Array)
      raise CitationError, "keywords must be an array if present"
    end
    kw_text = keywords.join(" ").downcase
    unless kw_text.include?("python") && kw_text.include?("compiler")
      raise CitationError,
            "keywords must describe a Python compiler"
    end
    if kw_text.include?("ai compiler") || kw_text.include?("machine learning")
      raise CitationError,
            "keywords must not describe pycc as an AI or ML compiler"
    end
  end

  puts "CITATION.cff validation passed."
rescue CitationError => e
  warn "CITATION.cff validation failed: #{e.message}"
  exit 1
rescue Psych::SyntaxError => e
  warn "CITATION.cff validation failed: invalid YAML: #{e.message}"
  exit 1
end

check!
