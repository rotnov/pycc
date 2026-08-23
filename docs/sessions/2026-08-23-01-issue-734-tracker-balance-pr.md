# 2026-08-23-01 — issue #734, tracker-growth balance, pull request opened

## Baseline

- Default branch: `origin/main` at `4df39320` ("docs: ratify the PEP 701 conformance row at
  `◐` (Part 2 of #719) (#731)"), re-fetched immediately before committing this entry.
- Open pull requests at that moment: none.
- Task branch: `issue-734-tracker-balance`, 9 commits ahead of `origin/main`, being pushed as
  this entry lands.
- Issue #734 ("P1: The tracker grows +4/day on self-generated process work while v0.3's
  Accept criterion stalls") — OPEN, closed by this pull request.

## What this delivered

The diagnosis behind #734 is that nothing in the loop bounded tracker growth: every session
could file without limit, and non-milestone process work competed with milestone work on
equal footing. D-192 records five rules against that, two of which are mechanical:

- a per-issue creation ceiling of 20 in `issue-select` step 2, with a bootstrap exemption for
  umbrella issues (a ceiling that forbids creating the containers meant to absorb the
  overflow would deadlock on its first application);
- a 4:1 non-milestone merge quota in step 5, whose window is the candidate plus the four
  merges preceding it — the candidate occupies the fifth slot, so counting five *preceding*
  merges would silently enforce 1-in-6.

`issue-implement` gained the third `Fixes #N`-exempt pull-request shape (umbrella-checklist,
alongside D-080 stage and D-185 narrowing). An umbrella issue is a standing container with no
completion state, so both "close it" and "narrow it to what remains" are meaningless for the
container itself.

The D-068 review loop ran six rounds. Rounds 1 through 5 each surfaced a distinct defect;
round 6 was clean. Rounds 4 and 5 are the ones worth reading: round 4's fix cited, as evidence
that the umbrella is never narrowed, a branch that says its comment *does* narrow it; round 5
found that fix itself too narrow, because it tied narrowing to the trigger rather than the
outcome. Three consecutive rounds where a repair inherited less caution than the original
design.

## Harden pass

Run over the seven review findings as one batch (`.harden/findings/issue-734.jsonl`). One
batched tracer produced three root-cause classes rather than seven traces.

- **Class A** (3 findings) — a new exceptional case reaches the sites it is *about* rather
  than every site that already dispatches on the rule. Artefact landed in the planning gate:
  `issue-to-plan` step 3 now requires an affected-site inventory, and step 6's plan-contents
  enumeration names it. That second edit exists because turning the rule on its own file found
  the same omission inside the commit introducing it.
- **Class B** (3 findings) — a summary tier disagreeing with its own body. A recurrence in a
  family consolidated into one class on 2026-08-21; `build nothing`, on the evidence rather
  than by deference to the prior adjudications. No running tally of that family is restated
  here — two attempts at one produced two different wrong numbers, which is the defect the
  class is about.
- **Class C** (1 finding) — a policy forbidding its own bootstrap. Escalated alone on
  severity; singleton counter seeded, deliberately not folded into the other two.

Arena: **no baseline across two campaigns**, and the second is the informative one. Campaign
1's no-baseline was routed as this skill directs — fix the fixture, not the artefact — and the
handbook was more than doubled (60 → 127 lines, 3 → 6 required sites). The control arm then
got *better*: codex 5/10 → 7/10, devin 6/10 → 10/10. Scaling the fixture scales the
telegraphing along with the difficulty, so a third campaign would buy the same answer again.
One control run also passed while never reading the document under test, recorded under the
existing `subagent-fabricated-evidence` topic. The class occurred inside a 560-line skill in a
session already carrying a review loop and a decision record, and it survived two repair
rounds — none of which a single-task fixture with a fresh context can instantiate. The rule
ships with its effect recorded as unmeasured rather than claimed as profit.

Two retrospective entries were added for the process mistakes behind the classes.

## Follow-ups this session did not do

- **The three umbrella issues D-192 assumes** (CI governance, website/SEO, agent tooling) do
  not exist yet. D-192 and `AGENTS.md` both say the first filer bootstraps them; nobody has.
  Until they do, the bootstrap exemption is the only thing keeping the ceiling from blocking
  its own remedy. This is a non-code deliverable of the change and is explicitly outstanding.
- **Stale vendored copies, out of tree:** the harden skill bundles its own copies of the
  sessions practice (`skills/sessions/SKILL.md` and `assets/features/sessions/`) that still
  describe the handoff log as "one snapshot per checkpoint", which D-130 superseded. Those
  files live under `.claude/skills/harden/`, which this clone excludes as machine-local, so
  they are not repository content and no change here can fix them — the correction belongs
  upstream in the harden skill. The tracked copy, `.claude/skills/sessions/SKILL.md`, is
  updated on this branch.
- **`AGENTS.md`'s standing bypass authorization** still names "the recurring D-103 manifest
  deadlock class", stale since D-172 (PR #570) retired that mechanism.
- **Issue #703** has a published plan; the planner recommends splitting it into Part 3A
  (rendering, closes #705) and Part 3B (payload materialization, subsuming #711 and #714),
  with a "Step 0 — handoff" instructing the executor to open those sub-issues. That split has
  not been opened.
- **Reviewer plugin** `ievo@ievo-skills` is bound at 0.78.8; 0.80.21 is available.

## Autopilot state

Standing `/next-milestone` directive over milestone **v0.3**, whose Accept criteria are not
met. Last iteration's outcome: #734 implemented and its pull request opened.

**In-run denylist carried forward: #20, #631, #604, #558.** A resumed session must keep this
or its fresh inventory will re-select and re-fail them.

Next step: land this pull request, then resume `issue-select` at step 1 with a fresh baseline
— or take #703's 3A/3B split directly, since its plan is already published.

## Where to resume

- Issue: <https://github.com/rotnov/pycc/issues/734>
- Decision record: `docs/decisions/D-192-*.md`
- Skills changed: `.claude/skills/issue-select/SKILL.md` (and its Codex mirror
  `.agents/skills/issue-select/SKILL.md`), `.claude/skills/issue-implement/SKILL.md`,
  `.claude/skills/issue-to-plan/SKILL.md`, `.claude/skills/sessions/SKILL.md`,
  `.claude/skills/ultra-review/SKILL.md`
- Also touched: `AGENTS.md`, `docs/ROADMAP.md`, `docs/SPEC.md`, the D-066/D-130/D-185
  decision records, `scripts/run_alpha_skill_evals.py` and its test, and
  `scripts/validate_agent_assets.py`
- Harden journal: `.harden/incidents/new-case-misses-branching-sites/`
