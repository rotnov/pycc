# Session handoff: issue #401 (GitHub Pages status/roadmap sync) merged

- Date: 2026-08-07
- Trigger: user-directed `/issue-implement 401` (not the `issue-select` autopilot loop)
- Terminal state: **merged**. PR [#407](https://github.com/rotnov/pycc/pull/407) squash-merged
  into `main` as `722b90bd2c7a0b6b9dc4a69f09b8934161d5ef35`; issue
  [#401](https://github.com/rotnov/pycc/issues/401) closed automatically via `Fixes #401`.
  `origin/main` tip at the time this entry was written: `722b90b`.

## What landed

- `scripts/check_status_page_freshness.rb` + `.github/workflows/status-page-freshness.yml`: a
  new, non-required CI check enforcing
  [D-156](../decisions/D-156-status-page-freshness-check-two-signal-design.md) — when
  `docs/ROADMAP.md`'s current-milestone line or a `roadmap-evidence` checklist entry changes,
  the same diff must also touch `site/status/index.html` or `site/index.html`.
- `site/index.html`, `site/status/index.html`, `site/sitemap.xml`: refreshed to reflect v0.2
  acceptance being met and v0.3 (class model core, #385) being in progress — the actual content
  drift #401 reported.
- `docs/decisions/D-156-...md`, `docs/decisions/README.md`, `docs/WEBSITE.md`: new decision
  record and updated maintenance guidance.
- `scripts/check-site.sh`, `scripts/test-check-site.sh`: minor supporting fixes (see PR #407
  diff for detail; not re-summarized here).

## Process notes worth keeping

- The core technical fix went through two review-driven corrections after the first
  implementation pass: (1) the milestone-signal regex initially compared only the bold
  `**Current milestone: ...**` span, missing edits to trailing status prose on the same physical
  line — fixed by comparing the whole line (`milestone_line` in
  `scripts/check_status_page_freshness.rb`); (2) `docs/decisions/D-156-...md` originally claimed
  the check "reuses" `scripts/check_roadmap_evidence.rb`'s `EVIDENCE_MARKER` regex — it actually
  duplicates it byte-for-byte with no `require_relative` link, corrected in both D-156 and
  `docs/WEBSITE.md`.
- Three full D-068 `ievo:deep-reviewer` rounds ran against this PR (0 blockers surviving
  verification each time); the third round's two warnings were fixed directly rather than via a
  dispatched agent, given their small, independently-verifiable, doc-prose-only scope — a
  deliberate, reasoned departure from the default dispatch pattern, not an oversight.
- The task branch needed **two** rebases onto a moving `origin/main` during this session (PR
  #357/D-147 merge, then PR #406's unrelated session-log-only merge) — both were zero-file-overlap,
  clean, conflict-free rebases. Local gate suite and the D-103 policy-successor-manifest
  zero-overlap check were re-run after each rebase before pushing.
- All 18 required and non-required CI checks passed on the final head
  (`d66c5c348a603237cbfbfd2178ce2ccc04ea286c`), including the new `status-page-freshness` check
  observing itself green on its own `pull_request` trigger.

## Explicit follow-up not done here

D-156 deliberately merged the new workflow **without** registering it as a required
branch-protection check — registration needs its own observe-then-register sequence (a green
`push`-to-`main` run is a different code path than the `pull_request` trigger this PR's own CI
already exercised, plus a deliberate red-run confirmation on a throwaway PR). Filed as
[issue #408](https://github.com/rotnov/pycc/issues/408) so the follow-up isn't lost; not started
in this session.

## Where a fresh session should look to resume

- No task branch remains — `task/issue-401-github-pages-roadmap-sync` was deleted (both remote,
  via `gh pr merge --delete-branch`, and should be deleted locally by whichever worktree still
  has it checked out).
- Issue #408 (branch-protection registration for `status-page-freshness.yml`) is open and
  unstarted — a reasonable next pick, though it is P3 and not milestone-blocking.
- Whether to re-enter the `issue-select`/`issue-implement` v0.3 autopilot loop after this session
  is an open question the user has not yet answered — this session's trigger was a single named
  issue, not a standing autopilot directive, so there is no automatic continuation.
