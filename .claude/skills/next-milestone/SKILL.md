---
name: next-milestone
description: Use this alpha project skill when the user wants the next versioned milestone chosen and adopted as this session's standing autonomous goal — "what's next", "start the next milestone", "release the next version", or a bare autopilot directive with no milestone or issue named. Equivalent to the user typing "/goal release <name>" for whichever milestone is determined. Walk docs/ROADMAP.md's ordered milestone sections to find the first one whose Accept criteria are not yet met with real evidence, ensure its GitHub milestone exists, adopt it as the standing goal without asking, then hand off into the issue-select loop scoped to it.
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

Fetch and record the default-branch tip, per [D-021](../../../docs/DECISIONS.md#d-021-agent-task-preflight-and-documentation-refresh).

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
[D-127](../../../docs/DECISIONS.md#d-127-autonomous-agent-operation-model), do not pause to ask for
confirmation before proceeding. If the milestone was ambiguous (two adjacent sections both plausibly
"not yet met," e.g. a partially-shipped one with an open dispute in its own prose), resolve it via
the `advisor` tool rather than asking the user.

### 5. Hand off to issue-select, scoped to this milestone

Invoke `.claude/skills/issue-select/SKILL.md`'s loop with this milestone as the active scope: its
own step 2 milestone-triage housekeeping, its blocker screen's roadmap-fit check, and
[D-021](../../../docs/DECISIONS.md#d-021-agent-task-preflight-and-documentation-refresh) step 10's
issue-to-plan gate all already understand milestone-scoped work — this skill does not duplicate any
of that, it only establishes which milestone they're operating against.

### 6. Milestone completion and continuation

The loop above runs until it hits one of `issue-select`'s own stop conditions, or until this
milestone's Accept criteria are all met by the same evidence bar step 2 used to judge prior
milestones. On completion: record the "Update (`<date>`): met." note in `docs/ROADMAP.md` (the
existing convention — see the v0.1/v0.2 sections for examples), close the GitHub milestone, and — if
the original directive was open-ended ("release everything" rather than one named milestone) —
re-enter step 1 to find the next one. A directive scoped to exactly one milestone stops here instead
of auto-advancing.

## Output

The determined milestone name, the evidence review that ruled out every earlier milestone as
already met, confirmation the GitHub milestone exists, and the handoff to `issue-select`.
