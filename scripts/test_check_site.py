#!/usr/bin/env python3
"""Tests for scripts/build_site.py and scripts/check_site.py (D-259).

The tests build the real `site/` sources inside a fixture repository whose
`docs/ROADMAP.md` is synthetic and whose linked repository files are empty
placeholders, so an ordinary change to the real roadmap table or a renamed
document never turns this governance-discovered suite red (D-259: compiler
pull requests do not touch site machinery). Positive control: that fixture
builds and passes. Negative controls: each check rejects a minimal broken
input. The real-tree positive control runs only with PYCC_SITE_REAL_TREE=1,
which the `Pages` workflow sets. The tests inject the commit through
PYCC_SITE_COMMIT/PYCC_SITE_DATE so they never need a git checkout.
"""

from __future__ import annotations

import os
import shutil
import sys
import tempfile
import unittest
from pathlib import Path
from urllib.parse import urlsplit
from unittest import mock

SCRIPTS = Path(__file__).resolve().parent
REPO_ROOT = SCRIPTS.parent
sys.path.insert(0, str(SCRIPTS))

import build_site  # noqa: E402
import check_site  # noqa: E402

FAKE_COMMIT = {"PYCC_SITE_COMMIT": "0123456789abcdef0123456789abcdef01234567", "PYCC_SITE_DATE": "2026-01-02"}

ROADMAP = """# Roadmap

**Current milestone: v9 — in progress.** Prose.

| Area | Status in this commit | Evidence and remaining gap |
|---|---|---|
| Compiler | `pycc` <works> | [link](x) |
| Pipes | a \\| b | c |

After.
"""


def make_fixture_repo(root: Path) -> Path:
    """Copy the real site sources beside a synthetic roadmap and placeholder targets."""
    repo = root / "repo"
    shutil.copytree(REPO_ROOT / "site", repo / "site")
    (repo / "docs").mkdir()
    (repo / "docs" / "ROADMAP.md").write_text(ROADMAP)
    for source in (repo / "site").rglob("*"):
        if source.suffix not in (".html", ".txt"):
            continue
        for path in check_site.REPO_LINK_RE.findall(source.read_text(encoding="utf-8")):
            target = repo / urlsplit(path).path
            target.parent.mkdir(parents=True, exist_ok=True)
            target.touch()
    return repo


class SiteTestCase(unittest.TestCase):
    def setUp(self) -> None:
        self.tmp = Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, self.tmp)
        patcher = mock.patch.dict(os.environ, FAKE_COMMIT)
        patcher.start()
        self.addCleanup(patcher.stop)
        self.repo = make_fixture_repo(self.tmp)

    def build_fixture(self) -> Path:
        out = self.tmp / "_site"
        build_site.build(self.repo, out)
        return out

    def assert_rejected(self, site: Path, fragment: str) -> None:
        errors = check_site.check(site, self.repo)
        self.assertTrue(any(fragment in e for e in errors), f"expected {fragment!r} in {errors}")


