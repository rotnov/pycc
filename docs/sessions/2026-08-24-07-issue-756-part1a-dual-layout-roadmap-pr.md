# Session handoff: issue-756-part1-dual-layout-roadmap (PR #761)

## Status

PR #761 delivers "Part 1a of #756": it teaches two of the three consumers
named in the fully-reviewed plan on issue #756 (comment
https://github.com/rotnov/pycc/issues/756#issuecomment-5398764626) to accept
roadmap content from either the single canonical `docs/ROADMAP.md` file or a
future `docs/roadmap/**/*.md` directory tree, fail-closed in both directions,
preferring the single file when both exist. `docs/ROADMAP.md` itself is
untouched and stays canonical — this is additive, dead-code-until-a-later-part
capability; nothing observably changes yet.

Consumers covered by this PR:

- `scripts/check_roadmap_evidence.rb` — new `resolve_evidence_ids(root)`.
  Each `docs/roadmap/*.md` file is parsed independently with its own fresh
  `heading_path` stack (the existing per-document parser tracks heading state
  across the whole text, so naive concatenation would let one file's trailing
  heading leak into the next file's evidence attribution), then evidence IDs
  are merged with a duplicate-claim check.
- `scripts/check_conformance_breadth.py` — new `resolve_roadmap_text(path)`.
  This checker's regexes (`ROADMAP_HEADLINE`, `ROADMAP_FIGURES`,
  `ROADMAP_PEP_FIGURES`, `ACCEPT_CLAUSE_FIGURES`) are whole-text and
  stateless, so plain concatenation of `docs/roadmap/**/*.md` files (sorted
  by path) is safe here, unlike the Ruby side.

Both sides ship full positive/negative test coverage: missing-both-layouts,
an empty `docs/roadmap/` directory, a duplicate evidence-id claim across two
files, and the file-wins-over-directory precedence case.

Issue #756 stays open; this PR does not carry a closing keyword.

## Deviation from the published plan: workflow-policy.yml deferred

The plan's third work item — `.github/workflows/workflow-policy.yml`'s JS
prefix rule replacing its fixed-path `docs/ROADMAP.md` membership check — is
**not** in this PR, and this is a deliberate, reasoned deviation from a
literal single-PR reading of the plan.

While implementing that item directly (editing the `protectedPaths` download
loop in `workflow-policy.yml` to add a `docs/roadmap/` prefix walk over the
downloaded HEAD tree), I discovered that `workflow-policy.yml` itself is
pinned by an exact SHA256 digest in `scripts/check_ci_permissions.rb`'s
`TRUST_ANCHOR_SHA256_ALLOWLIST` (`validate_policy_set`, checked
unconditionally at the start of `main`). The required `pull_request_target`
"Workflow policy" (`audit`) job checks out `scripts/check_ci_permissions.rb`
from the **pre-merge base** revision (`ref: github.sha`) and validates the
**head**'s `workflow-policy.yml` bytes against that base's allowlist —
exactly the D-034 trust boundary this repository relies on. Changing the
file's content in the same PR that also changes it would fail against the
not-yet-updated base allowlist.

This repository has documented precedent for exactly this class of problem:
`scripts/check_roadmap_evidence.rb`'s `REVIEWED_PERF_CI_WORKFLOW_SHA256S`
carries a long history of staged, coexisting digest entries for `ci.yml`
(the `D80_*`/`D84_*`/`D91_*`/`D99_*`/`D100_*`/`D112_*`/`D114_*` constants,
each commented "coexists with X until a later round retires them ... not yet
active"), and `git log -p --follow -- scripts/check_ci_permissions.rb`
confirms `TRUST_ANCHOR_SHA256_ALLOWLIST` itself has been staged this way
before, across several historical digest transitions
(`5f5be6e9...` → `3a8b5677...` → `4dc12b9c...` → the current
`f8d60936...`).

Resolved via `advisor` consultation per D-127 (autonomous agent operation
model) rather than pausing for the repository owner. The adopted split:

- **This PR (Part 1a)**: Ruby + Python resolvers and their tests only;
  `workflow-policy.yml` is byte-identical to `main` (confirmed via
  `git diff --name-only` before commit — it does not appear in the diff).
- **A follow-up PR (Part 1b of #756, not yet started)**: stages the new
  `workflow-policy.yml` digest into `TRUST_ANCHOR_SHA256_ALLOWLIST`
  (coexisting with the current one), updates
  `tests/fixtures/workflow-policy-search-ledger.yml` and the constants in
  `scripts/test_check_ci_permissions.rb`
  (`ACTIVE_TRUST_ANCHOR_SHA256`/`RETIRED_TRUST_ANCHOR_SHA256`/
  `PROSPECTIVE_SEARCH_LEDGER_TRUST_ANCHOR_SHA256`) accordingly, then in a
  later merge (or the same one, depending on whether the staged digest is
  visible to the base checker by the time that PR's own `audit` job runs —
  this still needs to be worked out at the start of that PR) flips the
  actual JS content to the prefix rule and extends
  `scripts/test_check_ci_permissions.rb` with the prefix-rule assertions
  the plan's work item 3 calls for (present/absent-prefix coverage,
  no regression on the other fixed-path entries).

The plan's own Gates section hints at awareness of this trust boundary
("verify ... the D-024/D-034 pinned-checker discipline (base-revision
execution) is unchanged") but does not explicitly call out the
SHA256-allowlist staging mechanic, so this deviation is recorded here rather
than assumed covered by the plan text.

## What happened in this PR's lifecycle

1. Implemented all four work items from the plan's first two work items
   (Ruby resolver + tests, Python resolver + tests) plus the third item's
   JS change, in the same working tree.
2. Discovered the trust-anchor deadlock described above while about to
   extend `scripts/test_check_ci_permissions.rb` per the plan's third work
   item. Reverted `.github/workflows/workflow-policy.yml` to `main` via
   `git checkout --`, scoping this PR to the Ruby/Python resolvers only.
3. Ran the plan's Gates section commands (all against the reverted,
   Ruby/Python-only diff):
   - `ruby scripts/test_check_roadmap_evidence.rb` — 225 runs, 1196
     assertions, 0 failures, 0 errors (run with `LANG=en_US.UTF-8
     LC_ALL=en_US.UTF-8`; the sandbox's default `US-ASCII` locale produces 6
     pre-existing failures + 1 pre-existing error unrelated to this change,
     confirmed via `git stash`/`git stash pop` comparison against the
     unmodified baseline).
   - `python3 -I -B scripts/test_check_conformance_breadth.py` — 74 tests,
     OK (73 before a reviewer-driven addition, see step 5).
   - `ruby scripts/check_roadmap_evidence.rb .` (real, unmigrated
     `docs/ROADMAP.md`) — passes unchanged.
   - `python3 scripts/check_conformance_breadth.py` (same) — passes
     unchanged.
   - `ruby scripts/test_check_ci_permissions.rb` — 39 runs, 0 failures
     (unaffected by this PR's scope, run as a sanity check since the plan
     names it).
   - `ruby scripts/check_ci_permissions.rb` — passes unchanged.
