# 2026-09-11 (03) — #261: one clean-round contract for `issue-to-plan`'s review loop

## Overall status

Task base: `747a5677` (`origin/main`, "Sweep the decisions comment references and
pin the sweep (#1007)"). Branch: `autopilot/iter-2026-09-11-01`.

This session delivered one pull request against
[#261](https://github.com/rotnov/pycc/issues/261), a P2 v0.4 issue reporting that
`.claude/skills/issue-to-plan/SKILL.md`'s adversarial review loop had no
unambiguous clean-round condition, no fail-closed action when every round keeps
changing the plan, and a `## Stop conditions` section that defined the same rule a
second time in different words. `#261` closes with this merge.

The rewrite gives step 7 sole ownership of the loop's two terminal states — a clean
round, and an impasse that forbids publishing — and binds that contract to offline
eval runners so the text cannot be deleted or weakened with the gates green.

## What changed

| file | change |
|---|---|
| `.claude/skills/issue-to-plan/SKILL.md` | step 7 rewritten as the single canonical statement of the loop's termination rule; the frontmatter description, the alpha disclaimer, Non-negotiable #3, step 8's Publish entry, `## Stop conditions` and `## Output` all aligned to it. `## Stop conditions` is now a pure cross-reference, not a second definition. |
| `.agents/skills/issue-to-plan/SKILL.md` | frontmatter `description` mirrored byte-identically (the Codex wrapper's body is a pointer and was deliberately left alone — a second canonical-path mention breaks two validators). |
| `scripts/run_alpha_skill_evals.py` | new `PlanReviewLoopState`, `plan_review_terminal_state`, `plan_publication_authorized`, `plan_comment_may_be_posted`; four new runners; a named `ISSUE_TO_PLAN_LOOP_CONTRACT` of six pinned phrases concatenated into `ISSUE_TO_PLAN_CONTRACT`. |
| `scripts/validate_agent_assets.py` | the same four runner names in `ALPHA_EVAL_RUNNERS["issue-to-plan"]`. |
| `.claude/skills/issue-to-plan/evals/evals.json` | four new cases, ids 4-7. |
| `scripts/test_run_alpha_skill_evals.py` | branch coverage for the three new oracles, both authorization arms, the two-gate conjunction, and a per-phrase removal test proving each pinned phrase is individually load-bearing. |
| `docs/AGENT_TOOLING.md` | the loop description now points at step 7 instead of paraphrasing it; runner counts corrected to `issue-to-plan` seven and `issue-select` eight (the latter was already stale); the oracle description widened. |
| `.claude/skills/issue-implement/SKILL.md` | one line that restated the round count now cross-references step 7's definition. |
| `docs/AGENT_RETROSPECTIVE.md` | one new entry, see below. |
| `.harden/findings/issue-261.jsonl`, `.harden/incidents/` | the review pile and the `/harden batch` counters. |

The eval surface could not be extended in place. `PlanPublicationState`'s three
booleans are a payload-consent gate over a single payload, while the scenarios #261
asks for range over an ordered sequence of round outcomes; and
`run_issue_to_plan_case` asserted refusal *outside* every branch, so no
positive-outcome runner could exist at all. The assertion was pushed down into each
of the three existing branches first, mirroring `run_issue_implement_case`'s shape,
before the four new branches were added.

One subtlety is worth recording because it is easy to get backwards: the publish
oracle's consent half is a **disjunction**, not `plan_publication_allowed` alone.
On `issue-implement`'s delegated path (D-143) nothing is previewed and nothing is
approved, so `plan_publication_allowed` is false while publication is genuinely
authorized — a bare conjunction would have encoded a necessity the skill's own text
contradicts. The fourth runner exists precisely to prove that the delegated path,
the dominant one under autopilot, still cannot publish an unclean plan.

## Verification

Mutation control, the proof that the pins are load-bearing rather than decorative
(run against a scratch copy, never the tree):

| control | before the change | after |
|---|---|---|
| delete step 7 and `## Stop conditions` wholesale | all three cases **pass** — the defect | all seven cases **fail** |
| delete only the anti-gaming guard sentence | n/a (not pinned) | all seven cases **fail** |

The three pre-existing `issue-to-plan` cases returned identical verdicts across the
assertion push-down, and a monkeypatched positive control confirmed each refactored
branch still raises, so no branch silently lost its guard. A separate gate-dispatch
control inverted one new runner's assertion in a mirrored tree and confirmed the CI
entrypoint reports it (exit 1), so the four runners are genuinely executed rather
than merely name-checked.

Gates, each run so its exit status survived (`cmd > log 2>&1; echo $?`):

| gate | exit |
|---|---|
| `python3 -B scripts/validate_agent_policies.py` | 0 |
| `python3 -B scripts/validate_agent_assets.py` | 0 |
| `python3 scripts/run_alpha_skill_evals.py --client codex --pycc-bin target/debug/pycc` | 0 |
| `python3 scripts/run_alpha_skill_evals.py --client claude --pycc-bin target/debug/pycc` | 0 |
| `python3 -B scripts/check_decision_immutability.py --base <merge-base> --head HEAD` | 0 |
| `ruby scripts/check_site_pin_merge_currency.rb <merge-base> HEAD .` | 0 |
| `python3 scripts/check_harden_findings.py .harden/findings/issue-261.jsonl` | 0 |
| `cargo build --workspace` | 0 |
| `python3 -B -m unittest discover -s scripts -p 'test_*.py'` | **1**, see below |

The unittest suite exits 1 in this worktree on exactly one test,
`test_check_harden_findings.FindingsCheckerTests.test_real_repository_piles_conform`,
which reports untracked findings piles for seven unrelated issues. Those seven files
are hidden from `git status` by a machine-local exclude and do not exist in a clean
checkout, so the job is green in CI; `git diff --name-only <base> HEAD` confirms this
change touches no other `.harden/findings/` path. Recorded rather than waved away,
because a red gate reported as green is the failure mode this project keeps hitting.

The Rust coverage gate and `scripts/check-site.sh` do not apply: the change touches
no Rust and no document in the llms.txt context manifest.

## Review

The D-068 pinned reviewer (iEvo `deep-reviewer`) ran three rounds over the full
committed range from the merge base.

Round 1 raised two findings, both fixed in `cff79ba5`. The warning is the one worth
reading: step 7's new clean-round and impasse definitions were **not disjoint** — a
round whose only finding is a recurring one resolved as "considered, no change"
satisfies both — and the precedence between them existed only in the oracle's arm
order and a test comment, never in the canonical text. That is the same defect #261
exists to remove, reintroduced one level down by its own fix. The precedence is now
stated in step 7 and pinned. The note was a mutation test selecting its phrases by
positional slice, fixed by naming `ISSUE_TO_PLAN_LOOP_CONTRACT` — which the first fix
would otherwise have silently mis-scoped in the same round.

Round 2 raised two notes, both fixed in `26ec5ca1`: the anti-gaming guard sentence
was carried forward in the prose but pinned by nothing, and one eval case's prompt
described its arms as differing in *why* a round was clean when the runner's arms
differ in *when* the clean round falls. Round 3 returned no actionable findings.

`/harden batch` over the four-finding pile clustered them into three classes and
shipped **zero artefacts, three journal counters** — for all three, review is the
terminal catching rung. The one decidable candidate gate (every sentence in the
contract block is pinned or explicitly exempted) was rejected because it cannot
catch a sentence that was never written, and because it is satisfied by appending to
an exemption list. The `doc-comment-drifts-behind-a-widened-constant-table` class was
settled by counting: exactly one site had the coupling, and this change's own fix
already removed it.

## Follow-ups

- `.claude/skills/issue-to-plan/SKILL.md`'s workflow steps are still numbered as they
  were before an earlier shift, and `docs/decisions/D-143-*.md` restates the
  pre-shift numbers in its Context. Both were left alone deliberately — D-240 makes
  the decision file insert-only, and the drift predates this change.
- `issue-to-plan`'s review loop still has no `.harden/findings/` wire of its own; only
  `issue-implement`'s deep-review loop does. Adding one is its own change.
- `.claude/skills/issue-select/SKILL.md` and the two `review-findings` copies each
  define a clean-round rule for their **own** loops. They are deliberately not
  unified with this one; three sibling rules for three different loops is the correct
  shape, and a future sweep should not read this change as license to merge them.

## Where to resume

Read the most recent files under `docs/sessions/` in filename order. The autopilot
loop is mid-run: see the section below.

## Paused autopilot

- **Directive scope:** open-ended (`/goal fix all opened issues`), so the loop
  re-enters `issue-select` after each merge rather than stopping at a milestone.
- **Active milestone:** v0.4.
- **Last iteration's outcome:** #261 selected, planned, implemented and merged.
- **Next autopilot step:** re-enter `issue-select` for v0.4.
- **In-run denylist:** empty — no issue reached a per-issue stop condition this run.
