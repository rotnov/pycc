#!/usr/bin/env ruby
# frozen_string_literal: true

# Shared canonical-URL -> source-file map for the published website.
#
# This lived inside scripts/check_sitemap_lastmod.rb until issue #990 added a
# second validator (scripts/check_site_pin_merge_currency.rb) that needs the
# same mapping. That checker cannot simply `require` the older one: through
# this commit's parent, check_sitemap_lastmod.rb ran its `check!` at load
# time, so requiring it would have executed a full validation pass as a side
# effect. Extracting the constant into its own requirable file keeps exactly
# one definition of the mapping instead of two that can drift apart.
#
# This file defines constants only. It has no side effects and is never
# executed directly.

# Maps canonical URLs to their source HTML files relative to the repo root.
CANONICAL_TO_SOURCE = {
  "https://rotnov.github.io/pycc/" => "site/index.html",
  "https://rotnov.github.io/pycc/status/" => "site/status/index.html",
  "https://rotnov.github.io/pycc/architecture/" => "site/architecture/index.html",
  "https://rotnov.github.io/pycc/python-aot-compilers/" =>
    "site/python-aot-compilers/index.html",
  "https://rotnov.github.io/pycc/ai-native/" => "site/ai-native/index.html",
  "https://rotnov.github.io/pycc/language-support/" => "site/language-support/index.html",
  "https://rotnov.github.io/pycc/diagnostics/" => "site/diagnostics/index.html",
}.freeze

# The reverse mapping: source file -> canonical URL.
SOURCE_TO_CANONICAL = CANONICAL_TO_SOURCE.invert.freeze
