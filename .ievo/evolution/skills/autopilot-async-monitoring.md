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

## 2026-08-04 06:49 UTC — Under strict (up-to-date) branch protection, don't open multiple PRs in one session — they chase each other stale
**Trigger:** user-observed mistake ("если стоит бранч ап ту дейт протекшен, не сиысла в одной сесси открывать несколько ПР, начинается гонка")

если стоит бранч ап ту дейт протекшен, не сиысла в одной сесси открывать несколько ПР, начинается гонка

Context: this repo's branch protection is `strict: true` (a PR's branch must
be up to date with `main` before merge — see `docs/REPOSITORY_GOVERNANCE.md`).
Across roughly one hour this session, 4-5 PRs were opened in parallel
(#322, #324, #326, #327, plus an earlier #320) for genuinely independent,
small changes. Every single merge of one PR immediately made every *other*
still-open PR `STALE` (`mergeStateStatus: BEHIND`), which then needed its
own `git fetch && git merge origin/main`, a re-run of local tests, a push,
and a fresh CI wait — repeatedly, for every PR still open at the time.
This is a real race: with N PRs open under strict protection, merging any
one of them can stale up to N-1 others, and if those others are *also*
racing to merge, the staleness can cascade multiple times per PR before it
actually lands. The `ci-watch.sh`/`Monitor` tooling from this same skill
made the staleness events cheap to *detect* the moment they happened, but
did nothing to prevent the underlying churn (re-fetch, re-merge, re-test,
re-push, re-wait) each event still costs.

**Rule:** under `strict` (up-to-date-required) branch protection, do not
open multiple PRs in parallel within one session merely because the
underlying changes are independent. Serialize instead: fully land one PR
(open → CI green → merge) before opening the next. This trades a small
amount of session wall-clock (waiting for one PR's CI before starting the
next) for eliminating the stale-branch chase entirely — zero merges happen
concurrently, so no open PR can ever be staled by another one finishing
first. The one exception worth keeping: genuinely tiny, no-code-conflict
docs/overlay-only changes that are extremely unlikely to touch the same
lines as anything else in flight can still be batched loosely, but even
then expect at least one round of `STALE` catch-up per merge that lands
ahead of them — budget for it rather than being surprised by it.

## 2026-08-04 06:53 UTC — Record the session ID and client (Claude/Codex) in every PR body opened by an agent
**Trigger:** user-observed mistake ("при открытии пр указывать в теле ПР ид сессии и агента (клод, кодекс) что бы можно былл идентифицировать сессию и найти ее")

при открытии пр указывать в теле ПР ид сессии и агента (клод, кодекс) что бы можно былл идентифицировать сессию и найти ее

Context: PR #328 (the final v0.2 PR-14) was opened by a background-dispatched
agent, and the orchestrating session (this one) genuinely lost track of it
for a while amid handling a separate CI-noise investigation — it only
resurfaced when the user asked to check for forgotten open PRs. Nothing in
the PR body itself said which session or which agent (Claude Code vs.
Codex, and which invocation) had opened it, so there was no way to look it
up directly from the PR — only indirect reconstruction from git log/commit
messages.

On Claude Code, the session identifier is available as the
`CLAUDE_CODE_SESSION_ID` environment variable (confirmed present this
session: `791dd9a8-bca2-44f1-b88d-07a97612648b`, also visible in scratchpad
paths like `/private/tmp/claude-501/.../<session-id>/scratchpad`).

**Rule:** when opening a PR (via `gh pr create`) from an autonomous agent
session in this project, include a line identifying the session/agent in
the PR body — e.g. a footer line like `Session: claude-code <CLAUDE_CODE_SESSION_ID>`
(or the Codex-equivalent identifier when running under Codex, if one is
exposed the same way — check for it rather than assuming Claude Code's env
var name applies there too). This makes a PR traceable back to the exact
session/transcript that produced it, which matters specifically for a
background-dispatched or otherwise easy-to-lose-track-of PR like #328 was
here — the alternative (reconstructing which session opened what from
commit messages and timing alone) is exactly the gap that let #328 go
unnoticed.
