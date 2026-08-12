#!/usr/bin/env ruby
# frozen_string_literal: true

# Validates that the source link registry covers every external URL
# cited in the Python AOT compiler comparison's claims.json model
# (issue #202).
#
# The registry tracks the last live-check result for each source URL.
# This validator does NOT perform network requests — it only checks
# that the registry is complete and well-formed. A separate scheduled
# workflow performs the actual live link checking and updates the
# registry.
#
# Usage: ruby scripts/check_source_links_registry.rb [repository_root]
#
# Exits 0 if the registry is complete, 1 otherwise.

require "json"
require "pathname"
require "set"

class SourceLinkRegistryError < StandardError; end

REPO_ROOT = Pathname(ARGV[0] || Pathname(__dir__).parent)
CLAIMS_PATH = REPO_ROOT / "site" / "python-aot-compilers" / "claims.json"
REGISTRY_PATH = REPO_ROOT / "site" / "python-aot-compilers" / "source-link-registry.json"

# Maximum age (in days) for a registry entry before it's considered stale.
# The scheduled workflow should re-check at least this often.
MAX_ENTRY_AGE_DAYS = 90

def extract_source_urls(claims)
  urls = Set.new
  entities = claims["entities"]
  return urls unless entities.is_a?(Array)

  entities.each do |entity|
    next unless entity.is_a?(Hash)
    entity.each_value do |value|
      case value
      when Array
        value.each do |item|
          if item.is_a?(Hash) && item["url"]
            urls.add(item["url"])
          elsif item.is_a?(String) && item.start_with?("http")
            urls.add(item)
          end
        end
      when Hash
        sources = value["sources"]
        next unless sources.is_a?(Array)
        sources.each do |src|
          if src.is_a?(Hash) && src["url"]
            urls.add(src["url"])
          elsif src.is_a?(String) && src.start_with?("http")
            urls.add(src)
          end
        end
      end
    end
  end
  urls
end

def check!
  raise SourceLinkRegistryError, "claims.json not found at #{CLAIMS_PATH}" unless CLAIMS_PATH.exist?
  raise SourceLinkRegistryError, "registry not found at #{REGISTRY_PATH}" unless REGISTRY_PATH.exist?

  claims = JSON.parse(CLAIMS_PATH.read)
  registry = JSON.parse(REGISTRY_PATH.read)

  # Extract all external source URLs from claims.json
  claims_urls = extract_source_urls(claims)
  if claims_urls.empty?
    raise SourceLinkRegistryError, "claims.json contains no source URLs"
  end

  # Build a lookup of registry entries
  entries = registry["entries"]
  unless entries.is_a?(Array)
    raise SourceLinkRegistryError, "registry must have an 'entries' array"
  end

  registry_urls = {}
  entries.each do |entry|
    unless entry.is_a?(Hash)
      raise SourceLinkRegistryError, "each registry entry must be an object"
    end
    url = entry["url"]
    unless url
      raise SourceLinkRegistryError, "each registry entry must have a 'url'"
    end
    registry_urls[url] = entry
  end

  # Check that every claims URL has a registry entry
  missing = claims_urls.to_a - registry_urls.keys
  unless missing.empty?
    raise SourceLinkRegistryError,
          "registry is missing entries for #{missing.length} source URL(s): " \
          "#{missing.sort.first(3).join(', ')}"
  end

  # Check that no registry entry has status "broken"
  registry_urls.each do |url, entry|
    status = entry["status"]
    if status == "broken"
      raise SourceLinkRegistryError,
            "registry entry for #{url} has status 'broken' — " \
            "the source link is known to be broken and must be updated"
    end
    unless %w[ok broken redirect].include?(status.to_s)
      raise SourceLinkRegistryError,
            "registry entry for #{url} has invalid status '#{status}' — " \
            "must be 'ok', 'broken', or 'redirect'"
    end
    last_checked = entry["last_checked"]
    unless last_checked
      raise SourceLinkRegistryError,
            "registry entry for #{url} must have a 'last_checked' date"
    end
  end

  # Check for extra registry entries (URLs no longer in claims.json)
  extra = registry_urls.keys - claims_urls.to_a
  unless extra.empty?
    raise SourceLinkRegistryError,
          "registry has #{extra.length} entry/entries for URLs no longer in " \
          "claims.json: #{extra.sort.first(3).join(', ')}"
  end

  puts "Source link registry covers all #{claims_urls.length} claims.json URLs."
rescue SourceLinkRegistryError => e
  warn "Source link registry check failed: #{e.message}"
  exit 1
rescue JSON::ParserError => e
  warn "Source link registry check failed: invalid JSON: #{e.message}"
  exit 1
end

check!
