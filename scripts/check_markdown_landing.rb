#!/usr/bin/env ruby
# frozen_string_literal: true

# Validates that site/index.html.md is a faithful Markdown projection of
# site/index.html — the "clean text equivalent" of the landing page (issue #206).
#
# The Markdown file is publicly described (in site/llms.txt and docs/WEBSITE.md)
# as the clean text equivalent of the landing page, but the existing
# check-site.sh validator only checked a handful of scattered facts. It had no
# contract preventing one-sided semantic drift: the Markdown could omit entire
# prominent visible sections (the design-contract pillars, the "why pycc"
# rationale, the comparison table, the compiler pipeline, the final call to
# action) while the HTML kept them.
#
# This validator binds the Markdown to the HTML page's semantic contract. It
# defines a SECTION_CONTRACT table: each prominent visible section of the HTML
# page is paired with (a) the HTML heading or anchor text that anchors it and
# (b) a set of marker phrases that must appear in the Markdown. The validator
# first confirms each HTML anchor still exists in site/index.html (so the
# contract stays anchored to the real page and cannot silently describe a
# section that was removed from the HTML), then confirms each Markdown marker
# is present in site/index.html.md (so the Markdown cannot silently drop a
# section the HTML still shows).
#
# Usage: ruby scripts/check_markdown_landing.rb [repository_root]
#
# Exits 0 if the Markdown covers every contracted section, 1 otherwise.

require "pathname"

class MarkdownLandingError < StandardError; end

REPO_ROOT = Pathname(ARGV[0] || Pathname(__dir__).parent)
HTML_PATH = REPO_ROOT / "site" / "index.html"
MARKDOWN_PATH = REPO_ROOT / "site" / "index.html.md"

# Each entry maps a section name to:
#   :html_anchor  — a substring that must appear in site/index.html, anchoring
#                   the contract to a real visible section of the HTML page.
#   :md_markers   — substrings that must appear in site/index.html.md, proving
#                   the Markdown projects that section's key content.
#
# The anchors are drawn from the HTML page's visible headings and prominent
# content blocks. The markers are the distinctive phrases a reader would
# associate with each section. If a section is added to the HTML page, its
# anchor must be added here and its markers projected into the Markdown; if a
# section is removed from the HTML, the contract entry is removed here and the
# Markdown markers removed in the same change. Either side drifting alone
# fails the validator.
SECTION_CONTRACT = {
  "hero" => {
    html_anchor: "Typed Python in.",
    md_heading: nil,
    md_markers: [
      "Typed Python in. Autonomous artifacts out.",
      "pycc build hello.py -o hello",
    ],
  },
  "design contract" => {
    html_anchor: "Standard Python target",
    md_heading: "## Design contract",
    md_markers: [
      "Standard Python target",
      "Compile-time types",
      "Standalone output",
      "Python syntax stays Python",
    ],
  },
  "development model" => {
    html_anchor: "Built entirely by AI.",
    md_heading: "## Development model",
    md_markers: [
      "Built entirely by AI. Managed by a human.",
      "No project code is handwritten by a human.",
    ],
  },
  "why pycc" => {
    html_anchor: "One tool should finish the whole job.",
    md_heading: "## Why pycc",
    md_markers: [
      "One tool should finish the whole job.",
      "Type checkers stop before runtime.",
      "Annotations are the contract",
    ],
  },
  "comparison table" => {
    html_anchor: "The intended position",
    md_heading: "## The intended position",
    md_markers: [
      "The intended position",
      "Design targets, not release claims",
      "LPython",
      "Codon",
      "Nuitka",
      "mypyc",
    ],
  },
  "compiler pipeline" => {
    html_anchor: "From source file to machine code.",
    md_heading: "## Compiler pipeline",
    md_markers: [
      "From source file to machine code.",
      "Parse",
      "Check",
      "Lower",
      "Compile",
    ],
  },
  "honest status" => {
    html_anchor: "v0.1 through v0.3 are met and released as",
    md_heading: "## Honest status",
    md_markers: [
      "v0.1 through v0.3 are met and released as",
      "v0.1 acceptance criteria met",
      "v0.2 acceptance criteria also met",
      "v0.3 acceptance criteria met and released as",
      "v0.4 is in progress. Cross-file project from imports have landed. Bare/submodule imports, namespace handling, broader project CLI behavior, and incremental compilation remain incomplete.",
    ],
  },
  "evidence pages" => {
    html_anchor: "Current evidence",
    md_heading: "## Evidence pages",
    md_markers: [
      "Current implementation status",
      "Compiler architecture",
      "Python AOT compiler comparison",
      "AI-native experiment",
    ],
  },
  "final call to action" => {
    html_anchor: "Follow an AI-built Python compiler from first slice to production.",
    md_heading: "## Follow the project",
    md_markers: [
      "Follow an AI-built Python compiler from first slice to production.",
    ],
  },
}.freeze

def check!
  raise MarkdownLandingError, "index.html not found at #{HTML_PATH}" unless HTML_PATH.exist?
  raise MarkdownLandingError, "index.html.md not found at #{MARKDOWN_PATH}" unless MARKDOWN_PATH.exist?

  html = HTML_PATH.read
  markdown = MARKDOWN_PATH.read

  # The Markdown must start with the canonical page title, matching the HTML
  # <title>. This is the first line of the semantic contract.
  expected_title = "# pycc — AOT compiler for typed Python to native binaries"
  unless markdown.start_with?(expected_title)
    raise MarkdownLandingError,
          "index.html.md must start with the canonical page title"
  end

  SECTION_CONTRACT.each do |section, contract|
    anchor = contract[:html_anchor]
    unless html.include?(anchor)
      raise MarkdownLandingError,
            "Section #{section.inspect} HTML anchor #{anchor.inspect} is " \
            "missing from site/index.html — the contract no longer matches " \
            "the live HTML page (issue #206)"
    end

    heading = contract[:md_heading]
    if heading && !markdown.include?(heading)
      raise MarkdownLandingError,
            "Section #{section.inspect} is missing its Markdown heading " \
            "#{heading.inspect} in site/index.html.md — the Markdown " \
            "landing must project this prominent visible section of the " \
            "HTML page (issue #206)"
    end

    contract[:md_markers].each do |marker|
      unless markdown.include?(marker)
        raise MarkdownLandingError,
              "Section #{section.inspect} is missing Markdown marker " \
              "#{marker.inspect} in site/index.html.md — the Markdown " \
              "landing must project this prominent visible section of the " \
              "HTML page (issue #206)"
      end
    end
  end

  # The Markdown must not claim production readiness, matching the HTML page's
  # pre-alpha status (mirrors the check-site.sh invariant, kept here so this
  # standalone validator is self-contained).
  md_lower = markdown.downcase
  ["production-ready", "production ready", "stable release"].each do |false_claim|
    if md_lower.include?(false_claim)
      raise MarkdownLandingError,
            "index.html.md must not claim #{false_claim.inspect} — " \
            "pycc is pre-alpha (issue #206)"
    end
  end

  puts "Markdown landing covers all contracted HTML page sections."
rescue MarkdownLandingError => e
  warn "Markdown landing check failed: #{e.message}"
  exit 1
end

check!
