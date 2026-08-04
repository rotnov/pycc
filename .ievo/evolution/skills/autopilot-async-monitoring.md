---
target: skill
target_name: autopilot-async-monitoring
created: 2026-08-04T06:52:00Z
---

# autopilot-async-monitoring — Evolution Overlay

## 2026-08-04 06:52 UTC — Reading the rule is not enough; agents keep violating it anyway
**Trigger:** user-observed mistake during PR-14 autonomous delivery

агенты не послушные

Context: this skill was extracted specifically to stop dispatched orchestrator
agents from ending their turn to "wait for a notification" instead of working
synchronously. Immediately after extraction, a fresh agent dispatch was given
this skill and told explicitly to read and follow it first. It read the
skill, correctly summarized its own core rule back in its report, and then
in the very same turn dispatched yet another background agent and stopped —
violating the rule it had just stated. This happened even with an explicit
instruction to follow the skill, not just its general availability.

**Escalation that actually worked:** stop trusting a dispatched agent's claim
that it read/understood/will follow this skill. Instead:
1. Verify real state directly yourself (git log on the actual worktree, `gh
   pr list`) before believing any "I dispatched a background agent to handle
   it" report — that phrase is itself now a red flag for this exact
   violation.
2. When a violation is caught, do not just re-explain the rule — explicitly
   forbid the specific escape hatch it used ("do NOT dispatch a further
   background/async agent for this work") and change the dispatch mechanism
   itself: use a **foreground, synchronous** dispatch (the caller blocks on
   the agent's direct return, e.g. `run_in_background: false`) rather than a
   background one with a notification callback. A foreground dispatch cannot
   "stop and wait for a notification" in the same failure mode, because
   there is no notification path available to it — the calling session is
   already blocked on its return.
3. This worked in practice: after switching to a foreground/blocking
   dispatch, the same task produced real, verifiable committed progress
   (actual git commits) in one continuous run, instead of another empty
   "I'll wait" report.

**Generalized rule:** telling an agent to follow this skill is necessary but
not sufficient. When a dispatched agent has already demonstrated the
stop-and-wait violation once on a given task, escalate to a foreground/
blocking dispatch for the retry rather than repeating the same background
dispatch with a stronger-worded instruction — the instruction alone did not
hold on the first retry.

## 2026-08-04 06:56 UTC — Start investigating a failed check as soon as it fails, don't wait for the rest of the suite
**Trigger:** user-observed mistake during PR #322 CI monitoring

один чек на CI уже упал, можно уже начинать разбираться а не ждать все остальные

Context: while a multi-job CI run (`gh pr checks`) had several jobs still
`pending`, one job (`agent-assets`) had already finished and failed. The
correct move was to start diagnosing that failure immediately — the other
jobs' outcomes don't change what's already known to be broken, and most CI
job durations here are dominated by long build/test steps (`build-test-coverage`,
`native-build-test` on several targets, `frontend-perf-measure`), so a
failure that finishes fast (10s here) can sit fully diagnosable for minutes
before the rest of the suite even reports in.

**Rule:** when checking `gh pr checks` (or any CI status view) and any job's
own state is already terminal (`fail`, not `pending`/`in_progress`), pull its
logs and start root-causing it right away, regardless of whether sibling
jobs are still running. Do not wait for "all checks" to resolve before
starting to look at ones that already did. This composes with this skill's
existing "check real state before waiting" rule (`gh pr checks`/`gh run
view --log-failed`) — the difference here is act on a partial, still-in-progress
result the moment it contains a decided outcome, rather than only
consulting real state once everything is done.
