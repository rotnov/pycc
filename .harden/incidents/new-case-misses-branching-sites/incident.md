# Incident: new-case-misses-branching-sites

**Date:** 2026-08-23
**Topic:** new-case-misses-branching-sites
**Verdict:** no baseline (twice) — artefact landed, effect unmeasured; do not re-run
**Batch:** `.harden/findings/issue-734.jsonl`, 7 findings, 5 review rounds

## Symptom

Three of the batch's seven findings, all in the same consumer skill, all one shape:

- the skill had no awareness of the new pull-request shape at all;
- the repair landed the new shape in steps 5 through 8 while the triage step stayed
  unconditional, so triage would still have closed the container the shape exists to keep open;
- the repair for *that* drew a bright line which contradicted the triage step's own outcome.

Each round extended the coverage by one site and stopped. The class survived two of its own
repair rounds, which is what distinguishes it from an ordinary omission.

## Root cause

A change introduces a new exceptional case into a rule that an existing document already
branches on in several places. The author enumerates the sites the new case is *about* — the
ones the request names — rather than every site that already dispatches on the general rule.
Coverage tracks salience, not decision-relevance. Because a repair is itself scoped to the
site that was just found missing, the same omission reproduces for every site still missing.

## Termination point

`Local-skill`: `.claude/skills/issue-to-plan/SKILL.md`, step 3 ("Establish the repository's
own constraints"). Gap classification: **absence** — nothing in the planning gate required
listing the sites a modified rule is dispatched from. Measured: the issue that produced this
batch carries zero plan comments and no plan file, and grepping the planning skill for
site-enumeration language returned only reading instructions.

A static rung was evaluated and not taken. A section-level co-occurrence check (every section
naming two of the three exempt-shape tokens but not the third) was run read-only over three
revisions: it fires on exactly the two sites the second round flagged, goes silent at that
round's fix, and emits one benign false positive at the branch head. Honest coverage is one
of three members, and the silence is a shape limit rather than a tuning one — a section that
branches on the general rule while naming no precedent token is permanently invisible to it.

## Artefact

`rule`, landed in the planning gate's step 3: when a change adds an exceptional case to a rule
an existing document already branches on, the plan enumerates every site that dispatches on the
general rule and records it as an affected-site inventory, one line per site stating whether
the new case needs a branch there or provably does not.

## Sweep result

Two hits across the planning and implementation skills, both benign: a site that names the
general rule without dispatching on it, and one whose omission of the new case is stated and
correct. No third site required a branch.

One further site was found by turning the rule on its own file: the planning gate's own
drafting step enumerates what a plan contains, and that enumeration did not name the
affected-site inventory the new paragraph requires — the defect reproduced inside the commit
introducing the rule. Fixed in the same commit by adding it to that enumeration. The
report-contents enumeration in the same file was checked and provably needs no branch.

## Arena

Two campaigns, both **no baseline**, and the second is the informative one.

| campaign | fixture | codex control | devin control | patch |
|---|---|---|---|---|
| 1 | 60-line handbook, 3 required sites | 3/3 pass (judge 5/10) | 3/3 pass (judge 6/10) | 6/6 pass |
| 2 | 127-line handbook, 8 dispatch sites, 6 required | 3/3 pass (judge 7/10) | 3/3 pass (judge 10/10) | 6/6 pass |

Campaign 1's `no baseline` was routed the way this skill directs — fix the fixture, not the
artefact — and the fixture was more than doubled. The control arm then got **better**, not
worse: codex 5/10 to 7/10, devin 6/10 to 10/10. That inversion is the finding. A larger
document with more visibly class-branching sections makes exhaustive enumeration the obvious
reading of a prompt whose only job is "say what to change and where", so scaling the fixture
scales the telegraphing along with the difficulty. A third campaign in that direction would
buy the same result again.

Corroborating: one control run passed while never reading the handbook at all
(`codex/control#1` — recorded under the `subagent-fabricated-evidence` topic), so the fixture
credits plan shape rather than site discovery.

The structural reason the class is not reproducible at fixture scale: it occurred inside a
560-line skill, in a session already carrying a review loop, a decision record, and a fix
scoped to one just-found site — and it survived two repair rounds, which a single-task fixture
with a fresh context cannot instantiate at all. `no baseline` here means the experiment was
not the incident, not that the rule is inert.

Shipped on that basis: the artefact is one paragraph in a planning gate that already carries
comparable enumeration duties, its cost is bounded, and it discriminated on the one case
available to test it — its own introducing commit, where it found a real missing branch (see
the sweep above). Effect on the class remains unmeasured, and this entry is the record of
that rather than a claim of profit.

**fixture:** `.harden/incidents/new-case-misses-branching-sites/fixture`
**artifact:** `.claude/skills/issue-to-plan/SKILL.md` step 3, affected-site inventory paragraph
**verify:** `arena` — no baseline, twice (see the Arena section; do not re-run); the fixture was reviewed against
`.claude/skills/harden/references/fixture-review.md` (verdict FIX-FIRST on four defects: two
site patterns creditable by incidental vocabulary, a missing incident entry, an over-narrow
site-1 pattern, and an overstated pass message — all four fixed) and `verify.py` was hand-run
in six directions: empty workdir FAILS, missing plan FAILS, salient-sites-only plan FAILS,
the reviewer's incidental-vocabulary plan FAILS, a correct paraphrased inventory PASSES, and
editing the source document instead of planning FAILS.
