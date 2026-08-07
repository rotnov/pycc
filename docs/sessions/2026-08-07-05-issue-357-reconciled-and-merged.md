# 2026-08-07 checkpoint 05 — PR #357 (ultra-review skill) reconciled against ~70 commits of drift and merged

## Status

PR [#357](https://github.com/rotnov/pycc/pull/357) **merged** as `fd93fd62`. `origin/main` tip
at the time this entry was written: `fd93fd62cb1de962474eae28db4dc2ec34a0d6b0`.

This closes out the standing "356 then 357" sequencing from checkpoint
[04](2026-08-07-04-issue-356-merge-issue-402-correction.md): issue #356/PR #399 merged first
(D-155 reviewer-binding fix), then this session resumed PR #357's own review loop against the
now-current tree.

## Why reconciliation instead of rebase

PR #357 ("Add ultra-review: periodic evidence-gated review that files prioritized issues") was
opened against a tree from before two significant changes landed on `main`:

- **D-151** decomposed the monolithic `docs/DECISIONS.md` into one file per decision under
  `docs/decisions/`. PR #357's own new decision entry, and every `docs/DECISIONS.md#d-XXX-...`
  anchor inside the skill it adds, targeted the now-deleted file.
- **D-155** (checkpoint 04) replaced D-068's exact-commit-pin reviewer-binding model with
  structural verification. The skill's own review-dispatch step used the old "digest-recorded
  artifact... stop condition" language.

A mechanical `git rebase`/`merge`/`cherry-pick` of the original 12-commit branch would have hit
the same dead-`docs/DECISIONS.md` conflict on nearly every commit. Instead, per the standing plan
from checkpoint 03/04, the PR's actual *content* was hand-applied onto a fresh worktree from
current `main`, written in current conventions — dispatched to a `general-purpose` subagent
working in an isolated worktree (`.claude/worktrees/pr357-reconcile`, since removed) to keep this
session's own context bounded, per `AGENTS.md`'s D-142/D-143 guidance.

## What changed in reconciliation

Same 13 files as the original PR, five commits, `docs/DECISIONS.md` replaced by
`docs/decisions/D-147-add-the-ultra-review-skill-for-periodic-review.md` +
`docs/decisions/README.md`:

- **D-147** claimed cleanly (free slot between D-146 and D-148 on current `main` — the earlier
  collision with PR #360 was already resolved when #360's own conflicting D-147 claim was
  renumbered to D-150 on merge).
- `.claude/skills/ultra-review/SKILL.md`: 3 targeted edits — the two `docs/DECISIONS.md#...`
  anchors rewritten to `docs/decisions/D-XXX-*.md` links, and the review-dispatch step's language
  rewritten to match `issue-implement/SKILL.md`'s current D-155 template phrasing exactly.
- `docs/AGENT_TOOLING.md`, `docs/ROADMAP.md`, `scripts/validate_agent_assets.py`,
  `scripts/run_alpha_skill_evals.py`, `scripts/test_run_alpha_skill_evals.py`: hand-applied at
  content-identical anchors (some had already been touched by unrelated interim work, e.g. #399's
  own edits to `validate_agent_assets.py`, so the original hunks didn't apply verbatim).
- `docs/SPEC.md` deliberately left untouched — its decisions row no longer enumerates a D-XXX
  range post-D-151, so the original PR's 1-line hunk there has no applicable target.
- Three files (`docs/sessions/2026-08-05-04-...md`, the plan and spec under
  `docs/superpowers/`) landed verbatim as frozen historical artifacts, per D-130 — not edited to
  reflect current state even though parts now read as stale.

Full reconciliation detail, including every exact edit location, is in the (ephemeral) agent
report; this entry is the durable record.

## Review and verification

The reconciled diff (`origin/main..HEAD`, 13 files, 1745/-7) was dispatched to the D-068/D-155
pinned `ievo:deep-reviewer`. One round, one non-blocking `note`: `docs/ROADMAP.md`'s "Agent
tooling" cell still uses pre-D-155 "immutable pinned iEvo `deep-reviewer`" / "immutable iEvo
baseline" language a few sentences past the clause this PR edits — pre-existing drift from
D-155's own PR (#399), not introduced here, left as out-of-scope and flagged as potential future
`ultra-review` dogfood fodder once the skill is actually live.

Before publishing, every gate claimed in the PR's test plan was independently re-run against the
reconciled diff rather than carried over from the original PR's own (now-stale) claims:
`python3 -m unittest discover -s scripts` (548 tests), `validate_agent_assets.py`,
`generate_decisions_index.py --check`, `check-claude-marketplace.sh`,
`check_claude_reviewer_binding.py`, `ruby scripts/check_roadmap_evidence.rb` +
`test_check_roadmap_evidence.rb` + `check_ci_permissions.rb`, and
`run_alpha_skill_evals.py --client claude/codex --pycc-bin target/debug/pycc` (after building
`pycc` and `pycc_rt`, absent from a fresh worktree). One environment-only false failure was found
and resolved: `check_roadmap_evidence.rb` crashed with `invalid byte sequence in US-ASCII` under
this shell's unset `LANG`/`LC_ALL` — passed clean once `LANG=en_US.UTF-8 LC_ALL=en_US.UTF-8` was
set; not a code or content defect.

The PR body was rewritten to reflect the reconciliation (a new "Reconciled against ~70 commits of
drift" section) and to retract its own now-resolved caveat: the body's prior "D-068 pinned
reviewer is verified but not actually bound" caveat pointed at issue #356, which checkpoint 04
already closed via D-155/PR #399 — that caveat no longer applies and was marked resolved in place
rather than left stale.

## Merge

Force-pushed the reconciled 5-commit history onto `feat/ultra-review-skill-design` (verified safe
first: all 12 original commits on that branch were authored by this session's own identity,
`rotnov <denis@27tech.co>`, so nothing outside this session's own work could be lost). CI green on
every check including `ci-gate`, `build-test-coverage`, all 4 `native-build-test` matrix legs,
`frontend-perf-measure`/`frontend-perf-gate`, `cross-compile-build`/`verify`, `agent-assets`,
`agent-policy`, `audit`. D-078 state re-verified immediately before merge (`origin/main`
unchanged at `580760e`, PR head unchanged at `2a8f06c`, zero unresolved review threads via GraphQL
`reviewThreads`), diff re-confirmed identical to what the pinned reviewer saw. Merged with
`gh pr merge 357 --merge --delete-branch`; confirmed `origin/main` contains `fd93fd62` afterward.

## Where to resume

No standing task remains from the "356 then 357" sequencing — both are closed out. The one
flagged-but-out-of-scope item is `docs/ROADMAP.md`'s pre-D-155 "immutable pinned"/"immutable
baseline" language in the same "Agent tooling" cell this PR touched (see Review above) — a small,
self-contained docs fix, or a natural first target for the newly-merged `ultra-review` skill
itself to surface once it has an actual scheduled trigger. Otherwise, the next task should re-run
`issue-select`'s workflow fresh from this checkpoint's `origin/main` tip (`fd93fd62`), since the
just-merged work may have changed other issues' standing.
