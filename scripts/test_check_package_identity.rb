#!/usr/bin/env ruby
# frozen_string_literal: true

# Test suite for scripts/check_package_identity.rb — validates the
# package-identity contract from issue #210 with positive and negative
# (mutation) controls.

require "minitest/autorun"
require "pathname"
require "rbconfig"
require "tmpdir"

REPO_ROOT = Pathname(__dir__).parent
CHECKER = REPO_ROOT / "scripts" / "check_package_identity.rb"

class TestCheckPackageIdentity < Minitest::Test
  # Run the checker against a temporary repository root that mirrors the
  # minimal file set the checker inspects.
  def run_checker(files)
    Dir.mktmpdir do |dir|
      root = Pathname(dir)
      files.each do |rel, content|
        path = root / rel
        path.dirname.mkpath
        path.write(content)
      end
      output = `#{RbConfig.ruby} #{CHECKER} #{root} 2>&1`
      return $?.exitstatus, output
    end
  end

  # A minimal valid repository that passes all invariants.
  def valid_files
    {
      "README.md" => "# pycc — ahead-of-time compiler\n\nSome content.\n",
      "Cargo.toml" => <<~TOML,
        [package]
        name = "pycc"
        version = "0.1.0"

        [[bin]]
        name = "pycc"
        path = "src/main.rs"

        [dependencies]
        pycc_parser = { path = "crates/pycc_parser" }
      TOML
      "docs/DISTRIBUTION.md" => <<~MD,
        # pycc Distribution

        ## Package identity (issue #210)

        `pip install pycc` resolves to an unrelated foreign PyPI project.
        `pip install pycc-compiler` resolves to a different foreign compiler.

        The collision-safe PyPI distribution name is `pycc-aot`. It is not
        yet registered or published.

        The Cargo package cannot be published to crates.io because its
        workspace dependencies are path-only. No project-owned content may
        claim crates.io readiness.
      MD
      "site/llms.txt" => "# pycc\n\n> pycc is a compiler.\n",
      "site/index.html" => <<~HTML,
        <html>
        <script type="application/ld+json">
        {
          "@context": "https://schema.org",
          "@graph": [
            {
              "@type": "SoftwareSourceCode",
              "name": "pycc",
              "url": "https://rotnov.github.io/pycc/"
            }
          ]
        }
        </script>
        </html>
      HTML
    }
  end

  # --- Positive: the live repository passes ---

  def test_live_repo_passes
    output = `#{RbConfig.ruby} #{CHECKER} #{REPO_ROOT} 2>&1`
    assert_equal 0, $?.exitstatus,
                 "live repository failed:\n#{output}"
  end

  # --- Positive: minimal valid repo passes ---

  def test_minimal_valid_repo_passes
    status, output = run_checker(valid_files)
    assert_equal 0, status, "minimal valid repo failed:\n#{output}"
  end

  # --- Mutation: pip install pycc in README ---

  def test_rejects_pip_install_pycc_in_readme
    files = valid_files.dup
    files["README.md"] = "# pycc\n\nInstall with `pip install pycc`.\n"
    status, output = run_checker(files)
    refute_equal 0, status, "accepted `pip install pycc` in README"
    assert_match(/pip install pycc/, output)
  end

  # --- Mutation: pip install pycc-compiler in README ---

  def test_rejects_pip_install_pycc_compiler_in_readme
    files = valid_files.dup
    files["README.md"] = "# pycc\n\nInstall with `pip install pycc-compiler`.\n"
    status, output = run_checker(files)
    refute_equal 0, status, "accepted `pip install pycc-compiler` in README"
    assert_match(/pip install pycc-compiler/, output)
  end

  # --- Mutation: pip install pycc with version specifier ---

  def test_rejects_pip_install_pycc_with_version
    files = valid_files.dup
    files["README.md"] = "# pycc\n\n`pip install pycc>=0.1`\n"
    status, = run_checker(files)
    refute_equal 0, status, "accepted `pip install pycc>=0.1`"
  end

  # --- Positive: pip install pycc-aot is accepted ---

  def test_accepts_pip_install_pycc_aot
    files = valid_files.dup
    files["README.md"] = "# pycc\n\nInstall with `pip install pycc-aot`.\n"
    status, = run_checker(files)
    assert_equal 0, status, "rejected `pip install pycc-aot`"
  end

  # --- Positive: documenting the collision is accepted ---

  def test_accepts_collision_documentation
    files = valid_files.dup
    files["README.md"] =
      "# pycc\n\nNote: `pip install pycc` resolves to an unrelated project.\n"
    status, = run_checker(files)
    assert_equal 0, status,
                 "rejected collision documentation in README"
  end

  # --- Mutation: DISTRIBUTION.md missing pycc-aot ---

  def test_rejects_distribution_missing_pycc_aot
    files = valid_files.dup
    files["docs/DISTRIBUTION.md"] =
      "# pycc Distribution\n\n`pip install pycc` resolves to an unrelated " \
      "foreign project.\n"
    status, output = run_checker(files)
    refute_equal 0, status, "accepted DISTRIBUTION.md without pycc-aot"
    assert_match(/pycc-aot/, output)
  end

  # --- Mutation: DISTRIBUTION.md missing collision explanation ---

  def test_rejects_distribution_missing_collision_explanation
    files = valid_files.dup
    files["docs/DISTRIBUTION.md"] =
      "# pycc Distribution\n\nThe PyPI distribution name is `pycc-aot`.\n"
    status, output = run_checker(files)
    refute_equal 0, status,
                 "accepted DISTRIBUTION.md without collision explanation"
    assert_match(/pip install pycc/, output)
  end

  # --- Mutation: DISTRIBUTION.md missing entirely ---

  def test_rejects_missing_distribution
    files = valid_files.dup
    files.delete("docs/DISTRIBUTION.md")
    status, = run_checker(files)
    refute_equal 0, status, "accepted missing DISTRIBUTION.md"
  end

  # --- Mutation: crates.io-ready claim with path deps ---

  def test_rejects_crates_io_ready_claim_with_path_deps
    files = valid_files.dup
    files["README.md"] =
      "# pycc\n\nThe package is now crates.io-ready.\n"
    status, output = run_checker(files)
    refute_equal 0, status,
                 "accepted crates.io-ready claim with path-only deps"
    assert_match(/crates\.io/, output)
  end

  # --- Mutation: "published to crates.io" claim about this project ---

  def test_rejects_published_to_crates_io_claim
    files = valid_files.dup
    files["README.md"] =
      "# pycc\n\npycc is published to crates.io.\n"
    status, output = run_checker(files)
    refute_equal 0, status,
                 "accepted 'published to crates.io' claim with path deps"
    assert_match(/crates\.io/, output)
  end

  # --- Positive: negative context is accepted ---

  def test_accepts_negative_crates_io_context
    files = valid_files.dup
    files["README.md"] =
      "# pycc\n\nThe package is not yet publishable to crates.io.\n"
    status, = run_checker(files)
    assert_equal 0, status,
                 "rejected negative crates.io context"
  end

  # --- Mutation: unevidenced pypi.org URL ---

  def test_rejects_unevidenced_pypi_url
    files = valid_files.dup
    files["README.md"] =
      "# pycc\n\nInstall from https://pypi.org/project/pycc-aot/\n"
    status, output = run_checker(files)
    refute_equal 0, status, "accepted unevidenced pypi.org URL"
    assert_match(/pypi\.org/, output)
  end

  # --- Mutation: unevidenced crates.io URL ---

  def test_rejects_unevidenced_crates_io_url
    files = valid_files.dup
    files["README.md"] =
      "# pycc\n\nInstall from https://crates.io/crates/pycc\n"
    status, output = run_checker(files)
    refute_equal 0, status, "accepted unevidenced crates.io URL"
    assert_match(/crates\.io/, output)
  end

  # --- Mutation: README title not pycc ---

  def test_rejects_wrong_readme_title
    files = valid_files.dup
    files["README.md"] = "# not-pycc\n\nSome content.\n"
    status, output = run_checker(files)
    refute_equal 0, status, "accepted README with wrong title"
    assert_match(/README.*title/, output)
  end

  # --- Mutation: CLI binary name not pycc ---

  def test_rejects_wrong_cli_binary_name
    files = valid_files.dup
    files["Cargo.toml"] = <<~TOML
      [package]
      name = "pycc"
      version = "0.1.0"

      [[bin]]
      name = "not-pycc"
      path = "src/main.rs"

      [dependencies]
      pycc_parser = { path = "crates/pycc_parser" }
    TOML
    status, output = run_checker(files)
    refute_equal 0, status, "accepted wrong CLI binary name"
    assert_match(/\[\[bin\]\]/, output)
  end

  # --- Mutation: Cargo package name not pycc ---

  def test_rejects_wrong_cargo_package_name
    files = valid_files.dup
    files["Cargo.toml"] = <<~TOML
      [package]
      name = "not-pycc"
      version = "0.1.0"

      [[bin]]
      name = "pycc"
      path = "src/main.rs"

      [dependencies]
      pycc_parser = { path = "crates/pycc_parser" }
    TOML
    status, output = run_checker(files)
    refute_equal 0, status, "accepted wrong Cargo package name"
    assert_match(/\[package\]/, output)
  end

  # --- Mutation: llms.txt title not pycc ---

  def test_rejects_wrong_llms_title
    files = valid_files.dup
    files["site/llms.txt"] = "# not-pycc\n\n> A compiler.\n"
    status, output = run_checker(files)
    refute_equal 0, status, "accepted llms.txt with wrong title"
    assert_match(/llms\.txt/, output)
  end

  # --- Mutation: JSON-LD name not pycc ---

  def test_rejects_wrong_jsonld_name
    files = valid_files.dup
    files["site/index.html"] = <<~HTML
      <html>
      <script type="application/ld+json">
      {
        "@context": "https://schema.org",
        "@graph": [
          {
            "@type": "SoftwareSourceCode",
            "name": "not-pycc",
            "url": "https://rotnov.github.io/pycc/"
          }
        ]
      }
      </script>
      </html>
    HTML
    status, output = run_checker(files)
    refute_equal 0, status, "accepted JSON-LD with wrong name"
    assert_match(/JSON-LD/, output)
  end

  # --- Positive: no path deps means crates.io claim is OK ---

  def test_accepts_crates_io_claim_without_path_deps
    files = valid_files.dup
    files["Cargo.toml"] = <<~TOML
      [package]
      name = "pycc"
      version = "0.1.0"

      [[bin]]
      name = "pycc"
      path = "src/main.rs"

      [dependencies]
      clap = { version = "4", features = ["derive"] }
    TOML
    files["README.md"] =
      "# pycc\n\npycc is published to crates.io.\n"
    status, = run_checker(files)
    assert_equal 0, status,
                 "rejected crates.io claim when no path deps exist"
  end
end
