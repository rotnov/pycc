---
id: D-259
title: "Replace the Pages site with a small generated site checked only for structure"
status: accepted
---

## D-259: Replace the Pages site with a small generated site checked only for structure

- Status: accepted
- Context: The GitHub Pages site had grown into seven canonical pages backed by
  a versioned evidence-hero manifest (D-186, D-230, D-241, D-243), four
  hand-maintained date pins per page that had to be rotated against the
  predicted merge date (D-239), a ROADMAP-diff freshness workflow (D-156,
  D-170), a hermetic Lighthouse performance budget and an accessibility gate
  wired into `ci-gate` (D-161, D-162), an llms.txt aggregate context budget
  (D-200, D-218, D-227), and prose checkers that pinned page and roadmap
  wording phrase by phrase. Compiler pull requests regularly had to touch this
  machinery: a ROADMAP edit tripped the freshness workflow or a wording
  checker, a page edit needed four pins rotated and a manifest digest
  recomputed, and a merge on the day after a page edit turned `Pages` red on
  `main`. The repository owner asked for the site to be rewritten from scratch
  so that it stops blocking compiler work (umbrella #802).
- Decision:
  1. `site/` holds only hand-written static sources: a landing page, a 404
     page, `llms.txt`, `robots.txt`, `sitemap.xml`, one small stylesheet, the
     favicon, the social image, the IndexNow key file, and
     `site/search-visibility/evidence.json` (still read by
     `scripts/check_search_visibility.rb`). No source file carries a
     hand-maintained date.
  2. `scripts/build_site.py` builds the published tree into `_site/` (gitignored).
     It generates the status page at build time from `docs/ROADMAP.md`'s
     "Current milestone" line and the Area and Status columns of its status
     table, stamped with the commit SHA and commit date read from git, and it
     writes `noindex` redirect stubs for the five retired routes
     (`/ai-native/`, `/architecture/`, `/diagnostics/`, `/language-support/`,
     `/python-aot-compilers/`) so inbound links keep resolving.
  3. `scripts/check_site.py` is the only site gate. It checks structure, never
     wording, dates, sizes or performance: every page has a doctype and
     balanced tags; every internal link, asset and same-page fragment
     resolves; sitemap URLs are unique, under the site root and resolve;
     `robots.txt` names the sitemap; `llms.txt` has its title and summary and
     its site and repository links resolve; `404.html` exists.
     `scripts/test_check_site.py` carries the positive and negative controls
     and runs in the governance job's unittest discovery.
  4. The `Pages` workflow builds, checks and deploys `_site/`. Its deploy job
     keeps the exact `push` + `refs/heads/main` guard and the protected
     `github-pages` environment, and `notify-indexnow` keeps its existing
     contract. Both the push and pull-request legs are path-filtered to the
     site sources, `docs/ROADMAP.md`, and the checkers the workflow runs, so a
     ROADMAP change redeploys the status page and IndexNow is notified once
     per such deploy; resubmitting two unchanged URLs is accepted.
  5. Retired: `scripts/check-site.sh` and its test, the date-pin machinery
     (`check_sitemap_lastmod.rb`, `check_site_pin_merge_currency.rb`,
     `site_canonical_pages.rb`), the Lighthouse performance budget and the
     accessibility gate (their `ci.yml` jobs, scripts, fixtures and
     `ci-gate` clauses), the evidence-hero manifest and its Python and Rust
     checkers (`tests/architecture_manifest.rs`), the status-snapshot
     collector and checker, the source-link registry and live-link checkers,
     the Markdown landing and its checker, the llms.txt context manifest and
     budget, the `status-page-freshness.yml` and `link-check.yml` workflows,
     and the five observation checkers' bindings to `docs/ROADMAP.md` and
     `docs/WEBSITE.md` wording. The roadmap's "Public evidence and
     discoverability" row is ordinary prose again.
  6. This decision supersedes D-156, D-161, D-162, D-170, D-186, D-200,
     D-218, D-227, D-230, D-239, D-241 and D-243 in full, and narrowly
     supersedes D-171's clause that keeps "both Pages quality gates" required:
     the classifier, the coverage, Tier-1, cross-build and performance
     clauses of D-171 stand unchanged.
  Untouched, with reasons: D-050 and D-172 (the base-owned `audit` trust
  anchor and its activation sequence; this change followed their
  checker-first two-PR rule, with #1403 teaching the base checker the
  retired-gates shape first); D-165, D-167 and D-168 (the search-visibility
  and traffic evidence records, whose checkers keep running in the `Pages`
  workflow; only their prose bindings to the roadmap and to `WEBSITE.md`
  wording were dropped, not the evidence contracts); D-201 (the scratch-crate
  lint gate, which only mentions the former evidence-hero byte pin on
  `tests/quick_start.rs` as history; that file has since been migrated); and
  D-244 (an interop decision that only mentions the site in passing).
- Alternatives: Keep the pages and make every Pages check advisory: rejected
  because the cost was the machinery itself (pins, manifests and wording
  bindings that compiler pull requests must keep consistent), not only its
  failure mode. Keep a static hand-written status page: rejected because a
  hand-written status page is exactly the drift D-156 and D-170 tried to
  police; generating it from the roadmap removes the drift instead of
  detecting it. Use a static-site generator: rejected as a new toolchain
  dependency for two pages. Keep a Lighthouse budget as advisory telemetry:
  rejected because two small static pages with no script do not need one and
  an unread report is noise.
- Consequences: Compiler pull requests no longer touch site machinery: a
  ROADMAP edit just redeploys the status page. The site makes fewer claims:
  the per-page evidence heroes, the compiler comparison and the AI-native
  page are gone, and their substance lives in the repository documents the
  landing page and `llms.txt` link to. The previous pages' URLs redirect to
  the landing page with `noindex`, so search engines drop them over time. The
  `tests/fixtures/architecture-trace/` trace and `tests/site_evidence.rs`
  stay as compiler regression tests. The fail-closed classifier still names
  the retired Pages inputs and still emits a `pages` output that no job
  consumes; retiring those strings needs another checker-first round and is
  tracked as a follow-up under umbrella #802.
