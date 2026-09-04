# 2026-09-04-07 — issue #923, the llms.txt context budget

## Previous checkpoint's merge outcome

Pull request #924 (issue #910, un-annotated class attributes) merged
2026-09-04T18:30:50Z as `12650781e6130d4fd30681795d85ed4b4cdc2c4e`. Issue #910 is
CLOSED and the branch is deleted. That outcome belongs here rather than in
`2026-09-04-06-issue-910-class-attrs-inferred.md`, whose `## Merge outcome`
placeholder stays as written: under D-130 a previous checkpoint's file is never
edited to append a later event.

## Status

Autopilot iteration 07 under the standing directive, milestone scope v0.4. Base
`12650781`, worktree `.claude/worktrees/autopilot-2026-09-04-07`, branch
`autopilot/iter-2026-09-04-07`.

`issue-select` ran against `main`'s authoritative skill copy. TOTAL_OPEN 119,
NO_MILESTONE 69 against the D-192 ceiling of 20, so this iteration filed no new
non-milestone issue. The in-scope v0.4 P2 tier was #414, #585, #636, #923; the
first three were excluded on verified blockers and #923 selected, with a clean
adversarial round.

## What is in flight

Four commits on the branch, all local at the time of writing, delivering the
budget work for #923:

- `65b00881` the main change — trims `docs/ROADMAP.md` by 46,051 bytes and
  partitions the ceiling across the six per-resource budgets under a new
  `sum(budget_bytes) <= budget_kib * 1024` invariant in `scripts/check-site.sh`.
  `budget_kib` stays 272; D-227 records the decision.
- `f71a2f71` bumps the roadmap's own `Last reviewed on` date, which the review
  round found stale, and starts this issue's findings pile.
- `1e1bb8c3` repairs that pile to the schema `scripts/check_harden_findings.py`
  enforces. Worth knowing: the pile as first written made
  `python3 -B -m unittest discover -s scripts` red, so the harden batch caught a
  CI-red defect that the ordinary gate sweep had already passed over.
- `d7587d6d` records the harden batch — four classes, no new artefact, one
  retrospective entry.

## Decisions worth carrying forward

The issue proposed raising the ceiling a third time. Two measurements refused
that: the D-200 to D-218 window spans 36 merges in 7 days at 145 roadmap
bytes per merge, which is what a suppressed growth rate looks like rather than
evidence a raise buys a dozen features; and the six per-resource budgets summed
to 339,968 against a 278,528 ceiling, so they were decorative and could never
fire. The binding constraint after this change is the roadmap's own budget at
143,311 of 168,960 — about 14 further 1,800-byte paragraphs. Aggregate headroom
reads 44,249 but is no longer what fails first, so quoting it as "the headroom"
overstates the runway by 1.7x.

`docs/ROADMAP.md:41` is a machine-bound projection carrying 26 exact phrases that
five checkers require, plus a third copy of the 272 KiB figure. It is byte-for-byte
identical across this change (`547d6b93...6510` before and after) and should stay
that way. `docs/ROADMAP.md:183` is parsed by `check_conformance_breadth.py`.

## Known follow-ups

- `.claude/skills/harden/references/batch.md` says a findings pile is
  "append-only; never rewritten", but `scripts/test_check_harden_findings.py`
  requires every checked-in pile's current content to pass the schema checker. A
  pile committed with a schema violation therefore cannot be repaired by
  appending. The two are in direct conflict and this iteration landed the first
  case that exposes it. Route to umbrella #806 (agent tooling); it is not this
  change's defect.
- `site/status/index.html` still claims v0.4 "has not started" at `:158` and in
  its `#next` section. Those literals are pinned by `scripts/check-site.sh`'s
  `required_visible_text`, so they cannot be corrected without moving the pin.
  Already narrowed onto umbrella #802 during iteration 06; this change bumps the
  page's `dateModified` for the freshness gate without touching them, so that
  bump must not be read as re-verification.
- `check_readme_milestone_projection.rb` aborts with `invalid byte sequence in
  US-ASCII` without `LC_ALL=en_US.UTF-8`. The locale prefix requirement extends to
  a second Ruby checker beyond `check_roadmap_evidence.rb`.
- `.harden/findings/` collection fails silently in clones whose
  `.git/info/exclude` carries a `.harden/` line: already-tracked piles still show
  as modified, but every new pile is swallowed with `git add -A` reporting
  nothing. `issue-910.jsonl` is absent for exactly this reason. Confirm a pile
  actually landed with `git diff --cached --name-only`, not with a clean
  `git status`.

## Where a fresh session should resume

Open the pull request for this branch if it is not yet open, watch it with
`.claude/skills/gha-watch-ci-pr/scripts/ci-watch.sh rotnov/pycc <pr>` under
`Monitor` (`persistent: true`, `timeout_ms: 3600000`) rather than any hand-rolled
`gh` polling loop, and merge once green. Record its merge outcome in the *next*
iteration's session file, never by editing this one.

## Paused autopilot

- **Directive scope:** open-ended — work the open-issue backlog to closure, one
  issue at a time.
- **Active milestone:** v0.4.
- **Last iteration outcome:** #923 implemented, reviewed over two rounds, harden
  batch complete; pull request pending.
- **Next step:** finish #923's pull request, then re-run `next-milestone`'s step 2
  evidence check against v0.4's Accept criteria before re-entering `issue-select`
  step 1.
- **In-run denylist:** empty. #414, #585 and #636 were excluded by the blocker
  screen, not by a per-issue stop condition, so a later iteration re-examines them
  from a fresh inventory.
