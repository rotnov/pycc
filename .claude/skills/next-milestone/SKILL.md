---
name: next-milestone
description: Use this alpha project skill when the user wants the next versioned milestone chosen and adopted as this session's standing autonomous goal — "what's next", "start the next milestone", "release the next version", "release everything", "run the whole roadmap", or a bare autopilot directive with no milestone or issue named. Equivalent to the user typing "/goal release <name>" for whichever milestone is determined, and — for an open-ended directive — repeating that for every subsequent milestone in sequence until v1.0 with no further prompting. Walk docs/ROADMAP.md's ordered milestone sections to find the first one whose Accept criteria are not yet met with real evidence, ensure its GitHub milestone exists, adopt it as the standing goal without asking, then hand off into the issue-select loop scoped to it.
---

# next-milestone (Alpha)

Resolve *which* milestone to work on, then behave exactly as if the user had given a standing
`/goal release <name>` directive for it. This skill does not pick an issue or write code itself —
step 5 hands that off to `.claude/skills/issue-select/SKILL.md`'s existing loop. Treat this as the
entry point one level above issue-select: issue-select assumes the milestone is already known;
this skill is what determines it in the first place.

This project-local skill is alpha, matching `issue-select` and `issue-to-plan`'s own alpha status.

## Scope

Use it when asked "what milestone is next", "start the next milestone", "release vX" with no
version given, or a standing autopilot directive that names no milestone and no issue. Do not use
it once a milestone is already known and confirmed (go straight to issue-select scoped to it), and
do not use it to plan or implement anything itself (issue-to-plan / issue-implement).

## Workflow

### 1. Baseline

Fetch and record the default-branch tip, per [D-021](../../../docs/decisions/D-021-agent-task-preflight-and-documentation-refresh.md).

### 2. Determine the next milestone

Read `docs/ROADMAP.md`'s milestone sections (`## vX.Y — ...`) **in file order** — that order is
the authoritative delivery sequence, not GitHub milestone numbers or creation dates. For each,
starting from the earliest, judge whether its **Accept:** bullet is stated as met with cited
evidence in the prose (an explicit "Update (`<date>`): met." note backed by a named PR, CI run, or
cross-referenced `docs/PYTHON_STANDARDS.md` count — not a bare unqualified claim). Verify any cited
evidence against the current tree rather than trusting the prose at face value; a stale "met" note
whose cited PR was later reverted, or whose referenced count no longer holds, does not count.

The **first milestone in sequence whose Accept criteria are not stated as met** is the next
milestone. A milestone is never skipped ahead of an earlier, still-unmet one, even if the later
one looks more tractable — this project's roadmap is an ordered sequence, not an unordered
backlog.

### 3. Ensure the milestone exists in GitHub

Check for an open GitHub milestone with a matching title (`gh api repos/<owner>/<repo>/milestones`).
Create it if missing (title matching the `## vX.Y — <name>` heading exactly, description summarizing
the section's own scope line). Do not create milestones speculatively for versions beyond the one
just determined — one milestone ahead is enough; later ones are created by this same skill when
their turn comes.

### 4. Adopt it as the standing goal

Report the determined milestone and its Accept criteria, then proceed exactly as if the user had
typed `/goal release <name>` for it — per
[D-127](../../../docs/decisions/D-127-autonomous-agent-operation-model.md), do not pause to ask for
confirmation before proceeding. If the milestone was ambiguous (two adjacent sections both plausibly
"not yet met," e.g. a partially-shipped one with an open dispute in its own prose), resolve it via
the `advisor` tool rather than asking the user.

### 5. Hand off to issue-select, scoped to this milestone

Invoke `.claude/skills/issue-select/SKILL.md`'s loop with this milestone as the active scope: its
own step 2 milestone-triage housekeeping, its blocker screen's roadmap-fit check, and
[D-021](../../../docs/decisions/D-021-agent-task-preflight-and-documentation-refresh.md) step 10's
issue-to-plan gate all already understand milestone-scoped work — this skill does not duplicate any
of that, it only establishes which milestone they're operating against.

### 6. Milestone completion

`issue-select`'s own loop (step 5) runs until it hits one of its own stop conditions, or until this
milestone's Accept criteria are all met by the same evidence bar step 2 used to judge prior
milestones. On completion: record the "Update (`<date>`): met." note in `docs/ROADMAP.md` (the
existing convention — see the v0.1/v0.2 sections for examples), update `README.md`'s status blurb
to describe the newly met milestone (it drifted silently after v0.2 — the v0.1-only wording sat
unchanged through all of v0.2 landing), refresh `docs/ROADMAP.md`'s "Current delivery status"
section — update the "Current milestone: ..." line to reflect the newly met milestone and bump the
"Last reviewed on ..." date to match (this third update was missing from step 6's original list and
let the section drift after v0.2 completed), and close the GitHub milestone.

**Content-complete is not released.** Meeting a milestone's Accept criteria only means the code and
its evidence are on `main` — it is not by itself a release. `docs/DISTRIBUTION.md`'s own "Release
and verification" section states this explicitly: "merging the manifest, or this workflow itself,
does not by itself create or advertise a release tag." Before tagging:

1. Dispatch `hook-install-check.yml` against the milestone-completing commit
   (`gh workflow run hook-install-check.yml --ref <sha>`) and wait for its Tier-1 result.
2. Record that dated run in `docs/DISTRIBUTION.md`'s "Current Tier-1 installation evidence"
   section, replacing the prior entry.
3. Only once that evidence is committed, create the release tag (`git tag vX.Y.0 <sha>`, pushed)
   matching the milestone name. A milestone with no dated post-completion Tier-1 evidence has not
   been released yet, no matter how long its Accept criteria have been met — this is exactly the
   gap that left v0.2 content-complete but untagged.

## Loop

A directive scoped to exactly one named milestone ("finish v0.3") stops at step 6 once that
milestone completes — do not auto-advance past what was actually asked for.

An **open-ended** directive ("release everything", "run the whole roadmap", a standing autopilot
directive naming no specific version) means a loop, not one milestone: when step 6 closes a
milestone, re-enter step 1 with a fresh baseline — the just-completed milestone moved the default
branch and may have changed later sections' standing (a later milestone's Accept bullet can
reference work the completed one shipped). Every iteration re-derives its milestone determination
from scratch per step 2; nothing about *which* milestone is next carries forward between
iterations, only the fact that the directive itself is still open-ended.

The loop ends only when: the user stops it; `docs/ROADMAP.md` has no further `## vX.Y` section
whose Accept criteria are unmet (v1.0 and, if in scope, v1.x reached); or `issue-select`'s own
**systemic** stop condition fires partway through a milestone (report which, and at which
milestone — do not silently drop back to step 1 as if that milestone were complete). A milestone
that completes with parked Minor/deferred findings per `issue-implement`'s own conventions is a
normal completion, not a stop condition.

## Output

The determined milestone name, the evidence review that ruled out every earlier milestone as
already met, confirmation the GitHub milestone exists, the handoff to `issue-select`, and — once
that milestone completes under an open-ended directive — the same report for every subsequent
milestone the loop advances through.
