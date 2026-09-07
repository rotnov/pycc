# 2026-09-07 (03) — issue #990: canonical page date pins vs. the merge date

## Overall status

Issue #990 is implemented on the task branch `autopilot/iter-2026-09-07-29`,
branched from `origin/main` at `7bc37e03609f03ad556c5377d1b4994cf8a7040a`
("fix(site): rotate the status-page date pins to the merge date (#987) (#991)").
The work is committed and pushed as pull request
[#993](https://github.com/rotnov/pycc/pull/993); nothing is merged by this
session — the orchestrating session reviews the diff and merges.

## What the change does

A pull request that edits a canonical page under `site/` on day N and pins its
dates to day N passes `scripts/check_sitemap_lastmod.rb` on its own
pull-request leg, then fails that identical check on `main` once it is
squash-merged on day N+1 — a squash commit's author date is the merge instant,
not the branch's last commit time. Commit `7bc37e03` is the repository's own
most recent instance of paying for that after the fact.

Landed in this change:

- `scripts/site_canonical_pages.rb` — the canonical-URL → source-file map,
  extracted out of `scripts/check_sitemap_lastmod.rb` so two checkers can share
  one definition. That checker could not simply be `require`d: through this
  commit's parent it ran `check!` at load time. It now also carries a
  `$PROGRAM_NAME == __FILE__` guard.
- `scripts/check_site_pin_merge_currency.rb` + its unit test — compares the
  head tree's pins against the date the prospective squash commit is predicted
  to carry. The diff is evaluated *before* any date reasoning, so a pull
  request touching no canonical page source passes unconditionally; only then
  is the offset derived (from the base revision's own author date) and the
  fail-closed rollover guard applied. That ordering is pinned by its own test.
- Wiring: `.github/workflows/ci.yml`'s `governance` job (load-bearing —
  `ci-gate` is a required context and fails when `governance` fails) and
  `.github/workflows/pages.yml`'s unprivileged `build` job (defense in depth).
  Both are pull-request-leg only, deliberately; the push leg is a documented
  no-op. All three new script files were added to both legs of `pages.yml`'s
  name-enumerated `paths:` filter.
- Docs: `AGENTS.md` carries the literal pre-merge invocation, D-239 records the
  decision and the rejected alternatives, `docs/WEBSITE.md`'s false "caught
  before merge" claim is corrected and the four-pin set documented, and
  `.claude/skills/issue-implement/SKILL.md` step 8 points at the `AGENTS.md`
  rule rather than restating it.

## Known follow-ups

- The prediction is an estimate. If the merging identity's UTC offset changes,
  the checker can be wrong for exactly one merge;
  `scripts/check_sitemap_lastmod.rb` on `main` remains the authority on the
  committed result.
- Inside the UTC/offset rollover window the checker fails closed for
  site-touching pull requests. That is intentional (the correct pin is
  unknowable there), but if it ever becomes an operational nuisance the remedy
  is to land site changes outside the window, not to soften the guard.

## Where a fresh session should look

`docs/decisions/D-239-check-canonical-page-date-pins-against-the-predicted.md`
for the reasoning, `docs/WEBSITE.md`'s "Canonical page date pins and the merge
date" section for the contract, and `AGENTS.md`'s "Rotate canonical page date
pins before merging" for the obligation.

Other open pull requests at this checkpoint: #992 (`feat/issue-974`), untouched
by this session.

## Codex review round on pull request #993

A Codex review of #993 found that the checker's success path validated only the
sitemap `<lastmod>`. When a canonical page is edited across a date boundary,
rotating that one pin to the predicted date and recomputing the HTML manifest
digest is enough to keep every required `ci-gate` job green, because only the
non-required `Pages` workflow runs `scripts/check-site.sh` — so the required
pre-merge checker could approve a merge that immediately left `Pages` red on
the JSON-LD `dateModified` and `PAGE_SPECS` dates. The finding was verified as
real and fixed in the same branch: `scripts/check_site_pin_merge_currency.rb`
now validates all four pins at the head revision for every touched canonical
page source, reporting every stale pin for every affected page in one failure.
Pins that are genuinely absent from an input (the landing page has no
`PAGE_SPECS` entry, and `scripts/check-site.sh` or the performance manifest may
not exist at a given revision) are recorded as skips in the success summary
rather than turned into diagnostics that would compete with the checkers that
own them. `docs/decisions/D-239-...` and `docs/WEBSITE.md` were updated in the
same commit, and `scripts/test_check_site_pin_merge_currency.rb` gained cases
for each newly validated pin, for the sitemap-only rotation the finding
describes, and for each skip path — plus two tests that run the new
`PAGE_SPECS` and JSON-LD parsers against the real repository files, so a
reindented heredoc or a restructured `@graph` cannot silently degrade those
pins into permanent skips.
