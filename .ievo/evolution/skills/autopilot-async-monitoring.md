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

## 2026-08-04 06:20 UTC — Always pass `--head`/`-B` explicitly to `gh pr create`; never rely on the shell's current directory
**Trigger:** user-observed mistake ("какие то конфликты") while opening the nbody RUNS=7 fix's PR

Context: `gh pr create` was run from the main session's own cwd
(`.claude/worktrees/project-overview-53ef3d`, the *orchestrating* session's
workspace), not from `.worktrees/nbody-median`, the worktree that actually
had the intended commit checked out. `gh pr create` silently used whatever
branch happened to be checked out in the cwd it ran from
(`fix/d084-median-throughput`, a stale, already-merged branch from an
earlier fix) instead of erroring — it produced a real, live PR (#323)
against the wrong head with no warning, which then showed
`mergeStateStatus: DIRTY` / `mergeable: CONFLICTING` purely because the
wrong branch was compared to `main`. The user's first signal was just
"какие то конфликты" (some conflicts) on a change that should have been a
trivial two-file diff — the actual bug (wrong branch entirely) only
surfaced after checking `gh pr view --json headRefOid,headRefName` and
finding it didn't match the branch that was actually pushed.

**Rule:** when running `gh pr create` (or any `gh`/`git` command whose
behavior depends on "the currently checked out branch") from a session
that manages multiple git worktrees, never rely on the shell's cwd having
the right branch checked out — always pass the target branch explicitly:
`gh pr create --head <branch> --base <base>` (and `cd` into the correct
worktree first regardless, as defense in depth — the explicit flags are
what actually prevent the silent-wrong-branch failure, not the `cd` alone,
since a forgotten `cd` after several worktree operations is exactly how
this happened). After creating a PR, verify `gh pr view --json
headRefName,headRefOid` matches the branch and commit actually intended
before treating the PR as real — the same "check real state before
waiting" discipline this skill already covers for CI/PR status, applied to
verifying a just-created artifact rather than an in-flight one. If a PR
does turn out to be against the wrong head, closing it and re-opening
correctly (rather than trying to retarget the existing PR's head, which
`gh`/GitHub does not support) is the fastest fix.

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

## 2026-08-04 06:26 UTC — A useful ad-hoc script must be committed into the skill, not left in the session scratchpad
**Trigger:** user-observed mistake ("не усилил скилл а положил себе локально")

не усилил скилл а положил себе локально

Context: wrote a genuinely useful CI-polling script (`ci-watch.sh`, poll a
PR every N seconds and emit exactly one line the moment it hits a terminal
state: conflicts, stale/behind base, a failed check, or fully green) and
ran it successfully via `Monitor` — but only saved it to the session's own
scratchpad directory (`/private/tmp/claude-.../scratchpad/`), never
committed to the repository or referenced from this skill. The scratchpad
is session-specific and gets discarded, so the very tool that would have
prevented the "minutes into hours" idle-waiting problem this skill exists
to solve would have been lost the moment the session ended, and the next
session would have had to reinvent it from scratch or fall back to the
`ScheduleWakeup`-interval pattern this skill already tries to discourage.

**Rule:** when a script, helper, or technique developed ad hoc during a
session turns out to be a real, reusable improvement to how this project's
autopilot loop operates — not a one-off debugging aid for the specific bug
at hand — commit it into the repository (e.g. `scripts/`) with a matching
local test (`scripts/test-*.sh`/`.py`/`.rb`, matching this repo's existing
per-script test convention) and update the owning skill to reference and
prescribe it, in the same session that developed it. Do not consider the
work "done" once it merely works once from the scratchpad; it is done once
a future session (with no memory of this one) can discover and use it via
the skill alone.

## 2026-08-04 06:40 UTC — `Monitor` doesn't inherit the session's cwd; `cd` into the right worktree first, don't hardcode an absolute path
**Trigger:** user-observed mistake ("почему не затригерился на ошибочный путь?", "а чего абсолютный?", "скилл то в репе")

Context: right after `scripts/ci-watch.sh` was committed into the repo, it
was invoked via `Monitor` as `sh scripts/ci-watch.sh rotnov/pycc 324` — a
path relative to a presumed repo root — and failed immediately with exit
127 ("command not found"). Root cause: `Monitor` runs its command in its
own shell, whose current working directory is not guaranteed to match the
calling session's cwd or any particular worktree. The relative path simply
didn't resolve to a file there, so the shell couldn't even exec the
script.

The first fix tried was hardcoding an absolute path
(`/Users/.../scripts/ci-watch.sh`), which worked but was the wrong lesson
to generalize from: the script is **committed to the repo**, at the same
relative path (`scripts/ci-watch.sh`) in every worktree, since every
worktree shares the same tracked tree. An absolute path only happens to
work for one specific worktree on one specific machine — it breaks the
moment that worktree is removed/renamed or the same script needs to run
against a different worktree, and it's not portable to another machine at
all. The real fix is to control the *working directory*, not to bypass it
with an absolute path.

**Rule:** when invoking a repo-committed script via `Monitor` (or any
background dispatch whose cwd isn't controlled), prefix the command with
an explicit `cd` into the correct worktree root, then use the normal
repo-relative path: `cd <worktree-root> && sh scripts/ci-watch.sh ...`.
Only reach for an absolute path when the target genuinely isn't part of
the repo's own tracked tree (e.g. a session-scratchpad file) — for
anything committed, `cd` + relative path is both correct and portable
across worktrees. This is the same "control the invocation, don't route
around it" instinct as `gh pr create --head`/`-B` from this skill's own
earlier lesson: fix the actual cwd assumption rather than hardcoding
around its symptom.
