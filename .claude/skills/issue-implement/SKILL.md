---
name: issue-implement
description: Use this alpha project skill when the user wants a GitHub issue in this repository taken end to end in one autonomously driven session — triaged for staleness against the current tree, planned when no current plan exists, implemented, deep-reviewed until a round reports no actionable findings, and merged. Close a stale issue only with cited evidence that its premise no longer holds, obtain or refresh the plan through issue-to-plan, monitor CI and review threads including inline comments, and merge only after re-reading the full pull-request diff with every required gate green. Explicit invocation authorizes the skill's enumerated public writes for the named issue without per-payload confirmation.
---

# issue-implement (Alpha)

Take one GitHub issue from triage to a merged pull request — or to the honest terminal state
short of that: an evidence-backed closure, or a stop with a reason. The session runs
autonomously; it stops only at the conditions listed at the end, not to ask for routine
confirmation.

This project-local skill is alpha. It has no bound evaluation runners yet, so treat its
judgment calls as reviewed-draft quality rather than validated workflow.

## Scope

Use it when the request is "implement issue #N", "take this issue end to end", or "close this
out if it's stale, otherwise build it".

Do not use it to produce only a plan (`/issue-to-plan` is that skill), or to report a new
defect (`/pycc-feedback` is that skill). If the user wants the work but not the merge, they
say so and the run stops after the pull request is green.

## Authorized writes

Explicit invocation of this skill for a named issue authorizes exactly these public writes,
without per-payload confirmation:

1. a closure comment on that issue, and closing it, when staleness is proven;
2. the plan comment that `/issue-to-plan` publishes to that issue when this skill invokes it;
3. pushing the task branch and opening the pull request that names the issue;
4. replies to, and resolution of, review threads on that pull request;
5. merging that pull request once every gate below is satisfied, and deleting the task branch.

Anything outside this set — touching another issue, editing an existing comment, force-pushing
over commits this session did not create, changing repository settings — still requires asking
first. `pycc-feedback`'s per-payload confirmation gate is deliberately not carried over here;
autonomy over this bounded set is the point of the skill.

## Workflow

### 1. Preflight (D-021)

Record `git status --short --branch` and the current commit. Fetch and prune, resolve the
remote default branch, and start from its exact tip in a clean task branch or isolated
worktree. Run `cargo doc --workspace --no-deps`. Read `docs/SPEC.md` and the specifications
owning the affected area. Checkpoint the open pull requests per D-078: number, state, draft
status, head; they may already be changing the files this issue targets, and they consume
shared decision-log numbering.

### 2. Triage for staleness

The issue was written against an older tree; its premise may have been resolved by unrelated
work since. Read the newest comments before re-deriving anything: this repository's issues
accumulate "reconfirmed at commit X" comments, and a reconfirmation at or near the current
default-branch tip settles "still current" immediately, while one at an old commit is dated
evidence exactly like the body. Then extract the premise — the observable defect or gap the
issue claims — and re-verify it against the current tree: run the issue's own reproduction
commands verbatim where it provides them, read the code or document it describes, and search
the history since the issue's creation date for merged work in that area.

Calibrate the prior to the tracker's hygiene: when resolved issues are being closed promptly
(check the recently-closed list), an issue that is still open is probably still real, and the
closure outcome below needs correspondingly strong evidence — a premise that fails to
reproduce plus the specific merged change that resolved it, not just the absence of a quick
repro.

Four outcomes:

- **Resolved.** Close the issue with a comment that cites the exact evidence — the commit or
  merged pull request that resolved it, and what was re-run or re-read to confirm the premise
  no longer holds. The comment states what was checked, not just the conclusion.
- **Still current.** Proceed.
- **Partially resolved.** Do not close. Comment with the same evidence standard, narrowing the
  issue to what remains, and implement the remainder; the plan must reflect the narrowed
  scope.
- **Inconclusive.** Stop and report. Never close on suspicion — the same bar D-022 sets for
  filing reports applies to closing them.

### 3. Obtain a current plan

Look for an implementation plan in the issue's comments. Plans published by `/issue-to-plan`
record the baseline commit they were planned against: check whether the default branch has
since moved in ways that matter — files the plan touches, gates it cites, open pull requests
it reasons about. A plan whose relevant ground has shifted is refreshed by invoking
`/issue-to-plan` again, not followed on faith.