class BuildTests(SiteTestCase):
    def test_fixture_site_builds_and_passes(self) -> None:
        out = self.build_fixture()
        self.assertEqual(check_site.check(out, self.repo), [])
        status = (out / "status" / "index.html").read_text()
        self.assertIn("0123456789ab", status)
        self.assertIn("2026-01-02", status)
        self.assertIn("Current milestone: v9", status)
        for route in build_site.RETIRED_ROUTES:
            self.assertIn("noindex", (out / route / "index.html").read_text())

    @unittest.skipUnless(os.environ.get("PYCC_SITE_REAL_TREE") == "1", "set by the Pages workflow")
    def test_real_tree_builds_and_passes(self) -> None:
        out = self.tmp / "real"
        build_site.build(REPO_ROOT, out)
        self.assertEqual(check_site.check(out, REPO_ROOT), [])

    def test_main_entry_point(self) -> None:
        out = self.tmp / "via_main"
        self.assertEqual(build_site.main(["--repo-root", str(self.repo), "--out", str(out)]), 0)
        # A rebuild replaces the earlier output, recognised by its marker file.
        self.assertEqual(build_site.main(["--repo-root", str(self.repo), "--out", str(out)]), 0)
        self.assertEqual(check_site.main([str(out), str(self.repo)]), 0)

    def test_existing_unmarked_output_is_not_deleted(self) -> None:
        out = self.tmp / "precious"
        out.mkdir()
        (out / "keep.txt").write_text("source")
        with self.assertRaisesRegex(build_site.BuildError, "not produced by build_site.py"):
            build_site.build(self.repo, out)
        self.assertTrue((out / "keep.txt").is_file())

    def test_output_inside_site_sources_is_refused(self) -> None:
        with self.assertRaisesRegex(build_site.BuildError, "inside site/"):
            build_site.build(self.repo, self.repo / "site" / "_out")

    def test_parse_roadmap_renders_inline_markdown(self) -> None:
        milestone, rows = build_site.parse_roadmap(ROADMAP)
        self.assertEqual(milestone, "Current milestone: v9 — in progress.")
        self.assertEqual(rows, [("Compiler", "`pycc` <works>"), ("Pipes", "a | b")])
        self.assertEqual(
            build_site.inline_markdown("`a<b>` [t](u) **x** <y>"),
            "<code>a&lt;b&gt;</code> t x &lt;y&gt;",
        )
        self.assertEqual(build_site.inline_markdown("[`x`](u)"), "<code>x</code>")

    def test_roadmap_without_milestone_fails(self) -> None:
        with self.assertRaisesRegex(build_site.BuildError, "Current milestone"):
            build_site.parse_roadmap(ROADMAP.replace("**Current milestone", "Current milestone"))

    def test_roadmap_without_table_fails(self) -> None:
        with self.assertRaisesRegex(build_site.BuildError, "no status table"):
            build_site.parse_roadmap(ROADMAP.replace("| Area | Status in this commit |", "| X | Y |"))

    def test_roadmap_with_empty_table_fails(self) -> None:
        text = ROADMAP.split("| Compiler")[0] + "\nAfter.\n"
        with self.assertRaisesRegex(build_site.BuildError, "no rows"):
            build_site.parse_roadmap(text)

    def test_roadmap_with_malformed_row_fails(self) -> None:
        with self.assertRaisesRegex(build_site.BuildError, "malformed"):
            build_site.parse_roadmap(ROADMAP.replace("| Pipes | a \\| b | c |", "| | x |"))

    def test_generated_route_in_source_is_rejected(self) -> None:
        repo = self.repo
        (repo / "site" / "status").mkdir()
        with self.assertRaisesRegex(build_site.BuildError, "generated at build time"):
            build_site.build(repo, self.tmp / "out")
        self.assertEqual(build_site.main(["--repo-root", str(repo), "--out", str(self.tmp / "o2")]), 1)

    def test_missing_site_directory_fails(self) -> None:
        with self.assertRaisesRegex(build_site.BuildError, "missing site source"):
            build_site.build(self.tmp, self.tmp / "out")

    def test_commit_info_without_git_or_override_fails(self) -> None:
        with mock.patch.dict(os.environ, {"PYCC_SITE_COMMIT": "", "PYCC_SITE_DATE": ""}):
            with self.assertRaisesRegex(build_site.BuildError, "cannot read the commit"):
                build_site.commit_info(self.tmp / "not-a-repo")


