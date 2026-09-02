---
id: reviewer-flags-a-later-phase-deliverable/2026-09-02-issue-866
date: 2026-09-02
project: pycc
session: 2c68147a
trigger: self-post-failure
model: claude-fable-5-1
effort: medium
harness: claude-code
type: process
termination: Local-skill
related: [reviewer-hypothesis-refuted-on-verification/2026-08-22-issue-719]
fixture: none — prose addition to a dispatch-brief template; no behaviour fixture built
artifact: .claude/skills/issue-implement/SKILL.md
verify: manual — the next pile from this skill is the measurement; a refuted finding of this shape recurring there is a recurrence here
verdict: pending
---

# Incident: reviewer-flags-a-later-phase-deliverable

**Batch:** `.harden/findings/issue-866.jsonl`, findings 1 and 5 (both `refuted`)

## What happened

The D-068 reviewer, dispatched by `issue-implement` step 5, reported two
findings that were factually right and phase-wrong: (1) the reviewed range
contained no `docs/sessions/` handoff file -- true, because step 6 writes it
after the review loop, immediately before the pull request opens; (2) it
could not re-run gates or query GitHub to confirm claims in that file --
true, because the reviewer has Read and Grep only, by design. Both were
refuted at zero cost, but each consumed a verification step and one of them
was mis-recorded on the way into the pile (see
`process-record-written-without-read-back`).

## Why it was not caught

The brief template names what the reviewer must receive (issue number, plan
path, acceptance criteria) but not what the round must *not* expect. A
reviewer reasoning from the merged pull request's deliverable set will
always flag a deliverable the workflow schedules later. Gap type: content --
the brief rule fired and did not cover this.

## Artefact and why this type

One sentence in the step 5 brief paragraph of
`.claude/skills/issue-implement/SKILL.md`: the brief states which step-6
deliverables are absent from the range by design and that gate/GitHub
claims are verified by the orchestrator. Documentation rung: the class is
caught at review tier at zero cost, and no static command distinguishes
"expected later" from "missing". Reach is this skill only, which is also
the only dispatcher of that reviewer. Not folded into
`reviewer-hypothesis-refuted-on-verification` (counter 3, build nothing):
that topic is a reviewer predicting a code defect the tree refutes, with a
verify-before-acting discipline as the answer; here the reviewer was
correct about the tree and wrong about the phase, and the fix is brief
content.

## Proof

Pending. Verification is the next `issue-implement` pile: no refuted
finding about a step-6 deliverable or about the reviewer's own tool scope.
