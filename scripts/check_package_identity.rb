#!/usr/bin/env ruby
# frozen_string_literal: true

# Validates the project's package-manager identity contract (issue #210).
#
# The public product is named "pycc", but its package-manager identity must be
# collision-safe: `pip install pycc` resolves to an unrelated foreign PyPI
# project, and `pip install pycc-compiler` resolves to a different foreign
# compiler.  The Cargo package cannot be published to crates.io because its
# workspace dependencies are path-only.  This validator enforces that no
# project-owned content makes a false or colliding package-identity claim
# before a collision-safe identity is defined and published.
#
# Invariants enforced:
#
# 1. No project-owned copy contains `pip install pycc` or
#    `pip install pycc-compiler` while those names remain foreign.
# 2. No package/index URL (pypi.org, crates.io) without published_owned
#    evidence — i.e. no claim that a package is installable from a public
#    index unless docs/DISTRIBUTION.md records that the project owns that
#    name on that index.
# 3. No crates.io publication claim while the clean exact-revision dry run
#    fails — i.e. no content may claim the Cargo package is crates.io-ready
#    while any workspace member still uses path-only internal dependencies.
# 4. No path-only internal dependency in a package declared crates.io-ready.
# 5. Consistent product/CLI/package/release/README/Pages/Markdown/llms.txt/
#    JSON-LD identities — the product name is "pycc", the CLI binary is
#    "pycc", and the PyPI distribution name is "pycc-aot".
#
# Usage: ruby scripts/check_package_identity.rb [repository_root]
#
# Exits 0 if all invariants hold, 1 otherwise.

require "pathname"
require "json"

class PackageIdentityError < StandardError; end

REPO_ROOT = Pathname(ARGV[0] || Pathname(__dir__).parent)

# The collision-safe PyPI distribution name chosen for this project.
# It does not collide with the foreign `pycc` or `pycc-compiler` PyPI packages.
PYPI_DISTRIBUTION_NAME = "pycc-aot"

# The product name and CLI binary name — these remain "pycc".
PRODUCT_NAME = "pycc"
CLI_BINARY_NAME = "pycc"

# Foreign PyPI names that must never appear in a project-owned
# `pip install` instruction.
FOREIGN_PIP_NAMES = ["pycc", "pycc-compiler"].freeze

# Files scanned for pip-install and publication claims.  Excludes
# .agents/skills/*, .claude/skills/*, seo-issues.md, and this script itself.
SCAN_GLOBS = [
  "README.md",
  "docs/**/*.md",
  "site/**/*.html",
  "site/**/*.txt",
  "site/**/*.json",
  "CITATION.cff",
  ".pre-commit-hooks.yaml"
].freeze

# Files explicitly excluded from scanning (agent skill references that
# legitimately mention pip install in unrelated contexts).
EXCLUDED_PATHS = [
  ".agents/skills/",
  ".claude/skills/",
  "seo-issues.md",
  "scripts/check_package_identity.rb",
  "scripts/test_check_package_identity.rb"
].freeze

def check!
  errors = []

  errors.concat(check_no_foreign_pip_install)
  errors.concat(check_distribution_documents_identity)
  errors.concat(check_no_unevidenced_package_url)
  errors.concat(check_no_crates_io_claim_with_path_deps)
  errors.concat(check_identity_consistency)

  if errors.empty?
    puts "Package-identity validation passed."
  else
    errors.each { |e| warn "Package-identity validation failed: #{e}" }
    exit 1
  end
rescue PackageIdentityError => e
  warn "Package-identity validation failed: #{e.message}"
  exit 1
end

# --- Helper: collect project-owned files to scan ---

def collect_scan_files
  files = []
  SCAN_GLOBS.each do |glob|
    base = REPO_ROOT
    Dir.glob(base.join(glob)).each do |path|
      p = Pathname(path)
      next unless p.file?
      rel = p.relative_path_from(base).to_s
      next if EXCLUDED_PATHS.any? { |ex| rel.start_with?(ex) }
      files << p
    end
  end
  files.uniq
end

# --- Invariant 1: no `pip install pycc` or `pip install pycc-compiler` ---