4. Confirmed via `scripts/classify_ci_changes.py`'s source that a
   `pull_request`-event diff touching only `scripts/*.rb`/`scripts/*.py`
   paths not in `COMPILER_GATE_SCRIPTS` resolves to `compiler=False`
   (`EMPTY_SELECTION`), so the required 100%-coverage `llvm-cov` job is
   legitimately not selected for this change — not a skipped required gate,
   a change set the fail-closed classifier correctly excludes from the Rust
   coverage job. Confirmed this held in the actual PR run (see step 6).
5. Ran the pinned local reviewer (`ievo:deep-reviewer`, dispatched via the
   `Agent` tool per AGENTS.md's "Local pinned review loop") against the
   staged diff. One `[warning]` finding: the Python side had no test
   mirroring the Ruby suite's
   `test_prefers_docs_roadmap_md_over_a_docs_roadmap_directory_when_both_exist`
   — `resolve_roadmap_text`'s early-return branch was never exercised in a
   scenario where both `docs/ROADMAP.md` and a sibling `docs/roadmap/`
   directory exist simultaneously. Fixed by adding
   `test_prefers_roadmap_md_over_a_sibling_roadmap_directory` to
   `ResolveRoadmapTextTests` in `scripts/test_check_conformance_breadth.py`,
   using a deliberately-invalid sibling directory file (so a wrong fallback
   would fail loudly via `check_roadmap_counts`, not coincidentally pass).
   Re-ran `python3 -I -B scripts/test_check_conformance_breadth.py` (74
   tests, OK) to confirm.
6. Committed, pushed `issue-756-part1-dual-layout-roadmap`, opened PR #761.
   The first `gh pr create` body used the phrasing "This PR does **not**
   close #756" — GitHub's closing-keyword scanner matched the adjacent
   `close #756` substring despite the negation (exactly the trap AGENTS.md's
   "Pull request creation" section warns about), which the GraphQL
   `closingIssuesReferences` check caught (`totalCount: 1`, node `756`)
   moments after opening. Edited the PR body to remove the keyword-adjacent
   phrasing ("Part 1a of #756; #756 stays open ...") and re-verified via
   GraphQL after a short propagation delay — `totalCount: 0`.
7. Watched CI via a background `Monitor` loop polling `gh pr checks 761`.
   All checks passed: `ci-gate`, `governance`, `classify-changes`, `audit`,
   `status-page-freshness`; `cross-compile-build`, `build-test-coverage`,
   `native-build-test`, `frontend-perf-measure`, `pages-performance`,
   `pages-accessibility`, `cross-compile-verify`, `frontend-perf-gate` all
   `SKIPPED` (as expected from the classifier reasoning in step 4).
8. Re-checked for concurrent/duplicate work immediately before merge:
   `gh issue view 756 --json state` still `OPEN`;
   `gh pr list --search "756"` shows only PR #761 itself. No collision with
   the documented concurrent background actor.

## Docs impact

No documentation outside this session log needed updating: `docs/ROADMAP.md`
is unchanged, and the plan explicitly scopes this part as producing no
observable behavior change (dead code until the migration part). No public
API, CLI, or diagnostics changed.

## Where to resume

At the time this entry was written, PR #761's `mergeStateStatus` was `CLEAN`
with every required check green; the plan is to merge with `gh pr merge 761
--repo rotnov/pycc --merge --delete-branch` immediately after this entry is
committed, then confirm both local and remote branch deletion.

Part 1b of #756 (the deferred `.github/workflows/workflow-policy.yml` prefix
rule plus its trust-anchor digest staging) has not been started. A future
session picking this up should re-read this entry's "Deviation from the
published plan" section before starting, re-derive the current
`TRUST_ANCHOR_SHA256_ALLOWLIST` state fresh (it may have changed since this
session), and decide at that point whether the staging and the content flip
can land in one PR or genuinely need two, based on the exact base-revision
timing of the `pull_request_target` "Workflow policy" job.
