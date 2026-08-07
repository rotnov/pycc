---
id: D-156
title: "Enforce GitHub Pages status/landing freshness with a two-signal ROADMAP.md diff check (#401)"
status: accepted
---

## D-156: Enforce GitHub Pages status/landing freshness with a two-signal ROADMAP.md diff check (#401)

- Status: accepted
- Context: #401 found `site/status/index.html` and `site/index.html` stuck on stale v0.1-only
  claims: both v0.1 and v0.2 acceptance are now met, v0.3 (class model core, #385/PR-15) has
  landed, and the pages' own `>2%` performance-regression figure was superseded by D-114's
  `>7.0%` months earlier. Nothing enforced that these two GitHub-Pages-published surfaces track
  `docs/ROADMAP.md`, so they drifted silently. Two mechanisms were considered to close this gap:
  (a) a narrowly-scoped CI check that fails a pull request when `docs/ROADMAP.md`'s status
  changes without a corresponding page update, or (b) fully auto-generating the pages' status
  content from `docs/ROADMAP.md` at build time.
- Decision (mechanism (a), not (b)): a new CI check,
  `scripts/check_status_page_freshness.rb`, watches `docs/ROADMAP.md` for two specific,
  mechanically detectable signals in a pull request's or push's diff: (1) the
  `**Current milestone: ...**` bold line changing, and (2) a `<!-- roadmap-evidence: ... -->`
  -tagged checklist line's checked state flipping, or a new evidence-marker line being added.
  Signal detection reuses `scripts/check_roadmap_evidence.rb`'s existing `EVIDENCE_MARKER` regex
  rather than reinventing marker parsing, matching this project's existing convention of only
  one parser per marker format. When either signal fires and the same diff touches *neither*
  `site/status/index.html` *nor* `site/index.html`, the check fails with a message pointing at
  this issue and at `docs/WEBSITE.md`. Full auto-generation (mechanism (b)) is rejected as
  impractical: the status/landing pages carry hand-tuned narrative prose (which v0.3 subset has
  landed, which gaps are real gaps versus roadmap shorthand) that a mechanical
  `docs/ROADMAP.md`-to-HTML transform cannot reproduce without effectively re-deriving the
  roadmap's own editorial judgment in a second format.
- Decision (touching *either* watched page satisfies the gate, not both): the gate's diff
  condition is an OR across `site/status/index.html` and `site/index.html` — updating either
  page in the same diff as a firing signal passes the check. The alternative (require both pages
  touched on every firing signal) was considered, since both pages currently duplicate the same
  milestone claim and requiring only one touched could in principle let the other silently drift
  again exactly as #401 describes. It was rejected because it would fire on every legitimate
  status-page-only content refresh that does not also touch the landing page's four status
  bullets (or vice versa), training maintainers to treat the gate as noise — the same
  over-firing risk this decision's two-signal design (versus an unconditional "any ROADMAP.md
  diff" trigger) was already built to avoid. The residual risk of one watched page drifting
  while the other is touched is accepted for this PR and left to ordinary review judgment; a
  stricter both-pages requirement can be revisited later if drift recurs in practice.
- Decision (wiring): a new, dedicated workflow, `.github/workflows/status-page-freshness.yml`,
  not `.github/workflows/ci.yml` and not `.github/workflows/workflow-policy.yml`. `ci.yml` is one
  of the 22 protected targets in `tests/fixtures/policy-successor-manifest.json` (D-103's
  propose/activate steady state) — editing it directly would require a two-PR propose/activate
  sequence this issue does not need, since the new check is entirely new, additive
  functionality with no predecessor to retire. `workflow-policy.yml` is the D-025 trust anchor
  itself, built around a `pull_request_target` / git-free content-download model that does not
  fit a check that needs an ordinary two-dot `git diff` between a PR's base and head. The new
  workflow runs on ordinary (non-`_target`) `pull_request` (opened, synchronize, reopened) and on
  `push` to `main`, with a top-level `permissions: contents: read` block (required by
  `scripts/check_ci_permissions.rb` on every workflow file) and no `paths:` filter on either
  trigger — deliberate, since a path filter would permanently block non-matching PRs from
  merging once this check is later registered as required. Registration as a required
  branch-protection check is an explicit, separate follow-up performed only after this PR merges
  and the workflow is observed green on a real push-to-main run and red on a real violating PR.
- Decision (diff mechanics): the check takes an explicit base revision and an explicit head
  revision and diffs them with a two-dot `git diff --name-only`, not a three-dot merge-base
  diff — a shallow CI checkout does not have enough history to compute a merge base. For
  `pull_request` events the base is `github.event.pull_request.base.sha` (a fixed commit), not
  `$GITHUB_BASE_REF` (a branch name that can move between the workflow starting and the check
  running), and the head is `github.event.pull_request.head.sha` rather than the literal
  checked-out `HEAD` — the workflow passes both explicit SHAs as `BASE_SHA`/`HEAD_SHA` env vars.
  For `push` events to `main`, the base is `github.event.before` and the head is `github.sha`.
  The script itself (not the workflow) resolves both the base and the head revision locally when
  possible and falls back to `git fetch --no-tags --depth=1 origin <revision>` otherwise for
  each, so the same code path works unmodified in a shallow CI checkout — where an explicit head
  SHA may not already be present the way the literal ref `"HEAD"` always is — and in a
  full-history local run.
- Empirical validation (performed against this repository's real history before finalizing,
  per this issue's plan): `21af3ae` ("Fix review findings: advance ROADMAP review date...", a
  pure date-refresh commit) does not fire — confirmed no milestone-line or evidence-marker
  change. `4a1e677` ("Close v0.1 acceptance checklist: all 5 criteria now green") fires on the
  evidence-marker signal, and — because that commit itself already updated both watched pages in
  the same change — the overall check still passes; this is the correct, self-consistent
  behavior for a commit that was already keeping the pages in sync manually. PR #388's merge
  commit (`d7963df`, class model core, #385) does not fire: it does not touch
  `docs/ROADMAP.md` at all (confirmed via `git show --stat d7963df -- docs/ROADMAP.md`
  producing no output), so neither signal has anything to compare. No signal-definition
  correction was needed; all three results matched the plan's stated expectations without
  adjustment.
- Consequences: a pull request that flips the roadmap's milestone or an evidence checkbox
  without touching either watched page now fails a required-eligible CI check with an actionable
  message, closing the exact silent-drift failure mode #401 reported. The check is
  intentionally narrow — it does not attempt to verify that the *content* of the page update is
  accurate, only that some update to a watched page accompanied the signal; content accuracy
  stays a review-time judgment call, same as before. `docs/ROADMAP.md` itself needed no content
  change from this decision. This entry supersedes nothing; it is new policy.
- Alternatives: full auto-generation from `docs/ROADMAP.md` (mechanism (b), rejected above); an
  unconditional "any `docs/ROADMAP.md` diff must touch a watched page" trigger (rejected — fires
  on routine prose-only edits with no status implication, training maintainers to ignore the
  gate); wiring into `ci.yml` or `workflow-policy.yml` (both rejected above); requiring both
  watched pages touched on every firing signal (rejected above, in the wiring decision).
