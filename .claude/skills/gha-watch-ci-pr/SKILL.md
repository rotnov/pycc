---
name: gha-watch-ci-pr
description: Use when driving the pycc autonomous-delivery autopilot loop and deciding how to wait on async state — a pull request, a CI run, or any dispatched `Agent`. Provides `ci-watch.sh` for PR/CI polling and rules for serialization, session identification, and dispatched-agent lifecycles.
---

# Autopilot async monitoring

Procedural rules and tools for waiting on asynchronous state during the
project's autonomous PR-by-PR delivery loop: pull requests, CI runs, and
dispatched background agents.

## Tools

### `scripts/ci-watch.sh` — poll PRs until terminal state

Run via `Monitor` (`persistent: false`, generous `timeout_ms` — the script
exits on its own):

```
scripts/ci-watch.sh <repo> <pr-number> [<pr-number> ...]
```

Polls every `$POLL_INTERVAL` seconds (default 10). Silent between polls.
Emits exactly one line per PR when it reaches a terminal state:
`MERGED`/`CLOSED`, `CONFLICTS`, `STALE` (branch behind base),
`CHECK FAILED -- <name> (<conclusion>)[, ...]` (every non-passing check
named), `BLOCKED` (all checks completed with no failures but not CLEAN —
typically an unresolved required review), or `READY` (all green + CLEAN).
`READY` and `BLOCKED` are only reported after the same verdict holds on two
consecutive polls, so a momentary all-complete gap between chained
workflows cannot resolve the watch early; an empty `statusCheckRollup`
(Actions not started yet) is never terminal — after `$EMPTY_NOTE_POLLS`
consecutive empty polls (default 30) the script emits one non-terminal
`NOTE` line (once per consecutive-empty streak, so it can recur after a
later non-empty poll) and keeps watching.

When every failing check is `CANCELLED` (no genuine `FAILURE`/`TIMED_OUT`
among them), the line adds a hint that this is often a partial-rerun or
GitHub Actions infra artifact — see `issue-implement`'s "Attribute CI
failures before reacting" step for how to act on it.

After `STALE`, update the branch and re-arm a fresh `Monitor` call for the
remaining PR(s) — the script does not retry a resolved PR itself.

If `ci-watch.sh` is not applicable (e.g. watching something that isn't a
GitHub PR), write an equivalent poll-loop script that exits once every
tracked item resolves, rather than reverting to a fixed `ScheduleWakeup`.

## Rules

### Check real state before waiting

Query actual current state directly — don't assume a prior plan is still
accurate. For pull requests: check `mergeStateStatus`/`mergeable` and the
check list (`gh pr checks`) before waiting on CI. For dispatched agents:
look at their actual reported result, branch/worktree git log, or written
artifacts — never assume "still running" without evidence.

### Identify the session in every PR body

Add a footer line identifying the session and client, e.g.:
`Session: claude-code <$CLAUDE_CODE_SESSION_ID>`. Makes background-dispatched
PRs traceable back to the session that produced them.

### Serialize PRs under strict branch protection

Under `strict` protection, opening multiple PRs in parallel creates a race:
merging any one immediately makes every other open PR `STALE`/`BEHIND`.
Default to serializing: fully land one PR before opening the next.

**Draft-then-ready queuing** lets you prepare more than one PR's worth of
work concurrently: open every independent change as a **draft** PR, mark
only **one** "Ready for review" at a time. Land it fully, then rebase the
next queued draft onto the new `main`, mark it ready, and let its CI run.

### Monitor only active work

Scope monitoring to what the current task genuinely depends on. Don't keep
checking on PRs, branches, or tasks from earlier, already-merged or
superseded milestones.

### Don't stop-and-wait on dispatched agents

A dispatched agent's notification fires only when it stops **with no live
background children of its own**. If it ends its turn while a background
shell, container, or `Monitor`-watched process is still running, the
notification is deferred — potentially arbitrarily long.

Instead: check actual current state directly, or resume the agent. Never
end your own turn to "wait for the notification."
