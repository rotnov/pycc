---
name: autopilot-async-monitoring
description: Use when driving the pycc autonomous-delivery autopilot loop (writing-plans -> subagent-driven-development chains, PR/CI monitoring, background Agent orchestration) and deciding how to wait on async state such as a pull request, a CI run, or any dispatched `Agent` -- a one-off spike/build/benchmark task just as much as a nested sub-dispatch inside a pipeline. Covers checking real state before waiting, monitoring only currently-active work, and never ending a turn to "wait for a notification" while a dispatched agent, or a background child it started, is still live.
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

## Use `scripts/ci-watch.sh` + `Monitor` instead of a fixed `ScheduleWakeup` interval

When waiting on one or more open pull requests to reach a terminal CI state,
do not fall back to a periodic `ScheduleWakeup` (e.g. every 20-30 minutes) as
the default mechanism — a fixed wakeup interval means a real event (a
conflict, a stale/behind branch, a failed check, or a fully green PR ready to
merge) can sit unreported for most of that interval, which is exactly the
"minutes into hours" dead time this skill exists to eliminate.

Instead, run `scripts/ci-watch.sh <repo> <pr-number> [<pr-number> ...]` via
the `Monitor` tool (`persistent: false`, a generous `timeout_ms` — the script
exits on its own once every listed PR reaches a terminal state, so the
timeout is just a backstop). The script polls every `POLL_INTERVAL` seconds
(default 10, overridable via env) and prints exactly one line per PR the
moment it becomes: `MERGED`/`CLOSED`, `CONFLICTS` (merge base diverged),
`STALE` (branch fell behind base — e.g. a sibling PR merged first), `CHECK
FAILED -- <name> (<conclusion>)[, <name> (<conclusion>)...]` (every
non-passing check is named, not just the first), `BLOCKED` (every check
completed with none failing, but `mergeStateStatus` is something other than
`CLEAN`/`BEHIND` — typically an unresolved required review or conversation
thread under branch protection), or `READY` (every check green and
`mergeStateStatus: CLEAN`). It is silent between polls — no per-poll spam,
only real terminal events reach the conversation as `Monitor` notifications.

When every listed failing check in a `CHECK FAILED` line is `CANCELLED`
(no genuine `FAILURE`/`TIMED_OUT`/`STARTUP_FAILURE` among them), the script
appends a hint that this is often a partial-rerun or GitHub Actions infra
artifact rather than a code defect — see `issue-implement`'s "Attribute CI
failures before reacting" step for how to act on it (a full, non-`--failed`
rerun of every affected top-level workflow run, not a diff investigation).

This composes with the "check real state before waiting" rule above: the
script *is* that state check, run in a loop instead of once, so the terminal
event surfaces itself instead of needing a manual re-check every wakeup.
After the script reports `STALE` (a common case when multiple PRs from the
same session are queued and one merges before another), update the affected
branch (`git fetch origin main && git merge origin/main` or rebase) and
re-arm a fresh `Monitor` call for the remaining PR(s) — the script does not
retry a resolved PR itself, by design, so it terminates cleanly rather than
looping forever on a branch update it cannot perform itself.

