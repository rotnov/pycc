# 2026-08-05 checkpoint: `issue-implement`'s delegated `issue-to-plan` invocation now also dispatches to a sub-agent (D-143)

## Status

Direct user follow-up request (not a GitHub issue), asked immediately after
D-142/PR #339 merged (see
`docs/sessions/2026-08-05-01-issue-implement-dispatches-implementation-to-a-subagent-d-127-compact-claim-corrected.md`):
"what else can be moved into agent dispatch to reduce orchestrator context
growth?" The answer's first item — dispatching `issue-to-plan`'s entire
workflow, not just its own review rounds — was approved and implemented as
**D-143**, applying D-142's same context-isolation reasoning to
`issue-implement`'s step 3 (obtaining or refreshing a plan) instead of only
its step 4/5 (implementation).

## What was actually done this session

1. Scoped D-143 deliberately narrower than "dispatch every `issue-to-plan`
   call": only the *delegated* invocation, i.e. when `issue-implement`
   invokes `issue-to-plan` under its own standing authorization. The
   standalone, user-facing `issue-to-plan` path (a human directly asking to
   plan an issue) is explicitly excluded — its per-payload publish approval
   needs live interactive turn-taking a dispatched agent cannot provide,
   since a dispatched agent only returns a final report and cannot pause
   mid-task for approval.
2. Verified empirically, before relying on it, that nested/recursive `Agent`
   dispatch actually works in this environment: dispatched a
   `general-purpose` agent instructed to dispatch its own nested
   `general-purpose` sub-agent, and confirmed it returned successfully. No
   documented recursion-depth limit was found anywhere in the repository's
   own tooling or configuration — recorded as "not found," not assumed safe.
   This mattered because D-143's design has the `issue-implement`-dispatched
   agent itself further dispatch `issue-to-plan`'s own step 6 adversarial
   review loop — two levels of nested dispatch, the first live exercise of
   that depth in this project's autopilot.
3. Recorded **D-143** in `docs/DECISIONS.md`: `issue-implement`'s step 3,
   when no plan exists or an existing one needs refreshing, now invokes
   `/issue-to-plan` inside a freshly-dispatched `Agent` rather than directly
   in the orchestrating session's own context. The dispatched agent invokes
   the `issue-to-plan` skill itself via the `Skill` tool and runs it to
   completion inside the same task branch/worktree this session already
   created in step 1 — read/build access for its own empirical verification,
   no commits, `issue-to-plan`'s own Non-negotiable #4 unchanged.
   `issue-implement`'s declared write authorization substitutes for
   `issue-to-plan`'s own per-payload publish approval exactly as it already
   did before delegation moved inside a dispatched agent.
4. Updated `.claude/skills/issue-implement/SKILL.md` step 3,
   `.claude/skills/issue-to-plan/SKILL.md`'s Publish-step paragraph,
   `docs/AGENT_TOOLING.md`'s `issue-implement` summary and its publish-gate
   paragraph, and `docs/SPEC.md`'s `DECISIONS.md` row/range to reflect
   D-143. Confirmed the Codex mirrors under `.agents/skills/` need no
   parallel edit — unchanged thin pointers to the canonical Claude files.