def check_no_foreign_pip_install
  errors = []
  collect_scan_files.each do |file|
    text = file.read
    FOREIGN_PIP_NAMES.each do |name|
      # Match "pip install <name>" as an install instruction, but not
      # "pip install pycc-aot" (the collision-safe name).  The negative
      # lookahead prevents matching pycc followed by a hyphen or word
      # character (e.g. pycc-aot, pycc_compiler).
      pattern = /pip\s+install\s+#{Regexp.escape(name)}(?![\w-])/i
      text.scan(pattern) do
        # Check surrounding context: if the match is inside a sentence that
        # explicitly says it resolves to a foreign/unrelated project, it is
        # acceptable (documenting the collision, not instructing it).
        line = text[/^.*#{pattern.source}.*$/i]
        next if line && line =~ /unrelated|foreign|resolves\s+to\s+(?:an?\s+)?(?:unrelated|foreign|different)/i

        rel = file.relative_path_from(REPO_ROOT).to_s
        errors << "#{rel}: contains `pip install #{name}` — this name " \
                  "resolves to a foreign PyPI project; use " \
                  "`pip install #{PYPI_DISTRIBUTION_NAME}` instead"
      end
    end
  end
  errors
end

# --- Invariant 2: docs/DISTRIBUTION.md documents the chosen identity ---

def check_distribution_documents_identity
  errors = []
  dist_path = REPO_ROOT / "docs" / "DISTRIBUTION.md"
  unless dist_path.exist?
    errors << "docs/DISTRIBUTION.md not found"
    return errors
  end

  text = dist_path.read

  # The distribution doc must mention the chosen PyPI distribution name.
  unless text =~ /#{Regexp.escape(PYPI_DISTRIBUTION_NAME)}/
    errors << "docs/DISTRIBUTION.md does not document the collision-safe " \
              "PyPI distribution name `#{PYPI_DISTRIBUTION_NAME}`"
  end

  # The distribution doc must explain why `pycc` and `pycc-compiler` are
  # not used as the pip install name (collision with foreign projects).
  unless text =~ /pip\s+install\s+pycc\b/i &&
         text =~ /unrelated|foreign|collision/i
    errors << "docs/DISTRIBUTION.md must document that `pip install pycc` " \
              "and `pip install pycc-compiler` resolve to foreign projects"
  end

  errors
end

# --- Invariant 3: no unevidenced package/index URL ---

def check_no_unevidenced_package_url
  errors = []
  dist_path = REPO_ROOT / "docs" / "DISTRIBUTION.md"
  dist_text = dist_path.exist? ? dist_path.read : ""

  # Determine whether DISTRIBUTION.md records published_owned evidence for
  # pypi.org or crates.io URLs.  Until a name is actually published, no
  # project-owned file may link to a pypi.org/project/<name> or
  # crates.io/crates/<name> URL as if the project owns it.
  pypi_owned = dist_text =~ /published.*#{PYPI_DISTRIBUTION_NAME}.*pypi/i ||
               dist_text =~ /pypi.*#{PYPI_DISTRIBUTION_NAME}.*published/i ||
               dist_text =~ /PyPI.*#{PYPI_DISTRIBUTION_NAME}.*own/i
  crates_owned = dist_text =~ /crates\.io.*publish/i &&
                 dist_text =~ /cargo\s+publish.*succeed/i

  collect_scan_files.each do |file|
    text = file.read
    rel = file.relative_path_from(REPO_ROOT).to_s

    # Check for pypi.org URLs claiming ownership
    text.scan(%r{https?://pypi\.org/project/([a-zA-Z0-9_-]+)/?}i) do |match|
      pkg = match[0]
      if pkg == PYPI_DISTRIBUTION_NAME
        # Linking to the chosen name is only OK if DISTRIBUTION.md records
        # published_owned evidence.  Until then, it's a premature claim.
        unless pypi_owned
          errors << "#{rel}: links to pypi.org/project/#{pkg} but " \
                    "DISTRIBUTION.md does not record published ownership"
        end
      elsif FOREIGN_PIP_NAMES.include?(pkg)
        # Linking to the foreign names is acceptable only in collision-
        # documentation context, not as an install instruction.
        unless text =~ /unrelated|foreign|collision/i
          errors << "#{rel}: links to pypi.org/project/#{pkg} (foreign) " \
                    "without collision-documentation context"
        end
      end
    end

    # Check for crates.io URLs claiming ownership
    text.scan(%r{https?://crates\.io/crates/([a-zA-Z0-9_-]+)/?}i) do |match|
      pkg = match[0]
      if pkg == PRODUCT_NAME
        unless crates_owned
          errors << "#{rel}: links to crates.io/crates/#{pkg} but " \
                    "DISTRIBUTION.md does not record a successful " \
                    "cargo publish dry run"
        end
      end
    end
  end
  errors
end

# --- Invariant 4: no crates.io-ready claim with path-only deps ---

def check_no_crates_io_claim_with_path_deps
  errors = []

  # Determine whether any workspace member has path-only internal deps.
  has_path_deps = workspace_has_path_only_deps?

  # Scan for crates.io-ready / cargo publish claims in project-owned files.
  collect_scan_files.each do |file|
    text = file.read
    rel = file.relative_path_from(REPO_ROOT).to_s

    # Detect positive claims that THIS project's package is crates.io-ready
    # or published.  Negative context (explaining why it is NOT ready) is
    # acceptable and must not trigger a false positive.
    crates_ready_patterns = [
      /crates\.io[\/-]ready/i,
      /cargo\s+publish\s+(?:--dry-run\s+)?(?:succeed|pass)/i,
      /published\s+to\s+crates\.io/i,
      /available\s+on\s+crates\.io/i,
      /install.*from\s+crates\.io/i
    ]

    crates_ready_patterns.each do |pattern|
      # Scan line by line so we can check surrounding context for negation.
      text.each_line do |line|
        next unless line.match?(pattern)
        # Skip lines that negate the claim (negative context is documentation,
        # not a false readiness claim) or that refer to foreign packages
        # rather than this project's own crate.
        next if line =~ /not\s+(?:yet\s+)?(?:publish|ready|available)/i ||
                line =~ /cannot\s+be\s+published/i ||
                line =~ /fails?\s+for\s+this\s+reason/i ||
                line =~ /no\s+project-owned\s+content\s+may\s+claim/i ||
                line =~ /not\s+publishable/i ||
                line =~ /would\s+fail/i ||
                line =~ /claim\s+while/i ||
                line =~ /any\s+crates\.io/i ||
                line =~ /false\s+crates\.io/i ||
                line =~ /crates\.io.*claim/i ||
                line =~ /ruff_python/i ||
                line =~ /infrequently/i
        if has_path_deps
          errors << "#{rel}: claims crates.io readiness " \
                    "(`#{pattern.source}`) but workspace members still " \
                    "have path-only internal dependencies — " \
                    "cargo publish --dry-run would fail"
        end
      end
    end
  end
  errors
end

# --- Helper: check if any workspace Cargo.toml has path-only internal deps ---

def workspace_has_path_only_deps?
  # Root Cargo.toml dependencies
  root_cargo = REPO_ROOT / "Cargo.toml"
  return true if cargo_toml_has_path_deps?(root_cargo)

  # Workspace member Cargo.toml files
  Dir.glob(REPO_ROOT.join("crates/*/Cargo.toml")).each do |path|
    return true if cargo_toml_has_path_deps?(Pathname(path))
  end

  false
end

def cargo_toml_has_path_deps?(cargo_path)
  return false unless cargo_path.exist?
  text = cargo_path.read
  # Match path = "..." in dependency declarations
  text =~ /^\s*[\w_-]+\s*=\s*\{\s*[^}]*path\s*=/
end

# --- Invariant 5: consistent identity across surfaces ---

def check_identity_consistency
  errors = []

  # Product name "pycc" must appear in README title
  readme = REPO_ROOT / "README.md"
  if readme.exist?
    readme_text = readme.read
    unless readme_text =~ /^#\s+#{Regexp.escape(PRODUCT_NAME)}/i
      errors << "README.md title does not start with `#{PRODUCT_NAME}`"
    end
  end

  # CLI binary name must be "pycc" in root Cargo.toml
  root_cargo = REPO_ROOT / "Cargo.toml"
  if root_cargo.exist?
    cargo_text = root_cargo.read
    unless cargo_text =~ /\[\[bin\]\][\s\S]*?name\s*=\s*"#{Regexp.escape(CLI_BINARY_NAME)}"/
      errors << "Cargo.toml [[bin]] name is not `#{CLI_BINARY_NAME}`"
    end
    # The Cargo package name is "pycc" (the product name) — this is fine
    # because we are not publishing to crates.io yet.  Match only the name
    # field immediately inside [package], not in [[bin]] or other sections.
    package_name = cargo_text[/^\[package\][^\[]*?name\s*=\s*"([^"]+)"/m, 1]
    if package_name != PRODUCT_NAME
      errors << "Cargo.toml [package] name is `#{package_name}`, " \
                "expected `#{PRODUCT_NAME}`"
    end
  end

  # llms.txt must reference the product name "pycc"
  llms = REPO_ROOT / "site" / "llms.txt"
  if llms.exist?
    unless llms.read =~ /^#\s+#{Regexp.escape(PRODUCT_NAME)}/i
      errors << "site/llms.txt title does not start with `#{PRODUCT_NAME}`"
    end
  end

  # JSON-LD in site/index.html must name the product "pycc"
  index = REPO_ROOT / "site" / "index.html"
  if index.exist?
    index_text = index.read
    # Extract JSON-LD blocks
    index_text.scan(/<script\s+type="application\/ld\+json">\s*(.*?)\s*<\/script>/m) do |match|
      json_text = match[0]
      begin
        data = JSON.parse(json_text)
        graph = data["@graph"] || [data]
        software = graph.find { |node| node.is_a?(Hash) && node["@type"] == "SoftwareSourceCode" }
        if software
          name = software["name"]
          if name && name != PRODUCT_NAME
            errors << "site/index.html JSON-LD SoftwareSourceCode name is " \
                      "`#{name}`, expected `#{PRODUCT_NAME}`"
          end
        end
      rescue JSON::ParserError
        # Non-JSON-LD script or malformed — skip, other validators cover this
      end
    end
  end

  errors
end

check!
