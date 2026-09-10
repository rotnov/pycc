# 2026-09-10 (04) — issue #1000: accepted-decision immutability guard (Part 2 of #77)

## Status

Delivered by the pull request that carries this file. Base `4b317abe`
(`origin/main` at branch time, Part 1 / PR #1001 already merged); branch
`feat/issue-1000-decision-immutability-guard`, commits `50e4010a`
(`parse_frontmatter` split), `12d335bf` (checker, tests, CI step, D-171
binding), `b0a14064` (D-240 and the documentation sites), `e798d3f3`
(review round 1 fixes), `26068b98` (session file and harden journal),
`4e37311f` (bounded stub and status exemptions after the external review on
PR #1002), `d902f2be` (the stub's long-form status line must sit inside the
replaced body, before the frozen tail; review round 3).

## What landed

- `scripts/check_decision_immutability.py`: a base-vs-head guard over
  `docs/decisions/D-*.md`. A file whose base frontmatter `status` is
  `accepted` or `superseded` is frozen: every base line must reappear in
  order at head (exact greedy subsequence walk), except the frontmatter
  `status:` line and the first body `- Status:` line, which may be replaced.
  Both replacements must read `accepted` or `superseded` (an annotation may
  follow the body form). D-151 index-only stubs (marker at 0-based index 8,
  no `- Status:` line) may have their five-line stub body replaced, only by a
  long-form entry carrying such a `- Status:` line. Deletion, rename, and same-line
  appends (PR #74's shape) fail. `pull_request` compares `base.sha` to
  `GITHUB_SHA`, `push` compares `before` to `GITHUB_SHA`; missing objects are
  fetched at depth 1. Local form: `--base "$(git merge-base origin/main
  HEAD)" --head HEAD`. Malformed events, unknown statuses and unresolvable
  revisions exit 2; violations exit 1.
- `scripts/test_check_decision_immutability.py` (rule, real-corpus, plumbing
  and CI-wiring tests); `scripts/test_generate_decisions_index.py` extended.
- `.github/workflows/ci.yml`: a `governance` step after "Check decisions
  index freshness and id uniqueness"; `tests/fixtures/policy-successors/ci-d171.yml`
  and `D171_GOVERNANCE_POLICY_STEPS` / `D171_CHANGE_AWARE_CI_WORKFLOW_SHA256`
  in `scripts/check_roadmap_evidence.rb` rotated in the same pull request
  (PR #936 precedent; not the D-080 two-PR shape).
- `docs/decisions/D-240-accepted-decision-files-are-insert-only-and-ci-enforces-it.md`
  and the dispatching sites: `AGENTS.md`, `docs/decisions/TEMPLATE.md`, the
  regenerated `docs/decisions/README.md` preamble,
  `docs/REPOSITORY_GOVERNANCE.md`, `docs/TESTING.md`.

## Gates at HEAD

All governance steps reproduced locally, exit 0 (unittest 1105 OK, 6
skipped; roadmap-evidence 247 runs). The
checker exits 0 on this branch and exits 1 on a violator demo (same-line
append on D-032 plus deletion of D-239). D-068 review: `ievo:deep-reviewer`
round 1 — four note findings fixed in `e798d3f3`; round 2 clean; two Codex
P2 threads on PR #1002 (unvalidated stub replacement, unbounded status
replacement) fixed in `4e37311f`; round 3 found the document-global
status-line check could be unlocked by a decoy after a frozen tail, fixed in
`d902f2be`; round 4 verified it. Harden batch: three journal entries, no artefact shipped (see
`.harden/incidents/plan-contradicts-its-own-constraint/2026-09-10-issue-1000.md`
for the proposed `issue-to-plan` step 7 rule, pending an arena run).

## Known window

The base-owned `audit` job tolerates the new step until this pull request
merges (D-171 routing is validated by the head tree's checker only after
merge); the head-owned checker rejects an `echo skip` or `if: always()`
mutation of the step. Documented in D-240.

## Follow-ups and merge order

- Closing this pull request closes #1000 and #77 (all four Expected bullets
  of #77 are delivered: bullets 1-3 by #999 / PR #1001, bullet 4 here).
- The ci.yml edit selects the full CI matrix; expect the long run.

## Where to resume

`scripts/check_decision_immutability.py` and D-240 for the rule; the
`governance` job in `.github/workflows/ci.yml` for the wiring.
