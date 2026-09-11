# 2026-09-11 (03) — #1006 (Part 1 of #566): Status hero as a commit-bound required-check snapshot

Status: draft; to be delivered by the pull request that carries this file;
base `origin/main` `4111208c` (re-fetched before the final commit; unchanged
throughout the session). Branch `feat/issue-1006-status-hero`.

## What changed

The Status page's evidence hero was the last first-screen claim about the
project's CI state and was still explicitly `unavailable` under D-186. It is
now a checked-in, sanitized snapshot of the required checks for exactly one
default-branch revision, refreshed offline by an agent and validated by the
same hermetic Pages gate as the other heroes (D-241). Nothing at build or
deploy time contacts GitHub.

- Contract and validator: `scripts/site_status_evidence.py` (`expected_shape`,
  `derive_state`, `validate`, `validate_projection`, `summary`) beside the
  execution module; `scripts/check_status_snapshot.py` CLI (`--verify-git`
  for the Pages full-history checkout, `--currency` for the pull-request leg,
  structural-only otherwise); `scripts/test_check_status_snapshot.py`
  (shallow-safe, synthetic `git init` repositories, stubbed collector shapes)
  discovered by the `governance` job. `scripts/check_site_evidence.py`
  dispatches the non-`unavailable` `status` record to the new module and the
  schema is `2.1.0`.
- Collector: `scripts/collect_status_snapshot.py` over read-only `gh api`
  (commits, pulls, paginated check-runs); unknown-not-green; rewrites only the
  `status` record; `scripts/test_collect_status_snapshot.py` stubs `gh`.
- Record: `site/evidence-heroes.json` `status` is `all-Tier-1` for main
  `4111208c0910c0a7588408a6ac7d2b2bad20592c` (PR #1005, head `984eb6f2…`,
  same tree, one parent): `ci-gate` success (run `34552229293`, completed
  2026-09-11T02:10:46Z), `audit` success on the head (completed
  2026-09-11T01:50:30Z), five Tier-1 jobs success; captured
  2026-09-11T03:09:20Z. The record pins the collector's and its suite's
  canonical SHA-256, so it was re-collected with
  `--collected-at 2026-09-11T03:09:20Z` after the suite changed.
- Gate wiring: `scripts/check-site.sh` runs `check_status_snapshot.py
  --verify-git` after `check_site_evidence.py`; `scripts/test-check-site.sh`
  stages the two pinned scripts, retargets every status-as-`unavailable`
  anchor, adds `status_mutation()` (commit/tree/parent-count co-mutations),
  and runs the new Pages-only `scripts/site_status_evidence_test.py`;
  `scripts/site_execution_evidence_test.py` tolerates a snapshot without
  `artifacts`; `.github/workflows/pages.yml` lists the five new inputs on
  both events and runs the currency step on the pull-request leg only;
  `scripts/test_site_status_wiring.py` binds all of it.
- Page and projections: `site/status/index.html` (`en-US`, `all-Tier-1`,
  one `page-meta` summary line, a `<details class="hero-evidence-details">`
  with the three subject rows and five job rows, the milestone line and the
  limitations) trimmed to fit the 25,600-byte budget; `site/index.html.md` and
  `site/llms.txt` carry the one-line `summary(hero)`; the Markdown landing's
  context budget rose 13312 -> 13824 bytes (aggregate 278016 of 278528);
  sitemap `lastmod`, JSON-LD `dateModified`, `check-site.sh` `date_modified`
  and the performance-manifest SHA were rotated.
- Documentation: D-241 (narrowly supersedes D-230's "five unavailable heroes"
  consequence), decisions index, `docs/WEBSITE.md` (schema `2.1.0`, the
  status record shape, the gate/convention table, the four remaining
  `unavailable` heroes, the context-budget allocation sentence),
  `docs/ROADMAP.md`, `docs/SPEC.md`.

## Forks resolved (D-127)

- Nested-field-required mirror would not reject deleting one `test.names`
  entry, because the validator only checked that each listed name exists.
  Resolved by tightening the validator to require the full ordered `def
  test_*` list (unit-tested, record re-collected) rather than weakening the
  mirror test.
- `site_execution_evidence_test.py`/`site_status_evidence_test.py` stage
  `snapshot.artifacts` for every non-null-fixture hero; the landing snapshot
  is a single `path` and the status snapshot holds subjects. Resolved by a
  narrow guard on the artifact line only (plan item 6a), so the landing hero
  is still staged.
- The 25,600-byte page budget (29,069 bytes after the proof rows) was met by
  four rounds of prose trims that keep all 18 `PAGE_SPECS` literals, the
  current-scope literals and the disclosure phrases, rather than by a budget
  raise (the plan's stated precedent).

## Gates at the delivering tree (all exit 0; only this file changed after the run)

`bash scripts/check-site.sh` ("Website checks passed."); `bash
scripts/test-check-site.sh` ("Website validator self-tests passed.");
`ruby scripts/check_pages_performance_budget.rb --skip-lighthouse`
(`site/status/index.html` 25,532 of 25,600 bytes); the `scripts/` unittest
suite (1180 tests, OK, 6 skipped); `scripts/site_status_evidence_test.py`
(10 tests, 111 nested-field subtests, OK); `check_site_evidence.py`;
`check_status_snapshot.py --verify-git` and `--currency --base origin/main
--head HEAD` (0 first-parent merges behind); `check_ci_permissions.rb`
(9 files); `check_roadmap_evidence.rb` and its 247-run suite;
`check_status_page_freshness.rb origin/main HEAD`; `validate_agent_policies.py`;
`validate_agent_assets.py`; `generate_decisions_index.py --check`;
`check_decision_immutability.py`; `classify_ci_changes.py --event-name
pull_request` (compiler=true, pages=true, agent=true; no Rust touched, so
D-014 is unaffected); `check_site_pin_merge_currency.rb`; the execution and
status wiring suites (17 tests). `site/index.html.md` is 13,760 of 13,824
bytes; the context aggregate is 278,016 of 278,528.

## Review rounds

(filled by the orchestrating session)

## Follow-ups

- #1007 (Part 2 of #566): the Architecture hero. #566 stays open.
- #408: `status-page-freshness` is still not a required context; the new
  currency step is deliberately Pages-only and not a merge gate either.
- The #802 checklist tick for the compiler-driver card (`init`/`explain` now
  listed as implemented on the Status page) is a `gh` write left for the
  orchestrating session.

## Where to resume

`git log --oneline -1 origin/main`; `python3 scripts/collect_status_snapshot.py`
against the current tip if `site/status/index.html` or the `status` record is
edited again; open issues in milestone v0.4.
