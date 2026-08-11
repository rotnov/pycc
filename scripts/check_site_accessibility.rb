#!/usr/bin/env ruby
# frozen_string_literal: true

# Site accessibility gate orchestrator (issue #199).
#
# This checker is the main gate for the site accessibility contract.
# It performs three phases:
#
# 1. **HTTP identity preflight** — starts the hermetic local server
#    (``serve_pages_fixture.py``) and verifies that every canonical page
#    returns HTTP 200 with the correct media type and a body whose SHA-256
#    matches the manifest artifact digest.  This reuses the same
#    preflight pattern as ``check_pages_performance_budget.rb``.
#
# 2. **LHR accessibility validation** — validates every Lighthouse Result
#    (LHR) JSON object: ``aria-allowed-role`` and ``color-contrast``
#    audits must each have score 1 with zero failing items, and expected
#    audits must be present and not ``notApplicable``.
#
# 3. **ARIA conformance + reduced-motion evaluation** — invokes the
#    W3C Nu ARIA conformance evaluator (Python) and the reduced-motion
#    computed-style evaluator (Node.js/Puppeteer), aggregating their
#    failures.
#
# The checker fails closed on any violation: exit 1 on an accessibility
# failure, exit 2 on a configuration or structural error.

require "json"
require "digest"
require "net/http"
require "uri"
require "open3"
require "fileutils"
require "pathname"

MANIFEST_DEFAULT =
  File.expand_path("../tests/fixtures/pages-performance-manifest.json", __dir__)
SERVE_SCRIPT =
  File.expand_path("serve_pages_fixture.py", __dir__)
ARIA_SCRIPT =
  File.expand_path("check_site_aria_conformance.py", __dir__)
REDUCED_MOTION_SCRIPT =
  File.expand_path("check_site_reduced_motion.js", __dir__)

# Audits that must be present, pass with score 1, and have 0 failing items.
REQUIRED_AUDITS = %w[aria-allowed-role color-contrast].freeze

# ---------------------------------------------------------------------------
# Manifest loading
# ---------------------------------------------------------------------------

def load_json_file(path, label)
  raise "#{label} not found: #{path}" unless File.file?(path)

  JSON.parse(File.read(path))
rescue JSON::ParserError => e
  raise "could not parse #{label} #{path}: #{e.message}"
end

def load_manifest(path)
  manifest = load_json_file(path, "manifest")
  unless manifest.is_a?(Hash)
    raise "manifest must be a JSON object"
  end
  unless manifest["manifest_version"] == 1
    raise "manifest version must be 1, got #{manifest["manifest_version"]}"
  end
  pages = manifest["canonical_pages"]
  unless pages.is_a?(Array) && !pages.empty?
    raise "manifest must contain at least one canonical page"
  end
  pages.each do |page|
    %w[id local_route expected_http_status source_artifact
       source_artifact_sha256 expected_title expected_h1
       canonical_url redirect_policy].each do |key|
      raise "canonical page missing required key '#{key}': #{page.inspect}" unless page.key?(key)
    end
  end
  manifest
end

# ---------------------------------------------------------------------------
# HTTP identity preflight (reuses the same pattern as check_pages_performance_budget.rb)
# ---------------------------------------------------------------------------

def start_hermetic_server(site_dir)
  stdin, stdout_stderr, wait_thr = Open3.popen2e(
    "python3", SERVE_SCRIPT, "--port", "0", "--site-dir", site_dir
  )
  stdin.close
  port_line = stdout_stderr.gets&.strip
  unless port_line && port_line.match?(/\A\d+\z/)
    remaining = stdout_stderr.read
    Process.kill("TERM", wait_thr.pid) rescue nil
    wait_thr.wait rescue nil
    raise "could not parse server port from stdout: #{port_line.inspect} (remaining: #{remaining})"
  end
  [Integer(port_line), wait_thr, stdout_stderr]
end

def stop_hermetic_server(wait_thr, stdout_stderr)
  return unless wait_thr

  stdout_stderr&.close
  Process.kill("TERM", wait_thr.pid) rescue nil
  wait_thr.wait rescue nil
end

