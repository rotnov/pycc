# Session checkpoint: issue #374 — v0.3 decomposition

**Date:** 2026-08-06
**Status:** Implementation complete, D-068 review clean, opening PR next.

## What happened

Under the standing v0.3 autopilot loop (task #9, D-127), `issue-select`'s pool went empty for
four consecutive iterations: every remaining open v0.3 issue was blocked (#359, #354),
deprioritized (#337, D-103), a hard exclusion (#336, #335), or a collision with open PR #358
(#142). Root cause: `docs/DELIVERY_PLAN.md` had never been decomposed for v0.3 the way v0.1/v0.2
were — no `## v0.3 execution strategy` section, no PR breakdown table, no brainstorm doc. Nothing
in the open-issue pool actually implemented the class model, `match` exhaustiveness, or any other
part of v0.3's named surface.

Per AGENTS.md's D-021 preflight step 10 ("route roadmap/milestone-decomposition work through the
same issue-to-plan/issue-implement gate as any other task"), filed
[#374](https://github.com/rotnov/pycc/issues/374) and ran it through the normal pipeline:

1. **Plan** (dispatched agent running `issue-to-plan`): published at
   https://github.com/rotnov/pycc/issues/374#issuecomment-5204010514 after 3 adversarial review
   rounds (8 corrections total). Key findings: the issue's own claim that #359 blocks `match`
   exhaustiveness didn't hold up (re-derived narrower); the "~6" PR estimate was too low (real
   count: 9, PR-15..23); the "≥45 PEPs" accept bullet had an 11-PEP gap that no named v0.3 feature
   closes; D-006 (not just D-005) was also an unresolved `proposed` stub.

2. **Implementation** (dispatched agent, pure docs — no Rust code): updated
   `docs/DELIVERY_PLAN.md` with a `## v0.3 execution strategy` section and PR-15..23 breakdown
   table; wrote `docs/superpowers/specs/2026-08-06-v0-3-classes-pattern-matching-design.md`
   (PEP→fixture→owning-PR table); added
   [D-153](../decisions/D-153-correct-v0-3-s-conformance-target-before-any-v0.md) revising the
   accept bullet from "≥45 PEPs" to "≥37 rows / 39 distinct PEP numbers" (mirroring D-088's
   precedent for v0.2), after a real per-PEP feasibility pass on the plan's 11 candidate PEPs
   found only 3 genuinely reachable (570, 591, 593) without a missing prerequisite subsystem;
   deliberately left D-005 and D-006 `status: proposed` (both need real cross-platform/dispatch
   design work better done inside their owning PRs, per `advisor` consultation) rather than force
   a premature `accepted`; filed 9 sub-issues (#375–#383) in the v0.3 milestone, one per PR row,
   dependency-ordered.

3. **D-068 review**: 1 warning (docs drift — `docs/ROADMAP.md`'s v0.2 accept bullet claims "17
   distinct PEP numbers... met", but the actual count is 14; verified this is **pre-existing**
   drift this diff's own `git diff` doesn't touch, not something introduced here — spawned as a
   separate follow-up task rather than scope-creeped into this PR) and 1 trivial note (a citation
   naming one of three call sites, doesn't affect the underlying claim). Independently closed the
   reviewer's two "not covered" gaps (no `gh`/`git` access in that review context): confirmed all
   9 filed issues state the D-005/D-006 deferral consistently with the design doc and
   DELIVERY_PLAN.md (no issue claims either ADR is resolved), and confirmed the flagged
   `ROADMAP.md` line is outside this commit's actual diff.

## Current state

- Worktree: `.claude/worktrees/issue-374-decompose-v0-3`, branch `task/issue-374-decompose-v0-3`,
  one commit (`0b3a731`) on top of `origin/main @ 155a9cb` (unchanged since preflight, re-verified
  immediately before this checkpoint).
- Issue #374 confirmed still open, no objecting comments, immediately before this checkpoint.
- Zero actionable findings remain; ready to open the pull request next.

## Resume point

If resumed fresh: push the branch, open the PR (`Fixes #374`), monitor CI, merge. Then re-enter
`issue-select` step 1 with a fresh baseline — the 9 newly-filed sub-issues (#375–#383) are now the
live v0.3 pool; PR-15 (#375, class model foundation) is the only one with no blocking dependency
and is the natural next pick.
