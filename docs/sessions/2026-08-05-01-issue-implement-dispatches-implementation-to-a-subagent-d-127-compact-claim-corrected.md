# 2026-08-05 checkpoint: `issue-implement` now dispatches implementation to a sub-agent (D-142); D-127's unexecutable "compact the session" claim corrected

## Status

Direct user request (not a GitHub issue): after finishing issue #181
(`float(x)` as a builtin, PR #316, see
`docs/sessions/2026-08-03-09-issue-181-implemented-float-x-as-a-real-builtin-conversion.md`),
the user asked how to keep the `issue-select` autopilot loop's own context
from growing unboundedly the way it did during that run. Investigated
directly rather than assuming, recorded the finding as D-142, and — in the
course of verifying a citation — discovered that an existing accepted
decision (D-127) and its `AGENTS.md` mirror both instruct something that
turns out to be impossible to execute. Corrected both.

## What was actually done this session

1. Investigated whether `/compact`/`/clear` can be invoked programmatically
   (by a hook, a setting, or the agent itself) to bound context growth at
   task-boundary checkpoints. Fetched Claude Code's own hooks documentation
   directly (not inferred): hooks are "purely reactive... they cannot
   initiate compaction themselves," `PreCompact` can only block compaction,
   never trigger it, and there is no `settings.json` field, CLI flag, or SDK
   call for a non-interactive `/compact`/`/clear`. Checked this
   environment's own `CronCreate`/`ScheduleWakeup` tool schemas directly:
   both are confirmed session-scoped ("jobs live only in this session...
   gone when the session exits") — neither spawns a fresh session, both
   just re-enqueue a prompt into the same one.
2. While citing `.claude/skills/autopilot-async-monitoring/SKILL.md` and
   D-127 as supporting context for the new decision, verified the exact
   wording rather than paraphrasing from memory — and found D-127's own
   Decision paragraph, and `AGENTS.md`'s mirroring bullet (line 15),
   instruct agents to "compact the session... proactively" (`AGENTS.md`
   spelling it out further as "e.g. `/compact` or the client's
   equivalent") at task checkpoints. Given step 1's findings, this
   instruction cannot actually be followed by a running session.
3. Recorded **D-142** in `docs/DECISIONS.md`: `issue-implement`'s step 4
   (implementation) and step 5's fix rounds now dispatch to a
   freshly-spawned `Agent` working inside the same task branch/worktree the
   orchestrating session already created in step 1's D-021 preflight,
   instead of executing directly in the orchestrator's own context. Fix
   rounds resume the same dispatched agent via `SendMessage` to its own
   agent id rather than fixing findings in-session or re-dispatching a
   stateless fresh agent each round. Motivating evidence cited directly:
   issue #181's own real run, where implementation plus a coverage-gap
   root-cause investigation plus a full D-068 review fix cycle consumed the
   large majority of that session's context.
4. Appended a dated **Correction** to D-127 (append-only convention, per
   this file's own established D-116/D-119/D-086 precedent — never
   rewriting an accepted decision's own body) documenting that the
   "compact the session" clause assumed an unavailable capability, citing
   the direct research from step 1, and pointing at D-142 as the mechanism
   actually available. Rewrote `AGENTS.md` line 15 in place (no
   append-only constraint there — it is a living instructions file) to the
   same effect.
5. Updated `.claude/skills/issue-implement/SKILL.md` (steps 4 and 5),
   `docs/AGENT_TOOLING.md`'s `issue-implement` summary paragraph, and
   `docs/SPEC.md`'s `DECISIONS.md` row/range to reflect D-142. Confirmed the
   Codex mirror (`.agents/skills/issue-implement/SKILL.md`) needs no
   parallel edit — it is an unchanged thin pointer to the canonical Claude
   file, verified directly rather than assumed.
6. Three rounds of the pinned `ievo:deep-reviewer` loop (D-068), all real
   findings fixed, not dismissed: round 1 caught a stale `docs/SPEC.md`
   decision range, an inaccurate quoted citation, and a missing
   failed-dispatch stop-condition note; round 2 caught the Correction
   paragraph itself misquoting D-127 (attributing `AGENTS.md`'s fuller
   `/compact`-naming wording to D-127's own, plainer text) plus a
   confusing self-reference; round 3 was clean.
7. Ran the applicable local gates (no Rust code touched): `ruby
   scripts/check_roadmap_evidence.rb` (`RUBYOPT="-E UTF-8"` — this
   environment's Ruby defaults to `US-ASCII` and chokes on this file's own
   em-dashes without it, a pre-existing environment quirk, not a repo
   defect), `python3 -B -m unittest discover -s scripts -p 'test_*.py'`
   (487 tests), `python3 -B scripts/validate_agent_policies.py`, `python3
   -B scripts/validate_agent_assets.py` (covers
   `validate_alpha_skill_contracts`) — all green, confirmed no new eval
   oracle is needed since this is workflow guidance, not a new decision
   the skill makes.

## What is NOT done

- No PR opened yet for this change as of this checkpoint — branch
  `issue-implement-subagent-dispatch` is committed locally, not yet
  pushed.
- D-142's own mechanism (dispatch step 4/5 to an `Agent`, resume it via
  `SendMessage` for fix rounds) has not yet been exercised on a real
  `issue-implement` run. The next autopilot iteration is the first live
  test of whether the dispatched-agent report format and the
  `SendMessage`-based fix-round resumption actually work as smoothly in
  practice as the ADR's own text assumes.
- No further audit was done of other places in the repository that might
  assume self-triggered compaction is possible — `grep` for "compact the
  session"/`/compact` during round 2's review found no other live
  reference, only historical `docs/sessions/*` journal entries (exempt
  under the D-066 informational carve-out), but this was the reviewer's
  own scoped check for this diff, not an exhaustive repository-wide sweep
  commissioned separately.

## Where a fresh session should resume

1. This change's own worktree: `/private/tmp/pycc-issue-implement-subagent-dispatch`
   (branch `issue-implement-subagent-dispatch`, based on `origin/main` at
   `d6d0d8e`). Re-run the D-021 preflight fast-forward check against
   `origin/main` first — do not assume it is still unchanged.
2. Commit the staged changes (already gate-clean, already reviewed clean
   through 3 rounds), push, and open the pull request. No `Fixes #N` — this
   is a direct user request, not a tracked issue.
3. Once merged, the *next* `issue-select` -> `issue-implement` autopilot
   iteration should actually exercise D-142's own new step 4/5 dispatch
   pattern for the first time, not just describe it — treat any friction
   found there (dispatched-agent report too large, `SendMessage` resumption
   behaving unexpectedly, the retry-once handling for a failed initial
   dispatch actually firing) as real evidence for a future correction to
   D-142, not a reason to quietly fall back to in-session implementation.
