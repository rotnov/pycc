#!/usr/bin/env ruby
# frozen_string_literal: true

require "fileutils"
require "minitest/autorun"
require "open3"
require "pathname"
require "rbconfig"
require "tmpdir"

require_relative "check_status_page_freshness"

class StatusPageFreshnessTest < Minitest::Test
  # Trailing prose continues on the same physical line after the closing
  # `**`, matching the real docs/ROADMAP.md shape (see the D-068 finding
  # that caught the original fixture putting the bold span alone on its
  # own line -- a shape the real file never took). The very first, pre-fix
  # regex's defect was its anchoring (`\A...\s*\z` required nothing after
  # the closing `**`), not its whole-line scope; `milestone_line` restores
  # whole-line comparison while fixing that anchoring bug, so this fixture
  # keeps the trailing prose on the same line deliberately.
  BASE_ROADMAP = <<~MARKDOWN
    # pycc Roadmap

    **Current milestone: v0.2 — acceptance criteria met; v0.3 in progress.** All five v0.1 acceptance-checklist bullets below are green: the checked v0.1 surface reaches a native executable through MIR, LLVM, and the runtime.

    ## v0.1 acceptance

    - [x] `fib` compiles and matches CPython output. <!-- roadmap-evidence: conformance-fib-mandelbrot-tier1 -->
    - [ ] Some later gate is still pending. <!-- roadmap-evidence: some-later-gate -->

    ## v0.3

    **[#999](https://github.com/rotnov/pycc/issues/999) — Test feature:** description here.
  MARKDOWN

  def run_git!(root, *args)
    stdout, stderr, status = Open3.capture3("git", *args, chdir: root.to_s)
    raise "git #{args.join(' ')} failed: #{stderr}" unless status.success?

    stdout
  end

  def init_repo(root)
    run_git!(root, "init", "--quiet", "--initial-branch=main")
    run_git!(root, "config", "user.email", "test@example.com")
    run_git!(root, "config", "user.name", "Test")
  end

  def write_and_commit(root, files, message)
    files.each do |relative, content|
      path = root / relative
      FileUtils.mkdir_p(path.dirname)
      path.write(content)
    end
    run_git!(root, "add", "-A")
    run_git!(root, "commit", "--quiet", "-m", message)
    run_git!(root, "rev-parse", "HEAD").strip
  end

  def with_repo
    Dir.mktmpdir do |directory|
      root = Pathname(directory)
      init_repo(root)
      yield root
    end
  end

  # (a) milestone-line change WITH a watched-page touch -> pass.
  def test_milestone_change_with_status_page_touch_passes
    with_repo do |root|
      base_sha = write_and_commit(root, { ROADMAP_PATH => BASE_ROADMAP }, "base")
      changed_roadmap = BASE_ROADMAP.sub(
        "v0.2 — acceptance criteria met; v0.3 in progress.",
        "v0.3 — class model core landed."
      )
      write_and_commit(
        root,
        {
          ROADMAP_PATH => changed_roadmap,
          "site/status/index.html" => "<html>updated</html>"
        },
        "milestone + status page"
      )

      result = check_status_page_freshness(root, base_sha, "HEAD")
      assert_match(/roadmap milestone\/evidence\/feature-paragraph signal matched/, result)
    end
  end

  # (b) milestone-line change WITHOUT a watched-page touch -> fail with the
  # expected, actionable message.
  def test_milestone_change_without_status_page_touch_fails
    with_repo do |root|
      base_sha = write_and_commit(root, { ROADMAP_PATH => BASE_ROADMAP }, "base")
      changed_roadmap = BASE_ROADMAP.sub(
        "v0.2 — acceptance criteria met; v0.3 in progress.",
        "v0.3 — class model core landed."
      )
      write_and_commit(root, { ROADMAP_PATH => changed_roadmap }, "milestone only")

      error = assert_raises(StatusPageFreshnessError) do
        check_status_page_freshness(root, base_sha, "HEAD")
      end
      assert_match(/site\/status\/index\.html/, error.message)
      assert_match(%r{https://github\.com/rotnov/pycc/issues/401}, error.message)
      assert_match(/docs\/WEBSITE\.md/, error.message)
    end
  end

  # (b2) trailing-prose-only edit on the milestone line: the bold span text
  # itself ("**Current milestone: ...**") is untouched, only the status
  # prose that continues on the same physical line *after* the closing
  # "**" changes. WITHOUT a watched-page touch -> must still fail. This
  # pins the exact gap the whole-line `milestone_line` comparison closes:
  # a span-only comparison produces no signal at all for this shape,
  # silently reproducing the staleness-detection gap issue #401 was filed
  # to fix.
  def test_milestone_line_trailing_prose_edit_without_status_page_touch_fails
    with_repo do |root|
      base_sha = write_and_commit(root, { ROADMAP_PATH => BASE_ROADMAP }, "base")
      changed_roadmap = BASE_ROADMAP.sub(
        "All five v0.1 acceptance-checklist bullets below are green",
        "All five v0.1 acceptance-checklist bullets below are green and re-verified"
      )
      write_and_commit(root, { ROADMAP_PATH => changed_roadmap }, "trailing prose only")

      error = assert_raises(StatusPageFreshnessError) do
        check_status_page_freshness(root, base_sha, "HEAD")
      end
      assert_match(/site\/status\/index\.html/, error.message)
    end
  end

  # (c) an evidence-marker checkbox flip WITHOUT a watched-page touch -> fail.
  def test_evidence_checkbox_flip_without_status_page_touch_fails
    with_repo do |root|
      base_sha = write_and_commit(root, { ROADMAP_PATH => BASE_ROADMAP }, "base")
      changed_roadmap = BASE_ROADMAP.sub(
        "- [ ] Some later gate is still pending.",
        "- [x] Some later gate is still pending."
      )
      write_and_commit(root, { ROADMAP_PATH => changed_roadmap }, "evidence flip")

      error = assert_raises(StatusPageFreshnessError) do
        check_status_page_freshness(root, base_sha, "HEAD")
      end
      assert_match(/roadmap-evidence checklist entry/, error.message)
    end
  end

  # (d) ordinary prose-only edit (no milestone-line/evidence-marker change)
  # WITHOUT a watched-page touch -> pass (negative control).
  def test_prose_only_roadmap_edit_without_status_page_touch_passes
    with_repo do |root|
      base_sha = write_and_commit(root, { ROADMAP_PATH => BASE_ROADMAP }, "base")
      changed_roadmap = BASE_ROADMAP.sub(
        "## v0.1 acceptance",
        "## v0.1 acceptance (see also the conformance matrix)"
      )
      write_and_commit(root, { ROADMAP_PATH => changed_roadmap }, "prose only")

      result = check_status_page_freshness(root, base_sha, "HEAD")
      assert_match(/no roadmap milestone, evidence-checklist, or feature-landing-paragraph signal/, result)
    end
  end

  # (e) a diff touching neither docs/ROADMAP.md nor a watched page -> pass
  # (no-op).
  def test_unrelated_change_passes
    with_repo do |root|
      base_sha = write_and_commit(root, { ROADMAP_PATH => BASE_ROADMAP }, "base")
      write_and_commit(root, { "README.md" => "unrelated" }, "unrelated")

      result = check_status_page_freshness(root, base_sha, "HEAD")
      assert_match(/no roadmap milestone, evidence-checklist, or feature-landing-paragraph signal/, result)
    end
  end

  # (f) the diff-base fetch/diff comes back empty when it shouldn't -- this
  # failure mode must not be silently swallowed. Simulate it directly: a
  # real content signal fired, but an injected diff fetcher reports no
  # changed files at all, exercising the dedicated diff-base sanity check
  # without depending on git ever actually behaving this way.
  def test_signal_with_empty_diff_is_a_hard_failure_not_a_silent_pass
    with_repo do |root|
      base_sha = write_and_commit(root, { ROADMAP_PATH => BASE_ROADMAP }, "base")
      changed_roadmap = BASE_ROADMAP.sub(
        "v0.2 — acceptance criteria met; v0.3 in progress.",
        "v0.3 — class model core landed."
      )
      write_and_commit(root, { ROADMAP_PATH => changed_roadmap }, "milestone only")

      empty_diff_fetcher = ->(*_args) { [] }

      error = assert_raises(StatusPageFreshnessError) do
        check_status_page_freshness(root, base_sha, "HEAD", diff_fetcher: empty_diff_fetcher)
      end
      assert_match(/diff-base failure/, error.message)
    end
  end

  def test_unresolvable_base_revision_raises
    with_repo do |root|
      write_and_commit(root, { ROADMAP_PATH => BASE_ROADMAP }, "base")

      error = assert_raises(StatusPageFreshnessError) do
        check_status_page_freshness(root, "0000000000000000000000000000000000000000", "HEAD")
      end
      assert_match(/could not resolve revision/, error.message)
    end
  end

  def test_missing_roadmap_at_revision_is_treated_as_empty_and_signals
    with_repo do |root|
      base_sha = write_and_commit(root, { "README.md" => "no roadmap yet" }, "base")
      write_and_commit(
        root,
        {
          ROADMAP_PATH => BASE_ROADMAP,
          "site/status/index.html" => "<html>updated</html>"
        },
        "add roadmap"
      )

      result = check_status_page_freshness(root, base_sha, "HEAD")
      assert_match(/roadmap milestone\/evidence\/feature-paragraph signal matched/, result)
    end
  end

  def test_cli_usage_error_without_arguments
    output = `ruby #{CHECKER} 2>&1`
    refute Process.last_status.success?
    assert_match(/usage: check_status_page_freshness\.rb/, output)
  end

  # (g) D-156 documents an OR condition across the two watched pages: a
  # milestone-span change satisfied by `site/index.html` alone (with
  # `site/status/index.html` left untouched) must still pass. Every other
  # passing test above only ever touches site/status/index.html, so this is
  # the only coverage of the landing page satisfying the gate by itself.
  def test_milestone_change_with_landing_page_touch_alone_passes
    with_repo do |root|
      base_sha = write_and_commit(root, { ROADMAP_PATH => BASE_ROADMAP }, "base")
      changed_roadmap = BASE_ROADMAP.sub(
        "v0.2 — acceptance criteria met; v0.3 in progress.",
        "v0.3 — class model core landed."
      )
      write_and_commit(
        root,
        {
          ROADMAP_PATH => changed_roadmap,
          "site/index.html" => "<html>updated</html>"
        },
        "milestone + landing page only"
      )

      result = check_status_page_freshness(root, base_sha, "HEAD")
      assert_match(/roadmap milestone\/evidence\/feature-paragraph signal matched/, result)
      assert_match(%r{site/index\.html}, result)
    end
  end

  # (h) test_unresolvable_base_revision_raises only exercises
  # ensure_revision_available's fetch-*failure* branch (no `origin` remote
  # configured at all). Exercise the fetch-*success* branch end to end
  # against a real local `origin` remote, the path an actual shallow CI
  # checkout depends on: `root` never has `origin_sha` locally, but can
  # fetch it from `origin` once a remote is configured.
  def test_ensure_revision_available_fetches_a_missing_revision_from_a_real_origin_remote
    Dir.mktmpdir do |directory|
      workspace = Pathname(directory)
      origin = workspace / "origin"
      root = workspace / "root"
      FileUtils.mkdir_p(origin)
      FileUtils.mkdir_p(root)

      init_repo(origin)
      origin_sha = write_and_commit(origin, { "README.md" => "origin only" }, "origin commit")

      init_repo(root)
      write_and_commit(root, { "README.md" => "root only" }, "root commit")
      run_git!(root, "remote", "add", "origin", origin.to_s)

      _stdout, _stderr, before_status = Open3.capture3(
        "git", "cat-file", "-e", "#{origin_sha}^{commit}", chdir: root.to_s
      )
      refute before_status.success?, "test setup should not already have #{origin_sha} in root"

      ensure_revision_available(root, origin_sha)

      _stdout, _stderr, after_status = Open3.capture3(
        "git", "cat-file", "-e", "#{origin_sha}^{commit}", chdir: root.to_s
      )
      assert after_status.success?,
             "expected ensure_revision_available to fetch #{origin_sha} from origin into root"
    end
  end

  # (i) main()'s CLI success/failure paths with real <base-revision>
  # [head-revision] [repository-root] arguments, mirroring
  # scripts/test_check_roadmap_evidence.rb's Open3.capture3(RbConfig.ruby,
  # CHECKER.to_s, root.to_s) convention. Only the 0-argument usage-error
  # branch was previously exercised via direct CLI invocation.
  def test_cli_passes_with_real_arguments
    with_repo do |root|
      base_sha = write_and_commit(root, { ROADMAP_PATH => BASE_ROADMAP }, "base")
      changed_roadmap = BASE_ROADMAP.sub(
        "v0.2 — acceptance criteria met; v0.3 in progress.",
        "v0.3 — class model core landed."
      )
      write_and_commit(
        root,
        {
          ROADMAP_PATH => changed_roadmap,
          "site/status/index.html" => "<html>updated</html>"
        },
        "milestone + status page"
      )

      stdout, stderr, status = Open3.capture3(
        RbConfig.ruby, CHECKER.to_s, base_sha, "HEAD", root.to_s
      )
      assert status.success?, stderr
      assert_match(/Status page freshness check passed/, stdout)
    end
  end

  def test_cli_fails_with_real_arguments
    with_repo do |root|
      base_sha = write_and_commit(root, { ROADMAP_PATH => BASE_ROADMAP }, "base")
      changed_roadmap = BASE_ROADMAP.sub(
        "v0.2 — acceptance criteria met; v0.3 in progress.",
        "v0.3 — class model core landed."
      )
      write_and_commit(root, { ROADMAP_PATH => changed_roadmap }, "milestone only")

      stdout, stderr, status = Open3.capture3(
        RbConfig.ruby, CHECKER.to_s, base_sha, "HEAD", root.to_s
      )
      refute status.success?
      assert_empty stdout
      assert_match(%r{site/status/index\.html}, stderr)
    end
  end

  # (j) a new feature-landing paragraph added to docs/ROADMAP.md WITHOUT a
  # watched-page touch -> must FAIL with the expected error. This is the
  # detection gap #522 identified: a new `**[#NNN](...) — ...:**` paragraph
  # under an existing milestone heading changes neither the current-milestone
  # line nor a roadmap-evidence checkbox, so the original two-signal check
  # reported `pass` while the status page drifted.
  def test_feature_paragraph_addition_without_status_page_touch_fails
    with_repo do |root|
      base_sha = write_and_commit(root, { ROADMAP_PATH => BASE_ROADMAP }, "base")
      added_paragraph = <<~MARKDOWN.chomp

        **[#1001](https://github.com/rotnov/pycc/issues/1001) — New feature:** description here.
      MARKDOWN
      changed_roadmap = BASE_ROADMAP + added_paragraph + "\n"
      write_and_commit(root, { ROADMAP_PATH => changed_roadmap }, "feature paragraph only")

      error = assert_raises(StatusPageFreshnessError) do
        check_status_page_freshness(root, base_sha, "HEAD")
      end
      assert_match(/feature-landing paragraph/, error.message)
      assert_match(/site\/status\/index\.html/, error.message)
    end
  end

  # (k) a new feature-landing paragraph added to docs/ROADMAP.md WITH a
  # watched-page touch -> must PASS.
  def test_feature_paragraph_addition_with_status_page_touch_passes
    with_repo do |root|
      base_sha = write_and_commit(root, { ROADMAP_PATH => BASE_ROADMAP }, "base")
      added_paragraph = <<~MARKDOWN.chomp

        **[#1001](https://github.com/rotnov/pycc/issues/1001) — New feature:** description here.
      MARKDOWN
      changed_roadmap = BASE_ROADMAP + added_paragraph + "\n"
      write_and_commit(
        root,
        {
          ROADMAP_PATH => changed_roadmap,
          "site/status/index.html" => "<html>updated</html>"
        },
        "feature paragraph + status page"
      )

      result = check_status_page_freshness(root, base_sha, "HEAD")
      assert_match(/feature-paragraph signal matched/, result)
    end
  end

  # (l) a text-only modification to an existing feature-landing paragraph
  # (same issue number, different description) WITHOUT a watched-page touch
  # -> must PASS (negative control). This pins the design choice that set
  # membership -- not text -- is the signal, so a description-only edit to
  # an existing paragraph does not fire.
  def test_feature_paragraph_text_only_edit_without_status_page_touch_passes
    with_repo do |root|
      base_sha = write_and_commit(root, { ROADMAP_PATH => BASE_ROADMAP }, "base")
      changed_roadmap = BASE_ROADMAP.sub(
        "description here.",
        "revised description with more detail."
      )
      write_and_commit(root, { ROADMAP_PATH => changed_roadmap }, "text-only paragraph edit")

      result = check_status_page_freshness(root, base_sha, "HEAD")
      assert_match(/no roadmap milestone, evidence-checklist, or feature-landing-paragraph signal/, result)
    end
  end

  # (m) an existing feature-landing paragraph REMOVED from docs/ROADMAP.md
  # WITHOUT a watched-page touch -> must FAIL. D-170 documents removal as
  # a signal (set membership shrinks), and this test verifies that claim
  # directly, complementing the addition tests (j/k) and the text-only
  # negative control (l).
  def test_feature_paragraph_removal_without_status_page_touch_fails
    with_repo do |root|
      base_sha = write_and_commit(root, { ROADMAP_PATH => BASE_ROADMAP }, "base")
      removed_paragraph = <<~MARKDOWN.chomp

        **[#999](https://github.com/rotnov/pycc/issues/999) — Test feature:** description here.
      MARKDOWN
      changed_roadmap = BASE_ROADMAP.sub(removed_paragraph + "\n", "")
      write_and_commit(root, { ROADMAP_PATH => changed_roadmap }, "remove feature paragraph")

      error = assert_raises(StatusPageFreshnessError) do
        check_status_page_freshness(root, base_sha, "HEAD")
      end
      assert_match(/feature-landing paragraph/, error.message)
      assert_match(/site\/status\/index\.html/, error.message)
    end
  end

  CHECKER = Pathname(__dir__) / "check_status_page_freshness.rb"
end
