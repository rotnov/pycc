# 2026-09-02-03 -- Issue #712: count-free conformance-test wording in ci.yml comments

## Status: delivered

Worktree: `/Users/denis/projects/pycc-proto/.claude/worktrees/autopilot-2026-09-02`,
branch `autopilot/2026-09-02-iter`, started from `origin/main` at
`751f10c7c5d255f2079033fe4f350e111a7b8932` and fast-forwarded before
implementation to `157ca610` (#871 merged mid-planning; no conflicts, the
task branch had no commits yet). Issue
[#712](https://github.com/rotnov/pycc/issues/712) (milestone v0.4, P3, S)
was resolved via pull request
[#872](https://github.com/rotnov/pycc/pull/872), squash-merged as
`cef7f66fe8011e11a53fdcbad8a77ea658a18bf8`. The pull request's `closingIssuesReferences` count was verified
as 1 before merge; this handoff entry deliberately carries no closing
keyword.

## How this task was run

One autopilot iteration under the standing `fix all opened issues`
directive: D-021 preflight (branch protection matched the documented
baseline; `cargo doc --workspace --no-deps` regenerated from `751f10c7`),
then `issue-select` over the full open-issue list, then `issue-implement`
with `issue-to-plan` and the implementation each dispatched into an
isolated agent (D-142/D-143).

### issue-select

- 103 open issues inventoried. D-192 merge-quota window: 0 of 4 recent
  merges were non-milestone, so a non-milestone slot was open but unused.
  Non-milestone open count 71, above the ceiling of 20, so no new
  non-milestone issue could be filed this run.
- Staleness screen closed three issues with cited evidence: #151, #363,
  #364 (closure comments on each issue).
- No milestone assignments were made: the remaining unmilestoned issues
  are apparatus/D-185 items that already carry recorded triage notes.
- Ordering followed the current skill text (milestone-scope membership
  first, then P1 > P2 > P3 > unmarked, then smaller). The first pick
  (#25) was refuted by the adversarial advisor because the loaded skill
  text came from an older worktree; the second pick (#869) was displaced
  by #712 once the advisor showed the "D-103 two-PR cost" note that had
  disqualified #712 is stale after D-172. Round 3 was clean.
- Runners-up: #869, #729, #798, #25. In-scope disqualifications recorded
  in the selection justification: #414/#585 (L), #636 (blocked on D-124),
  #408 (repository settings), #706 (blocked on varargs), #641 (needs CI-run
  evidence), #866 (claimed by another session; merged as #871 during this
  run), #867/#868 (sequenced behind it), #747 (needs decomposition), #335
  (release automation), #336/#337 (owner design). Maintainer-attention
  items surfaced: #82 (live CI trust-boundary gap, D-203 two-PR), #44/#45
  (rescope after D-172), #558/#265/#75 (awaiting owner verdicts).

### issue-to-plan and implementation

- Plan: <https://github.com/rotnov/pycc/issues/712#issuecomment-5507825840>
  (two adversarial rounds). It found three stale `ci.yml` comment sites,
  not the two the issue names, plus `docs/ROADMAP.md:36`'s "two-fixture"
  phrase, and confirmed the issue's own count (48) is itself stale --
  `tests/conformance.rs` now carries 55 `#[ignore]` attributes -- hence
  count-free wording rather than a corrected number.
- Changes: `.github/workflows/ci.yml` comments at the coverage-gate step,
  the oracle-setup step, and the `cargo test --workspace` step now say
  "oracle-backed tests are `#[ignore]`d"; `docs/ROADMAP.md:36` says "an
  oracle-backed conformance check (`tests/conformance.rs` fixtures vs.
  pinned CPython 3.14.7, D-085)". The 13 historical ci.yml fixtures under
  `tests/fixtures/`, D-080, and ROADMAP:86 were left untouched by design.
- Gates run locally (all green): roadmap-evidence checker and its tests,
  ci-permissions checker and its tests, README projection/badge checkers,
  `check-site.sh` (within D-218's 272 KiB llms.txt budget),
  scripts unittest (981 tests), agent policy/asset validators,
  conformance-breadth, scratch-dir, search-visibility, status-page and
  harden-findings checkers, `cargo fmt --check`, `cargo clippy -D
  warnings`. No Rust or test file changed, so the D-014 coverage gate ran
  only in CI (the classifier selects it for any ci.yml edit).
- Deep review (D-068, `ievo@ievo-skills` 0.80.19): round 1 had no
  actionable findings; one out-of-scope note about
  `tests/conformance.rs:699-701` ("unlike every other test in this file"
  overclaims: two oracle-free helper unit tests also run by default) is
  recorded in `.harden/findings/issue-712.jsonl` and in a recurrence-3
  counter entry under `.harden/incidents/doc-comment-overclaims-unqualified-scope/`.
- CI: full selection ran (Tier-1 native-build-test matrix, pages, agent, governance, rustfmt, status-page-freshness); every required check green, mergeable CLEAN, no review threads. Merged by this session.

## Known follow-ups

- `tests/conformance.rs:699-701` comment overclaim (above). Not filed:
  the D-192 non-milestone ceiling (20) is exceeded, and it is not
  `vX.Y`-scoped work. Fold it into the next change that touches that file.
- The `.git/info/exclude` entry for `.harden/` in this clone hid the
  tracked findings directory from `git add -A`; `git add -f` was needed.
  Machine-local, not a repository defect.
- Maintainer-attention items from issue-select (above) remain open.

## Where to resume

Run `issue-select` again from the refreshed default branch; no task is in
flight from this session. The autopilot worktree can be removed.

Session: claude-code 6aebf4b1-d3ba-4305-8415-acecf5d0151b
