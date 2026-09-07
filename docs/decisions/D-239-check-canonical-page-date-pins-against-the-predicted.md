---
id: D-239
title: "Check canonical page date pins against the predicted merge date"
status: accepted
---

## D-239: Check canonical page date pins against the predicted merge date

- Status: accepted
- Context: Every canonical page under `site/` carries four date pins that must
  agree with each other and with git history: the `<lastmod>` for the page's
  `<loc>` in `site/sitemap.xml`, the JSON-LD `dateModified` in the page's own
  HTML, `PAGE_SPECS["<page>"]["date_modified"]` in `scripts/check-site.sh`, and
  — because it digests the page HTML — `source_artifact_sha256` for that page
  in `tests/fixtures/pages-performance-manifest.json`.
  `scripts/check_sitemap_lastmod.rb` binds the first of those to the author
  date of the last non-merge commit that touched the page source. That
  contract is correct for any committed tree, but it is structurally incapable
  of catching one failure mode. `main`'s history is a near-linear chain of
  squash commits (D-024 routes every change through a pull request; the
  occasional true merge commit aside, the landed shape is a squash), and a
  squash commit's author date is the *merge instant* in the merging identity's
  timezone — not the branch's last commit time. So a
  pull request that edits a page on day N and pins day N passes its own
  pull-request leg honestly, and then fails the identical check on `main` the
  moment it is squash-merged on day N+1, turning the `Pages` workflow red on
  the default branch where it is most expensive to notice and repair. The
  window is not exotic: an autonomous run that opens a pull request in the
  evening and merges after CI routinely crosses a date boundary. Commit
  `7bc37e03` ("rotate the status-page date pins to the merge date") is the
  repository's own most recent instance of paying for it after the fact.
- Decision: Add `scripts/check_site_pin_merge_currency.rb`, a second, distinct
  validator that runs on the pull-request leg and compares the head tree's pins
  against the date the prospective squash commit is *predicted* to carry. It
  computes the diff first and returns success immediately when no canonical
  page source is touched; only then does it derive the date. "Today" means what
  `git log --format=%as` would report — an author date rendered in the merging
  identity's UTC offset — so the offset is estimated from the base revision's
  own author date (the base is `main`'s tip or the merge base with it, itself a
  prior squash by the same identity), falling back to UTC when it cannot be
  parsed. When the UTC date and the offset date disagree the checker fails
  closed rather than choosing one, because inside that rollover window no pin
  value can be verified as correct. Failure enumerates all four pins for every
  affected page plus the manifest-digest recompute. The checker is wired into
  `.github/workflows/ci.yml`'s `governance` job — the load-bearing placement,
  since `ci-gate` is a required context and fails when `governance` fails — and
  into `.github/workflows/pages.yml`'s unprivileged `build` job as defense in
  depth. `AGENTS.md` carries the literal pre-merge invocation, since a merge
  obligation recorded only in a Claude Code skill would be a single-platform
  workflow.
- Alternatives:
  - *Relax `scripts/check_sitemap_lastmod.rb` to accept a date within a
    tolerance window.* Rejected: it would weaken the one check that currently
    proves a pin corresponds to a real content change, trading a loud,
    correctly-timed failure for a silent class of stale pins.
  - *Derive the pin from merge-commit topology after the fact — teach the
    post-merge checker to look through the squash commit to the branch's own
    history.* Rejected: a squash merge deliberately discards that history, so
    on `main` there is nothing left to look through. The information the
    approach needs does not exist at the point it would run.
  - *Rewrite the pins automatically at merge time.* Rejected: it requires a
    privileged job writing to `main` from pull-request-influenced state, which
    the repository's CI privilege boundaries forbid, and it would silently
    mutate a reviewed diff.
  - *Document the obligation without a checker.* Rejected: this defect has
    already recurred, and the project's own rule is that a normative claim
    should be enforceable by a check wherever practical.
  - *Extend `scripts/check_sitemap_lastmod.rb` in place instead of adding a
    file.* Rejected: the two validators answer different questions against
    different inputs — one reads the working tree and git history, the other
    reads a specific head revision and a predicted future date — and that
    checker had no `$PROGRAM_NAME` guard, running its validation at load time.
    It gains the guard here, and the shared canonical-URL map moves into
    `scripts/site_canonical_pages.rb` so the two cannot drift apart.
- Consequences: A pull request that edits a canonical page now has to pin the
  date its merge will actually carry, so a branch left open across a date
  boundary must rotate its pins (and recompute the manifest digest) before it
  can merge — slightly more work on exactly the changes that were already
  silently failing. In exchange, `Pages` no longer breaks on `main` after a
  merge for this reason. Inside the rollover window the checker fails closed
  for site-touching pull requests, which is deliberate: the correct pin is
  genuinely unknowable there, and the remedy is to merge outside the window.
  Pull requests that touch no canonical page source are unaffected in every
  case, including inside that window. The prediction is an estimate, not a
  guarantee: if the merging identity's timezone offset changes, the checker can
  be wrong for one merge, and `scripts/check_sitemap_lastmod.rb` on `main`
  remains the authority on the committed result. The rule is also correct under
  a true merge commit rather than a squash: rotating a pin to the predicted
  date necessarily creates a branch commit on that same date, so the
  `--no-merges` binding the post-merge checker enforces still holds.
