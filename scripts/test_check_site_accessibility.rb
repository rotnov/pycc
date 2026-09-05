#!/usr/bin/env ruby
# frozen_string_literal: true

# Mutation tests for scripts/check_site_accessibility.rb (issue #199).
#
# These tests verify the checker's own logic — LHR accessibility
# validation, ARIA conformance checking, and mutation rejection —
# against crafted inputs.  Lighthouse and Puppeteer cannot run in this
# environment (no Chrome), so all LHR objects are synthetic JSON
# fixtures and the reduced-motion evaluator is skipped.  The actual
# Lighthouse execution is tested in CI.
#
# Each mutation must make the checker fail.  Positive controls preserve
# the current healthy site.

require "json"
require "minitest/autorun"
require "tmpdir"
require "fileutils"
require "digest"
require_relative "check_site_accessibility"

class TestCheckSiteAccessibility < Minitest::Test
  REPO_ROOT = File.expand_path("..", __dir__)

  DEFAULT_MANIFEST =
    File.join(REPO_ROOT, "tests/fixtures/pages-performance-manifest.json")

  # A healthy LHR fixture for a given page and base URL.
  def healthy_lhr(page, base_url)
    expected_url = "#{base_url}#{page["local_route"].sub(/^\//, '')}"
    {
      "requestedUrl" => expected_url,
      "mainDocumentUrl" => expected_url,
      "finalDisplayedUrl" => expected_url,
      "runtimeError" => nil,
      "runWarnings" => [],
      "configSettings" => {
        "formFactor" => "mobile",
        "screenEmulation" => {
          "width" => 412,
          "height" => 823,
          "deviceScaleFactor" => 1.75
        }
      },
      "audits" => {
        "aria-allowed-role" => {
          "score" => 1,
          "scoreDisplayMode" => "binary",
          "details" => { "items" => [] }
        },
        "color-contrast" => {
          "score" => 1,
          "scoreDisplayMode" => "binary",
          "details" => { "items" => [] }
        }
      },
      "categories" => {
        "accessibility" => { "score" => 1 }
      }
    }
  end

  def load_default_manifest
    load_manifest(DEFAULT_MANIFEST)
  end

  # Write a full set of healthy LHR replicates for all pages.
  def write_healthy_lhrs(dir, manifest, base_url)
    manifest["canonical_pages"].each do |page|
      page_dir = File.join(dir, page["id"])
      FileUtils.mkdir_p(page_dir)
      lhr = healthy_lhr(page, base_url)
      File.write(
        File.join(page_dir, "replicate-1.json"),
        JSON.generate(lhr)
      )
    end
  end

  # Write a single LHR file for a page.
  def write_lhr(dir, page_id, lhr)
    page_dir = File.join(dir, page_id)
    FileUtils.mkdir_p(page_dir)
    File.write(
      File.join(page_dir, "replicate-1.json"),
      JSON.generate(lhr)
    )
  end

  # ------------------------------------------------------------------
  # Positive control: the healthy site passes the HTTP preflight + ARIA
  # (with --skip-lighthouse --skip-reduced-motion)
  # ------------------------------------------------------------------

  def test_healthy_site_passes_with_skip_lighthouse
    exit_code = site_accessibility_main(
      ["--skip-lighthouse", "--skip-reduced-motion", "--site-dir",
       File.join(REPO_ROOT, "site")]
    )
    # After Merge 2 of #199 fixed the site's ARIA violations, the
    # current site should pass the accessibility checker.
    assert_equal 0, exit_code
  end

  # ------------------------------------------------------------------
  # LHR validation: accessibility audit checks
  # ------------------------------------------------------------------

  def test_lhr_fails_when_aria_allowed_role_has_failing_items
    manifest = load_default_manifest
    base_url = "http://127.0.0.1:9999/"
    page = manifest["canonical_pages"].first
    lhr = healthy_lhr(page, base_url)
    lhr["audits"]["aria-allowed-role"]["score"] = 0
    lhr["audits"]["aria-allowed-role"]["details"]["items"] = [
      { "node" => { "snippet" => "<article role=\"listitem\">" } }
    ]
    failures = validate_lhr_accessibility(lhr, page, 1, base_url)
    refute_empty failures
    assert(failures.any? { |f| f.include?("aria-allowed-role") && f.include?("failing items") })
  end

  def test_lhr_fails_when_color_contrast_has_failing_items
    manifest = load_default_manifest
    base_url = "http://127.0.0.1:9999/"
    page = manifest["canonical_pages"].first
    lhr = healthy_lhr(page, base_url)
    lhr["audits"]["color-contrast"]["score"] = 0
    lhr["audits"]["color-contrast"]["details"]["items"] = [
      { "node" => { "snippet" => "<span style=\"color: #747973\">" } }
    ]
    failures = validate_lhr_accessibility(lhr, page, 1, base_url)
    refute_empty failures
    assert(failures.any? { |f| f.include?("color-contrast") && f.include?("failing items") })
  end

  def test_lhr_fails_when_aria_allowed_role_score_is_zero
    manifest = load_default_manifest
    base_url = "http://127.0.0.1:9999/"
    page = manifest["canonical_pages"].first
    lhr = healthy_lhr(page, base_url)
    lhr["audits"]["aria-allowed-role"]["score"] = 0
    failures = validate_lhr_accessibility(lhr, page, 1, base_url)
    refute_empty failures
    assert(failures.any? { |f| f.include?("aria-allowed-role") && f.include?("score") })
  end

  def test_lhr_fails_when_color_contrast_score_is_zero
    manifest = load_default_manifest
    base_url = "http://127.0.0.1:9999/"
    page = manifest["canonical_pages"].first
    lhr = healthy_lhr(page, base_url)
    lhr["audits"]["color-contrast"]["score"] = 0
    failures = validate_lhr_accessibility(lhr, page, 1, base_url)
    refute_empty failures
    assert(failures.any? { |f| f.include?("color-contrast") && f.include?("score") })
  end

  def test_lhr_passes_when_aria_allowed_role_is_not_applicable
    manifest = load_default_manifest
    base_url = "http://127.0.0.1:9999/"
    page = manifest["canonical_pages"].first
    lhr = healthy_lhr(page, base_url)
    lhr["audits"]["aria-allowed-role"]["scoreDisplayMode"] = "notApplicable"
    failures = validate_lhr_accessibility(lhr, page, 1, base_url)
    # aria-allowed-role is notApplicable on pages without ARIA roles —
    # this is a valid pass state for that specific audit only.
    assert_empty failures
  end

  def test_lhr_fails_when_color_contrast_is_not_applicable
    manifest = load_default_manifest
    base_url = "http://127.0.0.1:9999/"
    page = manifest["canonical_pages"].first
    lhr = healthy_lhr(page, base_url)
    lhr["audits"]["color-contrast"]["scoreDisplayMode"] = "notApplicable"
    failures = validate_lhr_accessibility(lhr, page, 1, base_url)
    # color-contrast must never be notApplicable on pages with visible
    # content — if it is, that indicates a measurement failure.
    refute_empty failures
    assert(failures.any? { |f| f.include?("color-contrast") && f.include?("notApplicable") })
  end

  def test_lhr_fails_when_required_audit_is_missing
    manifest = load_default_manifest
    base_url = "http://127.0.0.1:9999/"
    page = manifest["canonical_pages"].first
    lhr = healthy_lhr(page, base_url)
    lhr["audits"].delete("aria-allowed-role")
    failures = validate_lhr_accessibility(lhr, page, 1, base_url)
    refute_empty failures
    assert(failures.any? { |f| f.include?("aria-allowed-role") && f.include?("missing") })
  end

  def test_lhr_fails_when_requested_url_mismatches
    manifest = load_default_manifest
    base_url = "http://127.0.0.1:9999/"
    page = manifest["canonical_pages"].first
    lhr = healthy_lhr(page, base_url)
    lhr["requestedUrl"] = "http://127.0.0.1:9999/pycc/wrong/"
    failures = validate_lhr_accessibility(lhr, page, 1, base_url)
    refute_empty failures
    assert(failures.any? { |f| f.include?("requestedUrl mismatch") })
  end

  def test_lhr_fails_when_runtime_error_is_present
    manifest = load_default_manifest
    base_url = "http://127.0.0.1:9999/"
    page = manifest["canonical_pages"].first
    lhr = healthy_lhr(page, base_url)
    lhr["runtimeError"] = { "code" => "PAGE_HANG", "message" => "Page hung" }
    failures = validate_lhr_accessibility(lhr, page, 1, base_url)
    refute_empty failures
    assert(failures.any? { |f| f.include?("runtimeError present") })
  end

  def test_lhr_fails_when_config_settings_form_factor_is_wrong
    manifest = load_default_manifest
    base_url = "http://127.0.0.1:9999/"
    page = manifest["canonical_pages"].first
    lhr = healthy_lhr(page, base_url)
    lhr["configSettings"]["formFactor"] = "desktop"
    failures = validate_lhr_accessibility(lhr, page, 1, base_url)
    refute_empty failures
    assert(failures.any? { |f| f.include?("formFactor") })
  end

  def test_healthy_lhr_passes_validation
    manifest = load_default_manifest
    base_url = "http://127.0.0.1:9999/"
    page = manifest["canonical_pages"].first
    lhr = healthy_lhr(page, base_url)
    failures = validate_lhr_accessibility(lhr, page, 1, base_url)
    assert_empty failures
  end

  def test_lhr_fails_when_accessibility_category_score_is_below_one
    manifest = load_default_manifest
    base_url = "http://127.0.0.1:9999/"
    page = manifest["canonical_pages"].first
    lhr = healthy_lhr(page, base_url)
    lhr["categories"]["accessibility"]["score"] = 0.85
    # Add a failing audit that is not one of the required audits
    lhr["audits"]["heading-order"] = {
      "score" => 0,
      "scoreDisplayMode" => "binary",
      "details" => { "items" => [] }
    }
    failures = validate_lhr_accessibility(lhr, page, 1, base_url)
    refute_empty failures
    assert(failures.any? { |f| f.include?("categories.accessibility.score") && f.include?("0.85") })
    assert(failures.any? { |f| f.include?("heading-order") })
  end

  def test_lhr_fails_when_accessibility_category_is_missing
    manifest = load_default_manifest
    base_url = "http://127.0.0.1:9999/"
    page = manifest["canonical_pages"].first
    lhr = healthy_lhr(page, base_url)
    lhr["categories"].delete("accessibility")
    failures = validate_lhr_accessibility(lhr, page, 1, base_url)
    refute_empty failures
    assert(failures.any? { |f| f.include?("categories.accessibility") && f.include?("missing") })
  end

  def test_lhr_fails_when_categories_object_is_missing
    manifest = load_default_manifest
    base_url = "http://127.0.0.1:9999/"
    page = manifest["canonical_pages"].first
    lhr = healthy_lhr(page, base_url)
    lhr.delete("categories")
    failures = validate_lhr_accessibility(lhr, page, 1, base_url)
    refute_empty failures
    assert(failures.any? { |f| f.include?("categories object") && f.include?("missing") })
  end

  # ------------------------------------------------------------------
  # LHR gate: all pages must have valid LHRs
  # ------------------------------------------------------------------

  def test_lhr_gate_fails_closed_on_missing_lhr
    Dir.mktmpdir do |tmp|
      manifest = load_default_manifest
      base_url = "http://127.0.0.1:9999/"
      write_healthy_lhrs(tmp, manifest, base_url)

      # Delete one page's LHR
      page = manifest["canonical_pages"].first
      FileUtils.rm_rf(File.join(tmp, page["id"]))

      failures = validate_all_lhrs(manifest, tmp, base_url)
      refute_empty failures
      assert(failures.any? { |f| f.include?("LHR file missing") })
    end
  end

  def test_each_execution_route_requires_its_own_accessibility_report
    %w[language-support diagnostics].each do |id|
      Dir.mktmpdir do |tmp|
        manifest = load_default_manifest
        base_url = "http://127.0.0.1:9999/"
        write_healthy_lhrs(tmp, manifest, base_url)
        report = File.join(tmp, id, "replicate-1.json")
        File.delete(report)
        failures = validate_all_lhrs(manifest, tmp, base_url)
        assert(failures.any? { |f| f.include?(id) && f.include?("LHR file missing") })
        FileUtils.cp(File.join(tmp, "home", "replicate-1.json"), report)
        failures = validate_all_lhrs(manifest, tmp, base_url)
        assert(failures.any? { |f| f.include?(id) && f.include?("Url") })
      end
    end
  end

  def test_lhr_gate_fails_when_one_page_has_aria_violation
    Dir.mktmpdir do |tmp|
      manifest = load_default_manifest
      base_url = "http://127.0.0.1:9999/"
      write_healthy_lhrs(tmp, manifest, base_url)

      # Corrupt one page's aria-allowed-role
      page = manifest["canonical_pages"].first
      path = File.join(tmp, page["id"], "replicate-1.json")
      lhr = JSON.parse(File.read(path))
      lhr["audits"]["aria-allowed-role"]["score"] = 0
      lhr["audits"]["aria-allowed-role"]["details"]["items"] = [
        { "node" => { "snippet" => "<article role=\"listitem\">" } }
      ]
      File.write(path, JSON.generate(lhr))

      failures = validate_all_lhrs(manifest, tmp, base_url)
      refute_empty failures
      assert(failures.any? { |f| f.include?("aria-allowed-role") })
    end
  end

  def test_lhr_gate_fails_when_one_page_has_color_contrast_violation
    Dir.mktmpdir do |tmp|
      manifest = load_default_manifest
      base_url = "http://127.0.0.1:9999/"
      write_healthy_lhrs(tmp, manifest, base_url)

      # Corrupt one page's color-contrast
      page = manifest["canonical_pages"].first
      path = File.join(tmp, page["id"], "replicate-1.json")
      lhr = JSON.parse(File.read(path))
      lhr["audits"]["color-contrast"]["score"] = 0
      lhr["audits"]["color-contrast"]["details"]["items"] = [
        { "node" => { "snippet" => "<span>" } }
      ]
      File.write(path, JSON.generate(lhr))

      failures = validate_all_lhrs(manifest, tmp, base_url)
      refute_empty failures
      assert(failures.any? { |f| f.include?("color-contrast") })
    end
  end

  def test_lhr_gate_passes_with_all_healthy_lhrs
    Dir.mktmpdir do |tmp|
      manifest = load_default_manifest
      base_url = "http://127.0.0.1:9999/"
      write_healthy_lhrs(tmp, manifest, base_url)

      failures = validate_all_lhrs(manifest, tmp, base_url)
      assert_empty failures
    end
  end

  # ------------------------------------------------------------------
  # ARIA conformance: mutation tests against the Python evaluator
  # ------------------------------------------------------------------

  def test_aria_conformance_passes_on_current_site
    # After Merge 2 of #199 fixed the site's div[aria-label] violations,
    # the current site should pass ARIA conformance with no failures.
    failures = check_aria_conformance(File.join(REPO_ROOT, "site"))
    assert_empty failures
  end

  def test_aria_conformance_fails_when_article_listitem_is_present
    Dir.mktmpdir do |tmp|
      # Create a minimal site with an article[role=listitem] — this is
      # an ARIA conformance issue (not a Lighthouse audit), but the
      # ARIA checker should catch div[aria-label] violations.
      site_dir = File.join(tmp, "site")
      FileUtils.mkdir_p(site_dir)
      html = <<~HTML
        <!DOCTYPE html>
        <html><head><title>Test</title></head>
        <body>
          <div class="page-meta" aria-label="Test summary">
            <span>Test</span>
          </div>
        </body></html>
      HTML
      File.write(File.join(site_dir, "index.html"), html)

      # Create a minimal manifest
      manifest = {
        "manifest_version" => 1,
        "canonical_pages" => [{
          "id" => "test",
          "local_route" => "/test/",
          "expected_http_status" => 200,
          "source_artifact" => "#{site_dir}/index.html",
          "source_artifact_sha256" => Digest::SHA256.hexdigest(html),
          "expected_title" => "Test",
          "expected_h1" => "Test",
          "canonical_url" => "https://example.com/",
          "redirect_policy" => "no_redirect"
        }]
      }
      manifest_path = File.join(tmp, "manifest.json")
      File.write(manifest_path, JSON.generate(manifest))

      output, status = Open3.capture2e(
        "python3", File.join(REPO_ROOT, "scripts/check_site_aria_conformance.py"),
        "--manifest", manifest_path
      )
      refute status.success?
      assert(output.include?("aria-label"))
    end
  end

  def test_aria_conformance_passes_when_div_has_permitted_role
    Dir.mktmpdir do |tmp|
      site_dir = File.join(tmp, "site")
      FileUtils.mkdir_p(site_dir)
      html = <<~HTML
        <!DOCTYPE html>
        <html><head><title>Test</title></head>
        <body>
          <div role="group" aria-label="Test group">
            <span>Test</span>
          </div>
        </body></html>
      HTML
      File.write(File.join(site_dir, "index.html"), html)

      manifest = {
        "manifest_version" => 1,
        "canonical_pages" => [{
          "id" => "test",
          "local_route" => "/test/",
          "expected_http_status" => 200,
          "source_artifact" => "#{site_dir}/index.html",
          "source_artifact_sha256" => Digest::SHA256.hexdigest(html),
          "expected_title" => "Test",
          "expected_h1" => "Test",
          "canonical_url" => "https://example.com/",
          "redirect_policy" => "no_redirect"
        }]
      }
      manifest_path = File.join(tmp, "manifest.json")
      File.write(manifest_path, JSON.generate(manifest))

      output, status = Open3.capture2e(
        "python3", File.join(REPO_ROOT, "scripts/check_site_aria_conformance.py"),
        "--manifest", manifest_path
      )
      assert status.success?
      assert(output.include?("OK"))
    end
  end

  # ------------------------------------------------------------------
  # Narrow page coverage: the gate must check ALL canonical pages
  # ------------------------------------------------------------------

  def test_lhr_gate_fails_when_page_coverage_is_narrow
    Dir.mktmpdir do |tmp|
      manifest = load_default_manifest
      base_url = "http://127.0.0.1:9999/"

      # Only write LHRs for the first page
      page = manifest["canonical_pages"].first
      write_lhr(tmp, page["id"], healthy_lhr(page, base_url))

      failures = validate_all_lhrs(manifest, tmp, base_url)
      refute_empty failures
      # Should report missing LHRs for the other pages
      assert(failures.any? { |f| f.include?("LHR file missing") })
    end
  end
end