def http_get(uri)
  uri = URI(uri)
  http = Net::HTTP.new(uri.host, uri.port)
  http.read_timeout = 10
  http.open_timeout = 5
  request = Net::HTTP::Get.new(uri.request_uri)
  response = http.request(request)
  {
    status: response.code.to_i,
    body: response.body || "",
    content_type: response["Content-Type"] || "",
    final_uri: uri.to_s
  }
end

def sha256_hex(body)
  Digest::SHA256.hexdigest(body)
end

def check_http_identity(manifest, base_url)
  failures = []

  manifest["canonical_pages"].each do |page|
    url = "#{base_url}#{page["local_route"].sub(/^\//, '')}"
    result = http_get(url)

    if result[:status] != page["expected_http_status"]
      failures << "#{page["id"]}: expected HTTP #{page["expected_http_status"]}, got #{result[:status]}"
      next
    end

    unless result[:content_type].start_with?("text/html")
      failures << "#{page["id"]}: expected text/html content type, got #{result[:content_type]}"
      next
    end

    actual_digest = sha256_hex(result[:body])
    expected_digest = page["source_artifact_sha256"]
    if actual_digest != expected_digest
      failures << "#{page["id"]}: body SHA-256 mismatch (expected #{expected_digest}, got #{actual_digest})"
      next
    end
  end

  failures
end

# ---------------------------------------------------------------------------
# LHR accessibility validation
# ---------------------------------------------------------------------------

def validate_lhr_accessibility(lhr, page, replicate, base_url)
  failures = []
  page_id = page["id"]
  expected_url = "#{base_url}#{page["local_route"].sub(/^\//, '')}"

  # 1. requestedUrl, mainDocumentUrl, finalDisplayedUrl must map to the
  #    same expected local canonical route.
  %w[requestedUrl mainDocumentUrl finalDisplayedUrl].each do |field|
    val = lhr[field]
    unless val == expected_url
      failures << "#{page_id} replicate #{replicate}: #{field} mismatch (expected #{expected_url}, got #{val})"
    end
  end

  # 2. runtimeError must be absent
  runtime_error = lhr["runtimeError"]
  if runtime_error && !runtime_error.empty?
    failures << "#{page_id} replicate #{replicate}: runtimeError present: #{runtime_error.inspect}"
  end

  # 3. configSettings must match the pinned mobile profile
  config = lhr["configSettings"]
  unless config.is_a?(Hash)
    failures << "#{page_id} replicate #{replicate}: configSettings missing or not an object"
  else
    unless config["formFactor"] == "mobile"
      failures << "#{page_id} replicate #{replicate}: configSettings.formFactor must be 'mobile', got #{config["formFactor"]}"
    end
  end

  # 4. Validate required accessibility audits
  audits = lhr["audits"]
  unless audits.is_a?(Hash)
    failures << "#{page_id} replicate #{replicate}: audits object missing or not a hash"
    return failures
  end

  REQUIRED_AUDITS.each do |audit_id|
    audit = audits[audit_id]
    unless audit.is_a?(Hash)
      failures << "#{page_id} replicate #{replicate}: required audit '#{audit_id}' is missing"
      next
    end

    # The audit must not be notApplicable (the fixture should exercise it).
    if audit["scoreDisplayMode"] == "notApplicable"
      failures << "#{page_id} replicate #{replicate}: audit '#{audit_id}' is notApplicable (fixture should exercise it)"
      next
    end

    # Score must be 1 (pass) — accessibility audits are binary (1 = pass, 0 = fail).
    score = audit["score"]
    unless score == 1
      failures << "#{page_id} replicate #{replicate}: audit '#{audit_id}' score is #{score}, expected 1"
    end

    # Zero failing items.
    details = audit["details"]
    if details.is_a?(Hash)
      items = details["items"]
      if items.is_a?(Array) && !items.empty?
        failures << "#{page_id} replicate #{replicate}: audit '#{audit_id}' has #{items.length} failing items, expected 0"
      end
    end
  end

  # 5. The overall accessibility category score must be 1.0 — no
  #    accessibility audit may fail.  This is a backstop beyond the two
  #    named audits above: if the category score is < 1.0, collect every
  #    failing audit ID for actionable diagnostics.
  categories = lhr["categories"]
  unless categories.is_a?(Hash)
    failures << "#{page_id} replicate #{replicate}: categories object missing or not a hash"
    return failures
  end

  a11y_category = categories["accessibility"]
  unless a11y_category.is_a?(Hash)
    failures << "#{page_id} replicate #{replicate}: categories.accessibility is missing or not an object"
    return failures
  end

  a11y_score = a11y_category["score"]
  if a11y_score.nil?
    failures << "#{page_id} replicate #{replicate}: categories.accessibility.score is missing"
  elsif a11y_score < 1.0
    # Collect all failing audit IDs for actionable diagnostics.  An audit
    # is considered failing when it has a numeric score < 1 and is not
    # notApplicable, manual, or informative (those do not affect the
    # category score).
    failing_audits = []
    audits.each do |audit_id, audit|
      next unless audit.is_a?(Hash)
      score = audit["score"]
      mode = audit["scoreDisplayMode"]
      if score.is_a?(Numeric) && score < 1 &&
         mode != "notApplicable" && mode != "manual" && mode != "informative"
        failing_audits << audit_id
      end
    end

    if failing_audits.empty?
      failures << "#{page_id} replicate #{replicate}: categories.accessibility.score is #{a11y_score}, expected 1.0"
    else
      failures << "#{page_id} replicate #{replicate}: categories.accessibility.score is #{a11y_score}, expected 1.0 (failing audits: #{failing_audits.join(', ')})"
    end
  end

  failures
