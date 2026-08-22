# Incident: milestone-scope-starvation

**Date:** 2026-08-22
**Topic:** milestone-scope-starvation
**Verdict:** pending

## Symptom

Running the `issue-select` autopilot loop under a v0.3 milestone scope, seven
consecutive iterations selected and merged issues from *outside* that
milestone: #674, #679, #681, #684, #688, #721, #726 — every one of them D-185
oversized-file decomposition work. `scripts/check_conformance_breadth.py`
reported 32 of 37 conformance rows at the start of the run and 32 of 37 at the
end of it. The milestone's own critical path (#541, #703, #542, #543, #719) was
never reached in any iteration.

Nothing looked wrong from inside a single iteration. Each pick was scored under
the stated rule, cleared the adversarial advisor round, and merged with green
gates.

## Root cause

`.claude/skills/issue-select/SKILL.md` step 5 ranked the repository's own
priority markers first and treated active-milestone membership only as a
same-priority tie-break (D-144 decision (a)). That ordering is sound when the
in-scope and out-of-scope pools carry comparable markers. This tracker does not
look like that: it holds a steady supply of well-marked, small, cleanly-scoped
out-of-scope issues, so on the marker alone an outsider outranked the
milestone's own work in every iteration.

The defect is in the ordering across iterations, not in any single selection.
Because the loop deliberately re-derives its inventory from scratch each cycle,
no individual iteration carries the evidence that the run is making no progress
— which is exactly why it ran seven times unnoticed.

## Termination point

`Local-skill`: `.claude/skills/issue-select/SKILL.md`, step 5 ("Score the
survivors").

## Artefact

**Type:** rule (local-skill edit)
**File:** `.claude/skills/issue-select/SKILL.md`
**Change:** Step 5's ordering paragraph is replaced. When a milestone scope is
in effect, membership in that scope ranks first — ahead of the priority marker
and ahead of size; inside each group the order remains marker-then-size, so
out-of-scope issues sort below every in-scope survivor and are reached only when
the scope contributes no survivor at all. The pool is never restricted, so every
issue stays selectable. Leaving the scope becomes a reportable event: the
justification must state that the scope contributed no survivor and name what
disqualified each of its members, and step 8's hand-off report carries that
record. With no milestone scope in effect the ordering is unchanged.

Recorded as `docs/decisions/D-191-milestone-membership-ranks-first-in-issue-select.md`,
superseding D-144 decision (a) only. Machine bindings moved with the rule:
`issue_select_higher_ranked` in `scripts/run_alpha_skill_evals.py` gains an inert
`milestone_scope_in_effect` parameter, `ISSUE_SELECT_CONTRACT` pins two literals
from the new sentence, and a fifth eval case covers the starvation scenario.

## Fixture

`.harden/incidents/milestone-scope-starvation/fixture/`
- `task.md`: a standing directive naming the v0.3 milestone plus an instruction
  to pick exactly one issue from `issues.md` and write it to `selection.md`
- `issues.md`: a static ten-row open-issue list — three tiny out-of-scope P1s,
  three moderately sized in-scope issues (unmarked, P2, P3), one blocked issue,
  one closed issue
- `control.md`: the project's selection rules with the pre-D-191 step 5 ordering
- `patch.md`: identical, with the D-191 ordering substituted for it
- `verify.py`: checks the selected issue number is one of the milestone's own
  reachable members, and that `issues.md` was not rewritten to re-tag the
  outsider into the milestone or to empty the milestone

Self-contained: no `gh`, no network, no `setup.py`.

## Arena verdict

Pending — see the campaign result recorded below when it completes.

## Verify

`verify: arena` — this is a behavioural decision, not a command-syntax rule, so
the arena can exercise it: the control and patch conditions differ only by the
ordering paragraph, and the correct answer under each is mechanically derivable
from `issues.md`. Reviewed against
`.claude/skills/harden/references/fixture-review.md` before the campaign
(verdict FIX-FIRST on two defects — an over-strict `SELECTED:` regex that would
have failed correctly-selecting runs on formatting alone, and this missing
incident entry; both fixed, and `verify.py` was hand-run in five directions:
empty workdir FAILS, missing `selection.md` FAILS, out-of-scope pick FAILS,
screened-out pick FAILS, edited `issues.md` FAILS, correct in-scope pick PASSES).
