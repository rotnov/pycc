# 2026-09-10 (05) — issue #260: derive the alpha promotion gate from the runner table

## Status

Delivered by the pull request that carries this file. Base `fd16e766`
(`origin/main` at branch time, PR #1002 already merged); branch
`feat/issue-260-alpha-promotion-gate`, commits `6c065de3` (the gate
derives its skill set from `ALPHA_EVAL_RUNNERS`, tests), `29e28db5` and
`6f93b1ba` (documentation, review round 1 fixes), `1519cb3d` (the harden
artefact `validate_alpha_skill_count_prose`), `2277e63b`, `13feef8f`,
`d9bcbefc`, `8fe9627e`, `93d9193b` (review rounds 3 through 7 fixes),
`d7958fd5` (the prose guard counts only numerals adjacent to "alpha
skill(s)", after the external review on PR #1003), `7a570baf` (the
promotion gate inverted, after the same review), plus the commits carrying
this file and the harden journal.

## What landed

- `scripts/validate_agent_assets.py`: `PROJECT_ALPHA_SKILLS` is deleted.
  `validate_alpha_promotion_gate` treats every locked skill as a promotion
  candidate unless it is named in `EXTERNAL_ORIGIN_LOCKED_SKILLS`, a reviewed
  exemption asserting external origin (currently `i-have-an-issue`); it
  iterates `sorted(set(locked_skills) - EXTERNAL_ORIGIN_LOCKED_SKILLS)` at
  call time and first rejects an exemption that names an alpha skill or a
  name outside `EXPECTED_SKILL_LOCK_ENTRIES`. `validate_alpha_skill_contracts`
  iterates `sorted(ALPHA_EVAL_RUNNERS)`, so a skill added to the runner table
  falls under the structural check without a second hand-maintained list.
  `ALPHA_EVAL_RUNNERS` still mirrors `EXPECTED_RUNNERS` in
  `scripts/run_alpha_skill_evals.py` by hand; the code comment names that gap.
- `validate_alpha_skill_count_prose` (the harden artefact): rejects a
  literal alpha-skill count in `docs/AGENT_TOOLING.md` that disagrees with
  `len(ALPHA_EVAL_RUNNERS)` when at most two words separate the numeral
  from a following "alpha skill(s)" (an issue number such as `#260` never
  counts) or it is immediately followed by "skill(s)", "alpha",
  "project-local", or "at the time of writing" inside a one-line sentence
  that mentions `ALPHA_EVAL_RUNNERS`; bound phrases ("at least", "at most",
  "more than", "fewer than", "up to") are excluded. It runs from `validate_skill_lock`
  before the lock-shape early return.
- `scripts/test_validate_agent_assets.py`: promotion tests parametrised over
  every table entry (absent, codex-only, claude-only, all present), the
  non-HTTPS shape, a derivation-from-table proof via `mock.patch.dict`, a
  vendored-skill-ignored proof (`i-have-an-issue`), and six prose-guard
  tests (stale spelled-out count, stale digit after the table mention,
  matching count accepted, unrelated numerals ignored, adjacent numerals
  counted under a widened table while distant ones stay ignored, the real
  document passes).
- `docs/AGENT_TOOLING.md` and `docs/ROADMAP.md`: the promotion gate is
  described as covering every locked skill outside
  `EXTERNAL_ORIGIN_LOCKED_SKILLS`, without a literal count; the structural check's trigger is scoped to agent-relevant
  pull requests and `main` pushes. No new decision entry: the change restores
  the policy D-190 already documents. D-190 and the dated plan file under
  `docs/superpowers/plans/` still name `PROJECT_ALPHA_SKILLS`; both are
  frozen records.

## Gates at HEAD

`python3 -B -m unittest discover -s scripts -p 'test_*.py'` OK (6 skipped),
`scripts/validate_agent_assets.py`, `scripts/validate_agent_policies.py` and
`ruby scripts/check_roadmap_evidence.rb` all exit 0. The prose guard, run
with the runner table forcibly emptied, reports lines 245, 247 and 253 of
`docs/AGENT_TOOLING.md` (the three legitimate "seven" mentions) and nothing
else. No Rust or coverage impact: the diff touches Python validators and
Markdown only.

D-068 review: `ievo:deep-reviewer` rounds 1 through 8. Round 1 found the
newly written prose reintroducing a literal count and a dangling "Until
then" (fixed `6f93b1ba`); round 2 clean; rounds 3 through 7 each found one
or two wording inaccuracies in the guard's own description or the code
comment (per-line scope, early return, pronoun antecedent, sentence
granularity, "single owner", trigger scope) and were fixed in the commits
listed above; round 8 clean. Every finding, including the external ones
and the rounds run after them, is in `.harden/findings/issue-260.jsonl`
(all `fixed`).

External review on PR #1003 (Codex, P2) showed the prose guard's
40-character window counting unrelated numerals ("the two clients cover all
seven alpha skills", "#260 covers every alpha skill"); rule A now allows at
most two words between the numeral and the phrase and skips `#`-prefixed
numbers (`d7958fd5`). The same review (Codex, P1) showed the derived intersection
omits a skill promoted out of the table in the same change. The mechanism
was replaced after an independent advisor round: every locked skill is a
candidate unless a reviewed exemption asserts external origin
(`EXTERNAL_ORIGIN_LOCKED_SKILLS`), cross-checked against the alpha inventory
and the lock allowlist. The plan's literal criterion "derive the skill set
from `ALPHA_EVAL_RUNNERS`" is therefore superseded by a stronger predicate;
the alpha inventory is consulted only for the disjointness invariant.

Harden batch (one tracer dispatch over the round-1 pile): three records.
`doc-comment-drifts-behind-a-widened-constant-table` shipped the static
guard above (verdict `profit`, `verify: manual` with violator and clean
copies of the document). `plan-contradicts-its-own-constraint` is the third
occurrence in the journal; a textual rule is disqualified, and the record
proposes a review-check line for `issue-to-plan`'s step 7 reviewer brief
("for each constraint the draft states, name the item that satisfies it and
search the rest of the plan for an instruction that contradicts it"),
pending an arena run and outside #260's scope. A third record,
`own-change-falsifies-adjacent-prose`, is `build-nothing`: the review round
is the right rung for it.

## Known follow-ups

- The promotion gate's residual is a deliberate false exemption in a
  reviewed diff (a name added to `EXTERNAL_ORIGIN_LOCKED_SKILLS` in the same
  change that promotes it). A base-to-head transition check would close it;
  it is deferred under the D-192 filing bar since no promotion has ever
  occurred.
- `ALPHA_EVAL_RUNNERS` and `EXPECTED_RUNNERS` are two hand-synced tables; an
  equality check between them was deferred as out of scope.
- `docs/ROADMAP.md`'s unrelated "All seven alpha skills bind deterministic
  offline eval cases" sentence is outside the prose guard, which reads
  `docs/AGENT_TOOLING.md` only.
- The proposed `issue-to-plan` step 7 review-check line needs an arena run
  before it ships.

## Where to resume

`git log origin/main --first-parent -n 5`, `gh pr list --state open`, then
`issue-select`. PR #996 (`pycc_types` `tests.rs` extraction, part of #695)
was open at the time of writing; #423 and #371 touch the same file and wait
for it.
