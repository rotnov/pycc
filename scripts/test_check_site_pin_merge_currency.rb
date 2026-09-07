#!/usr/bin/env ruby
# frozen_string_literal: true

require "fileutils"
require "minitest/autorun"
require "open3"
require "pathname"
require "tmpdir"

require_relative "check_site_pin_merge_currency"

class SitePinMergeCurrencyTest < Minitest::Test
  STATUS_SOURCE = "site/status/index.html"
  STATUS_CANONICAL = "https://rotnov.github.io/pycc/status/"
  HOME_SOURCE = "site/index.html"
  HOME_CANONICAL = "https://rotnov.github.io/pycc/"

  # A commit author date pinned to a fixed instant with a non-zero offset, so
  # every test exercises the same offset-derivation path the real repository
  # uses (the merging identity has historically committed at +01:00).
  BASE_AUTHOR_DATE = "2026-09-07T13:39:37+01:00"
  OFFSET_SECONDS = 3600

  def run_git!(root, *args, env: {})
    stdout, stderr, status = Open3.capture3(env, "git", *args, chdir: root.to_s)
    raise "git #{args.join(' ')} failed: #{stderr}" unless status.success?

    stdout
  end

  def init_repo(root)
    run_git!(root, "init", "--quiet", "--initial-branch=main")
    run_git!(root, "config", "user.email", "test@example.com")
    run_git!(root, "config", "user.name", "Test")
  end

  def write_and_commit(root, files, message, author_date: BASE_AUTHOR_DATE)
    files.each do |relative, content|
      path = root / relative
      FileUtils.mkdir_p(path.dirname)
      path.write(content)
    end
    run_git!(root, "add", "-A")
    run_git!(
      root, "commit", "--quiet", "-m", message,
      env: { "GIT_AUTHOR_DATE" => author_date, "GIT_COMMITTER_DATE" => author_date }
    )
    run_git!(root, "rev-parse", "HEAD").strip
  end

  def sitemap(status_date:, home_date:)
    <<~XML
      <?xml version="1.0" encoding="UTF-8"?>
      <urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
        <url><loc>#{HOME_CANONICAL}</loc><lastmod>#{home_date}</lastmod></url>
        <url><loc>#{STATUS_CANONICAL}</loc><lastmod>#{status_date}</lastmod></url>
      </urlset>
    XML
  end

  # Build a repository whose base commit holds a sitemap and both page
  # sources, then return [root, base_sha] with a block-provided head commit.
  def with_repo
    Dir.mktmpdir do |dir|
      root = Pathname(dir)
      init_repo(root)
      base = write_and_commit(
        root,
        {
          SITEMAP_RELATIVE_PATH => sitemap(status_date: "2026-09-06", home_date: "2026-09-06"),
          STATUS_SOURCE => "<html>base status</html>\n",
          HOME_SOURCE => "<html>base home</html>\n",
          "docs/README.md" => "unrelated\n",
        },
        "base"
      )
      yield(root, base)
    end
  end

  # 2026-09-07 12:00 UTC is 13:00 at +01:00 -- the same calendar day at both
  # offsets, so the fail-closed rollover guard does not fire.
  def midday
    Time.utc(2026, 9, 7, 12, 0, 0)
  end

  # 2026-09-07 23:30 UTC is 2026-09-08 00:30 at +01:00 -- the two dates
  # disagree, so the rollover guard is armed.
  def inside_rollover_window
    Time.utc(2026, 9, 7, 23, 30, 0)
  end

  def test_passes_when_no_canonical_page_source_is_touched
    with_repo do |root, base|
      head = write_and_commit(root, { "docs/README.md" => "edited\n" }, "docs only")
      result = check_site_pin_merge_currency(root, base, head, now: midday)
      assert_match(/no canonical page source touched/, result)
    end
  end

  # Pins the ordering of the two phases: the diff is evaluated before any date
  # reasoning, so a pull request that touches no page source must pass even
  # inside the rollover window. Without that ordering the fail-closed guard
  # would red every pull request in the repository for part of every day.
  def test_passes_inside_rollover_window_when_no_page_source_is_touched
    with_repo do |root, base|
      head = write_and_commit(root, { "docs/README.md" => "edited\n" }, "docs only")
      result = check_site_pin_merge_currency(root, base, head, now: inside_rollover_window)
      assert_match(/no canonical page source touched/, result)
    end
  end

  def test_passes_when_pin_already_matches_the_predicted_merge_date
    with_repo do |root, base|
      head = write_and_commit(
        root,
        {
          STATUS_SOURCE => "<html>edited status</html>\n",
          SITEMAP_RELATIVE_PATH => sitemap(status_date: "2026-09-07", home_date: "2026-09-06"),
        },
        "rotate status pins"
      )
      result = check_site_pin_merge_currency(root, base, head, now: midday)
      assert_match(/2026-09-07/, result)
    end
  end

  # The day-N/day-N+1 defect itself: the branch pinned day N, but the merge is
  # now predicted for day N+1.
  def test_rejects_a_day_n_pin_against_a_day_n_plus_one_merge
    with_repo do |root, base|
      head = write_and_commit(
        root,
        {
          STATUS_SOURCE => "<html>edited status</html>\n",
          SITEMAP_RELATIVE_PATH => sitemap(status_date: "2026-09-06", home_date: "2026-09-06"),
        },
        "edit status without rotating pins"
      )
      error = assert_raises(SitePinMergeCurrencyError) do
        check_site_pin_merge_currency(root, base, head, now: midday)
      end
      message = error.message
      assert_match(/#{Regexp.escape(STATUS_SOURCE)}/, message)
      assert_match(/2026-09-07/, message)
      # All four pins plus the manifest-digest recompute must be enumerated.
      assert_match(/<lastmod>/, message)
      assert_match(/dateModified/, message)
      assert_match(/PAGE_SPECS/, message)
      assert_match(/source_artifact_sha256/, message)
      assert_match(/shasum -a 256/, message)
      assert_match(/#{Regexp.escape(PERFORMANCE_MANIFEST_PATH)}/, message)
    end
  end

  def test_reports_every_stale_page_not_just_the_first
    with_repo do |root, base|
      head = write_and_commit(
        root,
        {
          STATUS_SOURCE => "<html>edited status</html>\n",
          HOME_SOURCE => "<html>edited home</html>\n",
          SITEMAP_RELATIVE_PATH => sitemap(status_date: "2026-09-06", home_date: "2026-09-06"),
        },
        "edit both pages without rotating pins"
      )
      error = assert_raises(SitePinMergeCurrencyError) do
        check_site_pin_merge_currency(root, base, head, now: midday)
      end
      assert_match(/#{Regexp.escape(STATUS_SOURCE)}/, error.message)
      assert_match(/#{Regexp.escape(HOME_SOURCE)}/, error.message)
    end
  end

  # A future-dated pin is as wrong as a stale one, and must not be softened
  # into a "stale only" comparison.
  def test_rejects_a_pin_ahead_of_the_predicted_merge_date
    with_repo do |root, base|
      head = write_and_commit(
        root,
        {
          STATUS_SOURCE => "<html>edited status</html>\n",
          SITEMAP_RELATIVE_PATH => sitemap(status_date: "2026-09-09", home_date: "2026-09-06"),
        },
        "future-dated status pin"
      )
      error = assert_raises(SitePinMergeCurrencyError) do
        check_site_pin_merge_currency(root, base, head, now: midday)
      end
      assert_match(/pinned 2026-09-09/, error.message)
    end
  end

  def test_fails_closed_inside_the_rollover_window
    with_repo do |root, base|
      head = write_and_commit(
        root,
        {
          STATUS_SOURCE => "<html>edited status</html>\n",
          SITEMAP_RELATIVE_PATH => sitemap(status_date: "2026-09-07", home_date: "2026-09-06"),
        },
        "rotate status pins"
      )
      error = assert_raises(SitePinMergeCurrencyError) do
        check_site_pin_merge_currency(root, base, head, now: inside_rollover_window)
      end
      assert_match(/ambiguous/, error.message)
      assert_match(/2026-09-07/, error.message)
      assert_match(/2026-09-08/, error.message)
    end
  end

  # A canonical page whose source is touched but which is absent from the
  # sitemap belongs to check_sitemap_lastmod.rb / check-site.sh; this checker
  # skips it rather than emitting a competing diagnostic.
  def test_skips_a_touched_page_absent_from_the_sitemap
    with_repo do |root, base|
      head = write_and_commit(
        root,
        {
          STATUS_SOURCE => "<html>edited status</html>\n",
          SITEMAP_RELATIVE_PATH => <<~XML,
            <?xml version="1.0" encoding="UTF-8"?>
            <urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
              <url><loc>#{HOME_CANONICAL}</loc><lastmod>2026-09-06</lastmod></url>
            </urlset>
          XML
        },
        "edit status, drop it from the sitemap"
      )
      result = check_site_pin_merge_currency(root, base, head, now: midday)
      assert_match(/2026-09-07/, result)
    end
  end

  # An untracked or otherwise unmapped path is not a canonical page source and
  # must not be treated as one.
  def test_ignores_unmapped_site_paths
    with_repo do |root, base|
      head = write_and_commit(root, { "site/robots.txt" => "User-agent: *\n" }, "robots only")
      result = check_site_pin_merge_currency(root, base, head, now: midday)
      assert_match(/no canonical page source touched/, result)
    end
  end

  def test_falls_back_to_utc_when_the_base_offset_cannot_be_read
    with_repo do |root, base|
      head = write_and_commit(
        root,
        {
          STATUS_SOURCE => "<html>edited status</html>\n",
          SITEMAP_RELATIVE_PATH => sitemap(status_date: "2026-09-07", home_date: "2026-09-06"),
        },
        "rotate status pins"
      )
      # An unreadable base offset must not crash the checker; it degrades to
      # UTC and says so in the passing summary.
      SitePinMergeCurrencyTest.stub_offset_failure do
        result = check_site_pin_merge_currency(root, base, head, now: midday)
        assert_match(/could not be read/, result)
      end
    end
  end

  def self.stub_offset_failure
    original = Object.instance_method(:base_author_offset_seconds)
    Object.send(:define_method, :base_author_offset_seconds) { |_root, _revision| nil }
    yield
  ensure
    Object.send(:define_method, :base_author_offset_seconds, original)
  end

  def test_diff_fetcher_is_injectable
    with_repo do |root, base|
      head = write_and_commit(root, { "docs/README.md" => "edited\n" }, "docs only")
      captured = nil
      fetcher = lambda do |fetch_root, fetch_base, fetch_head|
        captured = [fetch_root, fetch_base, fetch_head]
        [STATUS_SOURCE]
      end
      error = assert_raises(SitePinMergeCurrencyError) do
        check_site_pin_merge_currency(root, base, head, now: midday, diff_fetcher: fetcher)
      end
      assert_equal [root, base, head], captured
      assert_match(/#{Regexp.escape(STATUS_SOURCE)}/, error.message)
    end
  end

  def test_cli_rejects_a_missing_base_revision_argument
    assert_equal 1, main([])
  end

  def test_cli_reports_a_pass_for_a_docs_only_change
    with_repo do |root, base|
      head = write_and_commit(root, { "docs/README.md" => "edited\n" }, "docs only")
      assert_equal 0, main([base, head, root.to_s])
    end
  end

  def test_cli_reports_a_failure_for_an_unresolvable_revision
    with_repo do |root, _base|
      assert_equal 1, main(["0" * 40, "HEAD", root.to_s])
    end
  end

  def test_offset_formatting_covers_both_signs_and_the_unknown_case
    assert_equal "+01:00", format_offset(3600)
    assert_equal "-05:30", format_offset(-19_800)
    assert_equal "+00:00", format_offset(0)
    assert_equal "+00:00", format_offset(nil)
  end
end