5. Five rounds of the pinned `ievo:deep-reviewer` loop (D-068), each finding
   real and fixed, none dismissed:
   - round 1: the dispatch instruction was grammatically anchored only to
     the "no plan exists" trigger while the documentation claimed it also
     covered "plan needs refreshing" — fixed by unifying both triggers into
     one sentence; plus an optional note that `issue-to-plan`'s own
     Non-negotiable #3/step 7 didn't explicitly address a dispatched-`Agent`
     caller acting under delegated authorization — fixed too.
   - round 2: a genuine self-contradiction — the new D-143 Consequences text
     claimed `issue-to-plan`'s own `SKILL.md` "needs no change" while this
     same diff edits that exact file; fixed by narrowing the claim to "no
     *behavioral* change" and naming the one clarifying sentence added. Two
     note-level findings (an `AGENTS.md` citation not yet extended to D-143;
     a minor step-3/step-4 phrasing asymmetry) fixed alongside.
   - round 3: step 3's new dispatch paragraph lacked the
     fails-to-start/hangs/no-usable-report retry-once discipline step 4
     already has for its own dispatch — fixed by adding the matching
     sentence and a new per-issue Stop-conditions bullet.
   - round 4: the round-3 fix's own claim in D-143's Consequences ("no new
     stop condition... for the same reason D-142 needed none") was now
     false, since the round-3 fix *did* add one — fixed by rewriting that
     sentence to state the new bullet and explain why D-143's case differs
     from D-142's. `docs/AGENT_TOOLING.md`'s publish-gate paragraph still
     described the delegated-invocation exception as strictly closed to
     `issue-implement` itself, with no acknowledgment that this same diff
     widens it to a dispatched agent acting under that authorization — fixed
     by appending a pointer clause.
   - round 5: clean — explicitly re-verified both round-4 fixes plus a full
     re-pass over all five changed files and several unchanged files that
     reference them (`issue-select/SKILL.md`,
     `autopilot-async-monitoring/SKILL.md`, the `.agents/skills/` mirrors,
     both validator scripts, both skills' eval JSON files, the dated
     `docs/superpowers/` design/plan documents). No further findings.
6. Ran the applicable local gates (no Rust code touched by this diff): `ruby
   scripts/check_roadmap_evidence.rb` (`RUBYOPT="-E UTF-8"`, this
   environment's own pre-existing Ruby-locale quirk, not a repo defect),
   `python3 -B -m unittest discover -s scripts -p 'test_*.py'` (487 tests),
   `python3 -B scripts/validate_agent_policies.py`, `python3 -B
   scripts/validate_agent_assets.py` — all green after every fix round;
   confirmed no new eval oracle is needed since this is workflow guidance
   for existing steps, not a new decision the skill itself makes.
7. Fast-forwarded this branch onto `origin/main`'s tip (`a781a2b`, PR #341
   merged — value-less-annotation type-check work, no overlap with any file
   this change touches) before committing, per D-021.

## What is NOT done

- No PR opened yet for this change as of this checkpoint — branch
  `issue-to-plan-full-dispatch` is committed locally, not yet pushed.
- D-143's own mechanism (dispatching `issue-to-plan`'s full workflow,
  including its nested review-loop dispatch, from inside `issue-implement`'s
  step 3) has not yet been exercised on a real autopilot run. The next
  `issue-select` -> `issue-implement` iteration that needs a fresh or
  refreshed plan is the first live test of whether the two-level nested
  dispatch behaves as smoothly in practice as this diff's own text assumes —
  report size, nested-dispatch latency, and whether the outer dispatched
  agent's own report stays as compact as intended are all real open
  questions, same caveat D-142's own checkpoint recorded for its first live
  run.
- No broader audit was done of whether other delegated-invocation-style
  exceptions exist elsewhere in the repository's skills that might need the
  same "a dispatched agent acting under X's authorization also qualifies"
  clarification D-143 added to `issue-to-plan`. This diff's own reviewer
  checked only the files this change touches plus their direct
  cross-references, not a repository-wide sweep for the pattern.

## Where a fresh session should resume

1. This change's own worktree: `/private/tmp/pycc-issue-to-plan-dispatch`
   (branch `issue-to-plan-full-dispatch`, fast-forwarded onto `origin/main`
   at `a781a2b`). Re-run the D-021 preflight fast-forward check against
   `origin/main` first — do not assume it is still unchanged.
2. Commit the staged changes (already gate-clean, already reviewed clean
   through 5 rounds), push, and open the pull request. No `Fixes #N` — this
   is a direct user request, not a tracked issue, matching PR #339's own
   convention for D-142.
3. Monitor CI to green (D-078), do a final pre-merge diff read, and merge.
4. Once merged, the *next* `issue-implement` run that needs to obtain or
   refresh a plan should actually exercise D-143's own new step 3 dispatch
   pattern for the first time — treat any friction found there (the
   nested-dispatch review loop behaving unexpectedly, the dispatched agent's
   report growing too large, the retry-once handling for a failed initial
   dispatch actually firing) as real evidence for a future correction to
   D-143, not a reason to quietly fall back to in-session planning.