class CheckTests(SiteTestCase):
    def setUp(self) -> None:
        super().setUp()
        self.site = self.build_fixture()

    def edit(self, rel: str, old: str, new: str) -> None:
        path = self.site / rel
        text = path.read_text()
        self.assertIn(old, text)
        path.write_text(text.replace(old, new, 1))

    def test_missing_site_directory(self) -> None:
        self.assertIn("not a directory", check_site.check(self.tmp / "nope", self.repo)[0])
        self.assertEqual(check_site.main([str(self.tmp / "nope")]), 1)

    def test_missing_doctype(self) -> None:
        self.edit("index.html", "<!doctype html>", "")
        self.assert_rejected(self.site, "missing <!doctype html>")

    def test_unclosed_tag(self) -> None:
        self.edit("index.html", "</ul>", "")
        self.assert_rejected(self.site, "closes <ul>")

    def test_unexpected_end_tag(self) -> None:
        self.edit("404.html", "</html>", "</html></div>")
        self.assert_rejected(self.site, "unexpected </div>")

    def test_never_closed_tag(self) -> None:
        self.edit("404.html", "</html>", "")
        self.assert_rejected(self.site, "<html> opened on line")

    def test_void_end_tag(self) -> None:
        self.edit("404.html", "<main>", "<main><br></br>")
        self.assert_rejected(self.site, "void element <br>")

    def test_self_closing_syntax_on_void_element_is_accepted(self) -> None:
        self.edit("404.html", "<main>", "<main><br/>")
        self.assertEqual(check_site.check(self.site, self.repo), [])

    def test_self_closing_syntax_on_non_void_element_is_rejected(self) -> None:
        self.edit("404.html", "<main>", "<main><span/>")
        self.assert_rejected(self.site, "self-closing syntax on non-void element <span>")

    def test_html_link_to_missing_repository_file(self) -> None:
        self.edit("index.html", 'href="status/"', 'href="https://github.com/rotnov/pycc/blob/main/docs/GONE.md#x"')
        self.assert_rejected(self.site, "index.html links to docs/GONE.md, which does not exist")

    def test_broken_relative_link(self) -> None:
        self.edit("index.html", 'href="status/"', 'href="missing/"')
        self.assert_rejected(self.site, "broken link 'missing/'")

    def test_broken_absolute_link(self) -> None:
        self.edit("404.html", 'href="/pycc/status/"', 'href="/pycc/gone/"')
        self.assert_rejected(self.site, "broken link '/pycc/gone/'")

    def test_absolute_link_outside_project(self) -> None:
        self.edit("404.html", 'href="/pycc/status/"', 'href="/other/"')
        self.assert_rejected(self.site, "broken link '/other/'")

    def test_broken_fragment(self) -> None:
        self.edit("index.html", 'href="status/"', 'href="#nowhere"')
        self.assert_rejected(self.site, "#nowhere has no matching id")

    def test_broken_absolute_site_url(self) -> None:
        self.edit("index.html", 'href="status/"', 'href="https://rotnov.github.io/pycc/gone/"')
        self.assert_rejected(self.site, "does not resolve")

    def test_sitemap_rules(self) -> None:
        loc = "<loc>https://rotnov.github.io/pycc/status/</loc>"
        self.edit("sitemap.xml", loc, "<loc>https://example.com/</loc>")
        self.assert_rejected(self.site, "is outside")
        self.edit("sitemap.xml", "<loc>https://example.com/</loc>", "<loc>https://rotnov.github.io/pycc/</loc>")
        self.assert_rejected(self.site, "more than once")
        self.edit("sitemap.xml", "<loc>https://rotnov.github.io/pycc/</loc>", "<loc>https://rotnov.github.io/pycc/x/</loc>")
        self.assert_rejected(self.site, "does not resolve")

    def test_sitemap_malformed_and_missing(self) -> None:
        (self.site / "sitemap.xml").write_text("<urlset>")
        self.assert_rejected(self.site, "not well-formed")
        (self.site / "sitemap.xml").write_text('<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9"/>')
        self.assert_rejected(self.site, "no <loc>")
        (self.site / "sitemap.xml").unlink()
        self.assert_rejected(self.site, "sitemap.xml is missing")

    def test_robots_rules(self) -> None:
        (self.site / "robots.txt").write_text("User-agent: *\n")
        self.assert_rejected(self.site, "does not name the sitemap")
        (self.site / "robots.txt").unlink()
        self.assert_rejected(self.site, "robots.txt is missing")

    def test_llms_rules(self) -> None:
        self.edit("llms.txt", "# pycc", "# other")
        self.assert_rejected(self.site, "must start with '# pycc'")
        self.edit("llms.txt", "> pycc", "pycc")
        self.assert_rejected(self.site, "'> ' summary")
        self.edit("llms.txt", "docs/SPEC.md", "docs/NOPE.md")
        self.assert_rejected(self.site, "docs/NOPE.md, which does not exist")
        self.edit("llms.txt", "https://rotnov.github.io/pycc/status/", "https://rotnov.github.io/pycc/gone/")
        self.assert_rejected(self.site, "llms.txt: https://rotnov.github.io/pycc/gone/")
        (self.site / "llms.txt").unlink()
        self.assert_rejected(self.site, "llms.txt is missing")

    def test_missing_404(self) -> None:
        (self.site / "404.html").unlink()
        self.assert_rejected(self.site, "404.html is missing")


if __name__ == "__main__":
    unittest.main()