If no plan exists, invoke `/issue-to-plan`. This skill's declared write authorization
substitutes for that skill's per-payload publish approval; everything else about its workflow,
including its adversarial review loop, runs unchanged.

### 4. Implement

Follow the plan. Write tests for success, failure, and edge paths alongside the behavior —
the coverage gate is a merge invariant, not a target. Update every affected document in the
same commits as the code. Before entering review, run the full local gate set: the coverage
gate with its preparatory builds exactly as CI performs them, the `scripts/` unittest suite,
the agent-asset and agent-policy validators, and clippy with warnings denied.

If the tree refutes the plan mid-implementation — an assumption fails, a gate behaves
differently than planned — do not force it. Record what refuted it, refresh the plan if the
refutation changes the approach, and note the deviation in the pull request body. A plan
refuted twice on the same point is a stop condition.

### 5. Review loop (D-068)

Stage all changes, including new files — the pinned reviewer omits untracked files from
working-tree review. Invoke the pinned deep reviewer from the digest-recorded artifact; if
that exact reviewer cannot be bound, the review gate is unavailable — report that and stop
rather than substituting a weaker reviewer or skipping the loop.

Verify each finding against the sources before acting on it. A finding confirmed by evidence
gets a focused fix; a finding refuted by evidence gets its reasoning recorded, not a blind
fix. Rerun the review after fixes whenever the previous findings may no longer describe the
diff. The loop ends when a round reports no actionable findings. The same finding surviving
two genuine fix attempts is a stop condition, not a reason for a third identical attempt.

### 6. Pull request

Re-fetch. If the default branch moved, rebase the task branch — own committed work only,
never over commits this session did not create — and rerun the local gates. Push and open the
pull request: `Fixes #N` in the body, a summary of what was built, any plan deviations with
their reasons, and the test evidence. For significant work, update `docs/SESSION_LOG.md`
within the pull request per D-066, re-fetching immediately before that commit so every
referenced remote state is current.

### 7. Monitor (D-078)

Establish the monitoring checkpoint, then react only to real events: a new default-branch
commit, a state, head, mergeability, review-thread, or required-check change on the task pull
request. Before waiting on CI, query the pull request's current state; stop waiting the
moment it closes, becomes conflicting, or its head is superseded.

Read every review comment, including inline pull-request comments, not just top-level reviews.
For each: a confirmed finding is fixed through step 5's loop and pushed; a refuted finding
gets an evidence-backed reply. Either way, resolve the thread afterwards — branch protection
requires resolved conversations, so an unresolved thread is a merge blocker regardless of its
merit.

Attribute CI failures before reacting. A failure attributable to the diff goes back through
step 5. A known-noisy gate failing in a way unrelated to the diff — the nbody speedup gate on
shared runners is the standing example — gets one re-run; if it persists, treat it as real and
investigate. If the default branch moves mid-monitoring, reconcile once; two consecutive
failed reconciliation rounds against a moving target is a stop condition.

### 8. Merge

Preconditions, all of them: every required check green including the coverage gate, zero
unresolved review threads, zero unaddressed actionable findings, branch up to date with the
default branch. Then re-read the full pull-request diff, end to end, immediately before
merging — the last look is not ceremonial; anything found there goes back through step 5.

Merge with a merge commit, delete the task branch, and confirm the issue closed via the
`Fixes #N` reference. Fetch and verify the default branch actually contains the work before
reporting it merged.

## Stop conditions

Stop and report — with everything completed so far delivered — when: staleness is
inconclusive; the plan is refuted twice on the same point; a review finding survives two
genuine fix attempts; two consecutive reconciliations against a moving default branch fail;
a CI failure cannot be attributed after a re-run and an investigation; the pinned reviewer
cannot be bound; or the task branch's remote head moves in a way this session did not cause —
never force-push over commits that appeared from outside.

## Output

A report naming the terminal state — issue closed as stale with the comment link, pull
request merged with the link, or stopped with the reason — plus the evidence cited, the
number of review rounds and what each changed, the CI history in one line, and anything
deliberately left out.