end

def validate_all_lhrs(manifest, lhr_dir, base_url)
  failures = []

  manifest["canonical_pages"].each do |page|
    page_id = page["id"]
    replicate = 1
    lhr_path = File.join(lhr_dir, page_id, "replicate-#{replicate}.json")

    unless File.file?(lhr_path)
      failures << "#{page_id}: LHR file missing: #{lhr_path}"
      next
    end

    begin
      lhr = JSON.parse(File.read(lhr_path))
    rescue JSON::ParserError => e
      failures << "#{page_id}: could not parse LHR #{lhr_path}: #{e.message}"
      next
    end

    failures.concat(validate_lhr_accessibility(lhr, page, replicate, base_url))
  end

  failures
end

# ---------------------------------------------------------------------------
# ARIA conformance evaluation
# ---------------------------------------------------------------------------

def check_aria_conformance(site_dir)
  failures = []

  output, status = Open3.capture2e(
    "python3", ARIA_SCRIPT,
    "--site-dir", site_dir
  )

  unless status.success?
    # The script prints failures to stderr; capture them.
    output.each_line do |line|
      line.strip!
      next if line.empty?
      failures << line.sub(/^FAIL:\s*/, "")
    end
  end

  failures
end

# ---------------------------------------------------------------------------
# Reduced-motion evaluation
# ---------------------------------------------------------------------------

def check_reduced_motion(base_url, manifest_path, chrome_path)
  failures = []

  cmd = ["node", REDUCED_MOTION_SCRIPT,
         "--base-url", base_url,
         "--manifest", manifest_path]
  cmd += ["--chrome-path", chrome_path] if chrome_path && !chrome_path.empty?

  output, status = Open3.capture2e(*cmd)

  unless status.success?
    output.each_line do |line|
      line.strip!
      next if line.empty?
      failures << line.sub(/^FAIL:\s*/, "")
    end
  end

  failures
end

# ---------------------------------------------------------------------------
# Main entry point
# ---------------------------------------------------------------------------

def find_chrome
  candidates = [
    ENV["CHROME_PATH"],
    ENV["PUPPETEER_EXECUTABLE_PATH"],
    "/usr/bin/google-chrome",
    "/usr/bin/google-chrome-stable",
    "/usr/bin/chromium",
    "/usr/bin/chromium-browser",
    "/snap/bin/chromium",
    "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
    "/Applications/Chromium.app/Contents/MacOS/Chromium",
  ].compact
  candidates.each do |c|
    return c if File.executable?(c)
  end
  ""
