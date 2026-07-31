---
name: issue-select
description: Use this alpha project skill when the user wants the next GitHub issue chosen autonomously for end-to-end implementation — "pick the next issue", "what should we take next", a standing autopilot directive over the tracker, or an issue-implement run with no issue named. Inventory the full open list against the refreshed default branch and open pull requests, exclude issues whose execution would need maintainer-only authority or decisions, verify the top candidate's premise still reproduces, challenge the pick with an independent adversarial advisor instead of asking the user, and hand the selected issue to issue-implement with a written justification.
---

# issue-select (Alpha)

Choose the next issue an autonomous session should take end to end, and justify the choice well
enough that the run can start without the user. The deliverable is a selection with reasoning —
this skill performs no public writes and mutates no tracked file.

This project-local skill is alpha. It has no bound evaluation runners yet; treat its judgment
as reviewed-draft quality.

## Scope

Use it when the user asks which issue to take next, gives a standing directive to work through
the tracker on autopilot, or invokes `/issue-implement` without naming an issue.

Do not use it to plan (that is `/issue-to-plan`) or to implement (that is `/issue-implement`).
It ends by naming the target; with a standing autopilot directive in effect it hands off to
`/issue-implement` directly, and without one it stops after reporting the pick.

## The autopilot bar

Every selected issue must be executable start to merge without a single question to the user.
The question "does this need the maintainer's decision?" is itself never escalated to the user
— it is put to the independent advisor (below), and an issue that genuinely needs the
maintainer is excluded, not asked about. Exclusion with a recorded reason **is** the correct
outcome for such issues; surfacing them in the report keeps them visible without blocking the
run.

Hard exclusions — needs authority or state an agent session does not have:

- release and tag lifecycle, publishing, launch gates, external promotion;
- repository settings, branch protection, secrets, external accounts or credentials;
- process-only asks that no repository change can close;
- work whose verification requires hardware or environments the session lacks, when the issue
  cannot be restructured to verify on what is available;
- anything whose execution path requires an explicit maintainer sign-off by this repository's
  own governance documents.

Deprioritized, not excluded — take only deliberately: changes requiring the staged
CI-workflow digest process; changes that would conflict with an open pull request's in-flight
rewrite of the same files; tree-wide mechanical sweeps that bloat review surface.

## Workflow

### 1. Baseline

Fetch and record the default-branch tip. Inventory open pull requests with the files they
touch — overlap is a selection criterion, not just a planning concern. Sample the
recently-closed issues: a well-tended tracker (resolved issues closed promptly) means open
issues are probably real, and it means staleness closures will be rare finds rather than the
default expectation.

### 2. Inventory the full open list

List every open issue — paginate past client truncation limits; the oldest issues are the best
staleness candidates and the likeliest to hide behind a cut-off list. Note age, priority
labels or markers, theme clusters, and comment counts.

### 3. Staleness screen

Cheap pass over the inventory before any scoring: read newest comments first — this tracker
accumulates "reconfirmed at commit X" comments that settle currency instantly — and give a
quick premise check to any issue whose area has visibly changed since it was filed. An issue
that provably no longer reproduces is not a selection candidate; it is closed **now**, during
the screen, by invoking `/issue-implement`'s evidence-gated triage for it — every provable
closure found, not just one, so the tracker is cleaned as a side effect of every selection
pass. Each closure individually meets that skill's evidence bar (the resolving change cited,
the premise shown not to reproduce); anything inconclusive stays in the pool marked as
unverified rather than being closed on suspicion.

### 4. Blocker screen

Drop or defer, with a recorded reason each:

- **Blocked by another issue** — the issue's own text or completion criteria depend on an
  unresolved issue landing first.
- **Roadmap and delivery-plan mismatch** — the issue targets surface the roadmap schedules for
  a later milestone, or its area is mid-rewrite per `docs/DELIVERY_PLAN.md`'s current PR
  decomposition, so a fix now would be built on code an in-flight pull request is replacing.
  Read the current milestone's plan before trusting an issue's own framing of where its area
  stands.
- **Open-pull-request collision** — an open pull request is actively rewriting the same files;
  weigh landing order and conflict surface, and prefer targets whose diff stays out of the
  contested code unless the fix is urgent enough to justify the rebase burden on either side.
- The hard authority exclusions above.

### 5. Score the survivors

The ordering is fixed: **the repository's own priority markers rank first** (P1 before P2
before P3 before unmarked), and **within the same priority, smaller wins** — least effort,
smallest blast radius, cleanest scope. Fast merges of the most important work beat both
"biggest first" and "easiest first". Within that frame, use as tie-breakers and modifiers:

- **Scope clarity** — itemized completion criteria and a pinned root cause make an issue
  cheap to plan and safe to execute; a vague aspiration is expensive at every later step.
- **Premise verifiability** — can the defect be reproduced in this environment right now?
- **Blast radius detail** — files touched, overlap with open pull requests, conflict surface,
  whether the diff can stay localized.
- **Soundness over polish** when two issues share a priority marker and size.

When the session has a stated secondary goal — exercising an untested workflow path, building
evidence for a later decision — weigh it explicitly and say so in the justification rather
than letting it silently bias the scoring.

### 6. Verify before proposing

Reproduce the top candidate's premise against the current tree before naming it: newest
comments first, then the issue's own reproduction commands verbatim. A selection that hands
`/issue-implement` a stale or unreproducible target wastes the whole downstream pipeline.
If the premise fails to reproduce, that candidate moves to the staleness screen's closure
routing, and selection continues with the next survivor.

### 7. Adversarial advisor round

Present the verified pick to an independent advisor agent in a fresh context — not the user —
with the full justification: why it fits, why it is unblocked, why nothing in it needs the
maintainer. The advisor's brief is to refute the selection:

- a hidden maintainer-owned decision inside the issue's completion criteria;
- a blocker the scoring missed — an open pull request about to rewrite the same code, a
  governance gate, an unverifiable acceptance criterion;
- a materially better candidate passed over for a stated reason that does not hold.

Answer every objection with evidence or change the pick. The round is clean when the advisor
raises nothing that survives verification. One clean round suffices; two objection rounds that
change the pick mean the scoring was wrong — redo step 5 with what was learned.

### 8. Hand off

Report: the selected issue with the justification (fit, unblocked-ness, the no-user-decisions
rationale, the advisor's verdict), the runners-up with one-line reasons, the closures made
during the staleness screen, and the exclusions worth the maintainer's own attention. With a
standing autopilot directive, invoke `/issue-implement` on the selection; its enumerated write
authorization covers the run from this point, and this skill's own report is delivered
alongside, not instead.

## Loop

A standing autopilot directive means a loop, not one pick: when the handed-off run reaches a
terminal state, deliver its brief report, then re-enter this workflow at step 1 — a fresh
baseline, because the just-merged work moved the default branch and may have changed other
issues' standing. The loop ends only when the user stops it, when an `/issue-implement` stop
condition needs the user, or when the pool has no survivors — report which. Never carry a
previous iteration's inventory, scores, or baselines into the next: every iteration re-derives
them.

## Output

The named issue, the written justification, the advisor round's outcome, runners-up, and any
excluded issues that deserve maintainer attention — followed by the `/issue-implement` handoff
when autopilot is in effect.