If `scripts/ci-watch.sh` is not applicable (e.g. watching something that
isn't a GitHub PR), the same pattern — a poll loop that emits one line per
terminal state and exits once every tracked item resolves, run via `Monitor`
— still beats a fixed wakeup interval; write an equivalent small script
rather than reverting to periodic polling.

## Identify the session in every PR body

When opening a PR (`gh pr create`) from an autonomous agent session, add a
footer line identifying the session and client, e.g.:
`Session: claude-code <$CLAUDE_CODE_SESSION_ID>` (Claude Code exposes the
running session's ID via that env var — check for the Codex-equivalent
identifier when running under Codex instead of assuming the same var name
applies). A PR opened by a background-dispatched agent (e.g. from a
`writing-plans` -> `subagent-driven-development` chain) is otherwise easy to
lose track of — this makes it traceable back to the exact session/transcript
that produced it, which matters most for exactly that background-dispatch
case (see PR #328, which the orchestrating session genuinely forgot about
for a while).

## Serialize PRs under strict branch protection — don't open several at once

This repo's branch protection is `strict` (a PR must be up to date with
`main` before it merges). Under `strict` protection, opening multiple PRs
in parallel within one session — even for genuinely independent changes —
creates a real race: merging any one of them immediately makes every other
open PR `STALE`/`BEHIND`, which then needs its own fetch, merge, re-test,
push, and CI re-wait. With several PRs open at once this can cascade
(a catch-up push can itself go stale again before it lands), and it
happened repeatedly in one session here (#320/#322/#324/#326/#327).

Default to serializing instead: fully land one PR (open → CI green → merge)
before opening the next. This costs some session wall-clock time waiting on
one PR's CI before starting the next, but it eliminates the stale-branch
chase entirely, since no two merges are ever racing. Only batch multiple
PRs loosely when they are genuinely tiny, docs/overlay-only, and unlikely to
touch overlapping lines — and even then, expect at least one `STALE`
catch-up round per PR that lands ahead of the others, not zero.

**Draft-then-ready queuing** lets you prepare more than one PR's worth of
work concurrently without violating this rule: open every independent
change as a **draft** PR to reserve the work, but mark only **one** PR
"Ready for review" at a time. Land that one PR fully (CI green -> merge),
then take the next queued draft, rebase it onto the new `main`, mark it
ready, and only then let its CI run. This preserves the "no two merges ever
race" guarantee while avoiding a strict end-to-end serialization of the
underlying work. Verify per-repo before relying on the CI-cost angle of this:
opening a PR as draft only blocks the merge button by default — it does NOT
skip CI unless the repo's own workflows explicitly gate a job on
`github.event.pull_request.draft == false` (this project's
`.github/workflows/*.yml` carry no such guard as of 2026-08-04, so a draft
PR here still runs the full check suite; adding that guard would itself be a
CI-workflow change subject to this project's D-024/D-125 review rules).

## Monitor only active work

Do not keep checking on pull requests, branches, or tasks that are no longer
part of the current active work (e.g. a PR from an earlier, already-merged
or superseded milestone). Scope monitoring to what the current task genuinely
depends on.

## Any session that dispatches an `Agent` must not stop-and-wait on it — including a plain first-level dispatch, not only a nested sub-dispatch

This applies just as much to a top-level session's own first `Agent`
dispatch as to a dispatched orchestrator's nested sub-dispatch — being "the
outermost session" or "only one hop deep" changes nothing about the
mechanics below. When an agent (a top-level session running a single
spike/build/benchmark task, or one running a multi-task pipeline like
`subagent-driven-development`) dispatches an `Agent` and then ends its turn
with reasoning like "I'll wait for the monitor's notification rather than
poll further" — this does not work, regardless of dispatch depth or the
`run_in_background` flag passed to the dispatch itself.

The actual mechanism: a task-notification to the parent fires only when the
dispatched agent stops **with no live background children of its own** (a
backgrounded shell command, a running container, a `Monitor`-watched process
it started). If that agent ends its turn while such a child is still
running, the notification is not skipped outright — it is deferred until
that suppression condition later clears, which can take arbitrarily long and
does not run on any fixed schedule the parent can predict. The
`run_in_background` flag on the dispatch itself does not change this: it
governs how the *parent* is notified about the *dispatch*, not whether the
*dispatched agent's own* background children suppress that notification. A
returned `Agent` result whose text claims work is still continuing and
promises to notify later is a stopped agent making a promise the platform
does not keep on its behalf — treat it exactly like any other stopped agent:
verify the real state directly, or resume it, never end your own turn to
"wait for the notification."

Observed cost: a PR-13 delivery pipeline opened its PR at ~01:52 UTC and
merged at ~05:16 UTC — a ~3.5 hour gap dominated by an orchestrator
repeatedly stopping to "wait" rather than by real CI/build time (which was
closer to 15-20 minutes total across all the runs involved).

**The fix, applied when resuming a stalled dispatch:**

1. Never assume a dispatched agent will "notify" you the moment its own work
   concludes — if it ends its turn while it still has a live background
   child of its own, the notification is deferred until that child's state
   is next resolved, not guaranteed on any interval you control.
2. Check the actual current state directly (git log on the relevant
   worktree/branch, PR/CI status via `gh`, a sub-agent's actual reported
   result or written report file, the real state of anything it started in
   the background) rather than assuming work is still in flight.
3. Keep working synchronously within one continuous turn: dispatch a task,
   wait for and read its real result, review it, run the fix loop if needed,
   move to the next task — ending the turn only on genuine forward progress
   (a task's review passed, a PR opened, a real blocker found), never merely
   to "wait."
4. When dispatching an agent whose task may itself involve a long-running
   background process (a build, a container run, a watched job), say so
   explicitly in the dispatch brief and instruct it not to end its own turn
   while that child is still live — either block on it synchronously within
   its own turn, or keep polling it itself before stopping, so its returned
   result always reflects real, current state rather than a promise to
   follow up later.

This applies to any deeply-nested autonomous delivery pipeline in this
project (a top-level `Agent` dispatch that itself runs
`writing-plans` -> `subagent-driven-development`) and to a plain,
non-nested, single-task `Agent` dispatch alike — not just one specific PR.