end

def site_accessibility_main(arguments)
  manifest_path = MANIFEST_DEFAULT
  lhr_dir = nil
  site_dir = "site"
  repo_root = File.expand_path("..", __dir__)
  skip_lighthouse = false
  skip_reduced_motion = false

  args = arguments.dup
  while (arg = args.shift)
    case arg
    when "--manifest" then manifest_path = args.shift
    when "--lhr-dir" then lhr_dir = args.shift
    when "--site-dir" then site_dir = args.shift
    when "--repo-root" then repo_root = args.shift
    when "--skip-lighthouse" then skip_lighthouse = true
    when "--skip-reduced-motion" then skip_reduced_motion = true
    when "--help", "-h"
      warn "usage: check_site_accessibility.rb [--manifest PATH] [--lhr-dir DIR] [--site-dir DIR] [--repo-root DIR] [--skip-lighthouse] [--skip-reduced-motion]"
      return 0
    else
      warn "unknown argument: #{arg}"
      return 2
    end
  end

  manifest = load_manifest(manifest_path)

  # Phase 1: HTTP identity preflight
  puts "Phase 1: HTTP identity preflight"
  port, server_thr, server_io = start_hermetic_server(site_dir)
  base_url = "http://127.0.0.1:#{port}/"

  exit_code = catch(:done) do
    begin
      http_failures = check_http_identity(manifest, base_url)

      unless http_failures.empty?
        http_failures.each { |f| warn "FAIL: #{f}" }
        throw :done, 1
      end
      puts "OK: HTTP identity preflight passed for all canonical pages"

      # Phase 2: LHR accessibility validation
      if skip_lighthouse
        puts "SKIP: Lighthouse validation skipped (--skip-lighthouse)"
      else
        unless lhr_dir && File.directory?(lhr_dir)
          warn "FAIL: --lhr-dir is required when Lighthouse validation is not skipped"
          throw :done, 2
        end

        # Derive the base_url from the LHR reports (the Lighthouse runner
        # starts its own server on a random port).
        lhr_file = Dir.glob(File.join(lhr_dir, "**", "*.json")).first
        unless lhr_file
          warn "FAIL: no LHR JSON files found in #{lhr_dir}"
          throw :done, 2
        end
        lhr = JSON.parse(File.read(lhr_file))
        requested_url = lhr["requestedUrl"]
        unless requested_url.is_a?(String) && !requested_url.empty?
          warn "FAIL: LHR #{lhr_file} has no usable requestedUrl"
          throw :done, 2
        end
        uri = URI(requested_url)
        lhr_base_url = "#{uri.scheme}://#{uri.host}:#{uri.port}/"

        puts "Phase 2: LHR accessibility validation"
        lhr_failures = validate_all_lhrs(manifest, lhr_dir, lhr_base_url)
        unless lhr_failures.empty?
          lhr_failures.each { |f| warn "FAIL: #{f}" }
          throw :done, 1
        end
        puts "OK: Lighthouse accessibility validation passed"
      end

      # Phase 3: ARIA conformance + reduced-motion evaluation
      puts "Phase 3: ARIA conformance evaluation"
      aria_failures = check_aria_conformance(site_dir)
      unless aria_failures.empty?
        aria_failures.each { |f| warn "FAIL: #{f}" }
        throw :done, 1
      end
      puts "OK: ARIA conformance check passed"

      unless skip_reduced_motion
        puts "Phase 3: Reduced-motion evaluation"
        chrome_path = find_chrome
        rm_failures = check_reduced_motion(base_url, manifest_path, chrome_path)
        unless rm_failures.empty?
          rm_failures.each { |f| warn "FAIL: #{f}" }
          throw :done, 1
        end
        puts "OK: Reduced-motion contract passed"
      end

      puts "OK: site accessibility gate passed"
      0
    ensure
      stop_hermetic_server(server_thr, server_io)
    end
  end

  exit_code
rescue RuntimeError, ArgumentError => e
  warn "ERROR: #{e.message}"
  2
end

exit(site_accessibility_main(ARGV)) if $PROGRAM_NAME == __FILE__
