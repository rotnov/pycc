---
name: autopilot-async-monitoring
description: Use when driving the pycc autonomous-delivery autopilot loop (writing-plans -> subagent-driven-development chains, PR/CI monitoring, background Agent orchestration) and deciding how to wait on async state such as a pull request, a CI run, or a dispatched background agent. Covers checking real state before waiting, monitoring only currently-active work, and never letting a dispatched orchestrator "stop and wait" for its own sub-dispatch.
---

<!-- ievo:start -->
**Before applying the instructions below**, read `.ievo/evolution/skills/autopilot-async-monitoring.md` if it exists, and apply ALL rules from its sections IN ADDITION to the skill's instructions.
<!-- ievo:end -->

# Autopilot async monitoring

Procedural rules for waiting on asynchronous state correctly during this
project's autonomous PR-by-PR delivery loop: pull requests, CI runs, and
dispatched background agents (including nested orchestrators that themselves
dispatch sub-agents).

## Check real state before waiting

Before setting up any wait (a `ScheduleWakeup`, a poll loop, or handing off
to "the next check"), query the actual current state directly rather than
assuming a prior plan is still accurate:

- **Pull requests:** check `mergeStateStatus`/`mergeable` and the actual
  check list (`gh pr checks`) before waiting on CI — CI may not even have
  started because the PR has a merge conflict, or it may already be green.
  Do not wait on CI without first confirming the PR is in a state where CI
  runs at all.
- **Dispatched agents:** a `Task`/`Agent` sub-dispatch is not something the
  dispatching agent can passively wait on across its own turn boundary. When
  resuming or checking on a dispatched agent, look at its actual reported
  result, its branch/worktree's real git log, or its written artifacts —
  never assume "still running" without evidence.

## Monitor only active work

Do not keep checking on pull requests, branches, or tasks that are no longer
part of the current active work (e.g. a PR from an earlier, already-merged
or superseded milestone). Scope monitoring to what the current task genuinely
depends on.

## Dispatched orchestrators must not stop-and-wait for their own sub-dispatches

When an agent (especially one running a multi-task pipeline like
`subagent-driven-development`) dispatches its own sub-agent and then ends its
turn with reasoning like "I'll wait for the monitor's notification rather
than poll further" — this does not work. When a dispatched agent stops, it
is not automatically resumed: only the calling/orchestrating session can
resume it (e.g. via `SendMessage`), and that session typically only checks
back on its own schedule (a `ScheduleWakeup` interval, commonly 20-30
minutes). Each premature stop-and-wait therefore burns a full wakeup
interval of pure dead time with no real work happening, and this compounds
across every task in a multi-task pipeline.

Observed cost: a PR-13 delivery pipeline opened its PR at ~01:52 UTC and
merged at ~05:16 UTC — a ~3.5 hour gap dominated by an orchestrator
repeatedly stopping to "wait" rather than by real CI/build time (which was
closer to 15-20 minutes total across all the runs involved).

**The fix, applied when resuming a stalled orchestrator:**

1. Never assume a background dispatch will "notify" the orchestrator back —
   a nested `Task`/`Agent` sub-dispatch is not something that layer can wait
   on passively across its own turn boundary either.
2. Check the actual current state directly (git log on the relevant
   worktree/branch, PR/CI status via `gh`, a sub-agent's actual reported
   result or written report file) rather than assuming work is still in
   flight.
3. Keep working synchronously within one continuous turn: dispatch a task,
   wait for and read its real result, review it, run the fix loop if needed,
   move to the next task — ending the turn only on genuine forward progress
   (a task's review passed, a PR opened, a real blocker found), never merely
   to "wait."

This applies to any deeply-nested autonomous delivery pipeline in this
project (a top-level `Agent` dispatch that itself runs
`writing-plans` -> `subagent-driven-development`), not just any one specific
PR.
