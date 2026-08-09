# Session handoff: Cachegrind ir_ratio spike result, and a `/harden` fix for a stalled dispatch

**Checkpoint reason:** two independent pieces of work converge at a natural
stopping point — the Cachegrind feasibility spike for issue #416 finished and
its result is ready to report, and a `/harden` pass triggered by that spike's
own dispatch failure produced a shipped skill fix (PR #419). Recording both
before a fresh session picks up either #416's next phase or #419's merge.

## Baseline at this checkpoint

- `origin/main` tip: `46144c0545ece0159f23612cbdcbab2405a12b1d` (re-fetched
  immediately before writing this entry).
- Working tree: on task branch `harden/background-child-stop-and-wait`, ahead
  of `origin/main` by 2 commits (both pushed), nothing uncommitted except the
  untracked `.harden/` incident record and `arena-runs/` scratch output
  (neither tracked by git in this clone — see below).

## What happened this session

### 1. Cachegrind spike (issue #416, Approach B feasibility)

Per a standing directive to test gating on `ir_ratio` (Cachegrind instruction
count) instead of `wall_ratio` (wall-clock) for the nbody CI-noise problem,
ran a local spike matching #416's own "Cachegrind spike" plan item: built the
release nbody pycc binary and CPython 3.14.6 inside a Docker container
matching CI's Linux/x86_64 build recipe, then ran Cachegrind 10 times against
the pycc binary.

**Result: 10/10 runs produced a bit-identical `I refs` count —
`1,368,500,966`** — with no determinism controls applied beyond what Docker
gives by default (no explicit ASLR disable, no `PYTHONHASHSEED` pin; the
spike measured only the already-built pycc binary, not a fresh
pycc-vs-CPython `ir_ratio` computation end to end). This is strong evidence
for the design spec's core hypothesis (§4.1: instruction counting sidesteps
the hypervisor/frequency-scaling noise that made the CPU-time experiment
fail) — but it is one local measurement, not the CI-mined distribution
§5's decision checkpoint actually requires.

**Reconciled against the already-approved plan, not substituted for it:**
`docs/superpowers/specs/2026-08-08-nbody-ci-noise-evidence-gathering-design.md`
(landed via #415) explicitly scopes gating on `ir_ratio` as out of scope for
the lifetime of this design (§4.3, §8) — Approach B ships as a non-gating,
`#[ignore]`d diagnostic test, and a future gate change requires its own
separately-evidenced decision via §5 outcome 2 (a *mined*, real-CI `ir_ratio`
distribution clearing the same margin-below-worst-failure method used for
`wall_ratio`, not a single spike). The spike result was reported to the user
with this tension made explicit, recommending the design proceed as approved
(Approach A + non-gating Approach B) rather than reopening the already-merged
spec's scope on the strength of one local run.

Raw evidence copied off the (now-stopped) spike container to
`/private/tmp/claude-501/.../scratchpad/cachegrind-spike-results/` and
`cachegrind-spike-full-log.txt` — session-scratch, not committed anywhere;
a future session repeating this spike should re-run it rather than trust
these files to still exist.

### 2. `/harden`: dispatched-agent stop-and-wait fix (PR #419)

The dispatched spike agent itself (`Agent`, `run_in_background: false`)
stalled: it ended its turn twice claiming it would "wait for the monitor's
notification," while its own Docker container was still running, and
produced no self-initiated report for 42+ minutes — even after the container
had already failed. Discovered only because the user asked for a status
update.

Ran this through the `/harden` skill:

- Traced to `.claude/skills/autopilot-async-monitoring/SKILL.md`'s existing
  "don't stop-and-wait" rule, which was scoped to "dispatched orchestrators"
  running nested "sub-dispatches" — read literally, a plain first-level
  `Agent` call falls outside that scope.
- The tracer's first-pass fix text additionally mischaracterized the
  mechanism, claiming `run_in_background: false` "carries no
  parent-notification contract at all." This session's own evidence
  contradicts that (a notification did eventually arrive) — corrected before
  shipping: the parent notification is *deferred* while the dispatched agent
  has a live background child of its own (a backgrounded command, a running
  container, a `Monitor`-watched process), independent of the
  `run_in_background` flag on the dispatch itself.
- Applied the corrected edit, built a reproduction fixture at
  `.harden/incidents/background-child-stop-and-wait/fixture/`, and shipped
  the fix as [PR #419](https://github.com/rotnov/pycc/pull/419) — committed
  and pushed, CI in progress at this checkpoint (`mergeStateStatus: BLOCKED`
  pending the still-running checks, not a genuine conflict; a first push was
  red on `agent-assets` because the Codex forwarding stub's `description`
  frontmatter wasn't synced to the edited Claude copy — fixed in a follow-up
  commit on the same branch and re-verified locally with
  `scripts/validate_agent_assets.py`).
- The arena run for this fixture (3 harnesses × 2 conditions × 3 runs,
  launched via `Monitor` as task `b3amrenrs`) finished with **no baseline**:
  18/18 runs `PASS`, including every control run, meaning the fixture's
  12-second `check.sh` delay was not by itself enough to make any harness
  attempt the "stop and claim I'll wait" shortcut this incident is about.
  This is a fixture-quality gap, not evidence against the fix — the shipped
  change (PR #419) already stands on this session's own primary incident
  evidence (the real 42+-minute stall), and this project's `.harden/`
  apparatus is a local, not-yet-adopted trial in this clone (see below), so
  PR #419 was not held open waiting on the arena, and does not need to be
  reopened now that a non-`profit` result landed.

**`.harden/` stays local, deliberately.** This clone's `.git/info/exclude`
excludes `.harden/`, `.claude/skills/harden`, and `.agents/skills/harden`
(comment: "local: harden symlinked from ideas-research for testing") — the
harden skill itself is a personal trial via a symlink into `ideas-research`,
not an adopted project convention. The incident journal entry at
`.harden/incidents/background-child-stop-and-wait/2026-08-09-fdab0dac.md`
therefore stays untracked scratch rather than being force-committed against
that deliberate local exclude; only the genuinely tracked artefact
(`.claude/skills/autopilot-async-monitoring/SKILL.md`) shipped via PR #419.
Adopting harden as a real project convention (committing `.harden/`,
removing the exclude) would be its own separate, reviewed decision.

## Current state of related issues/PRs (re-verified at this checkpoint)

- **Issue #416** (open): unchanged since
  [2026-08-09-01](2026-08-09-01-issue-416-plan-published.md) — plan
  published, Phase 1 not started. This session added spike evidence
  supporting the plan's existing Approach B, but did not implement any phase.
- **PR #419** (open, this session's own): `harden/background-child-stop-and-wait`
  → `main`, CI in progress at this checkpoint. Merge once green; no known
  blockers beyond normal CI completion.
- **PR #391** (`docs/session-log-2026-08-07-01`, open): still unmerged,
  now stale — flagged again, not touched this session.
- **PR #418** (`harden/paused-autopilot-state`, open): unrelated
  session-log/autopilot-state PR, no file overlap with this session's work.

## Where a fresh session should resume

1. Monitor PR #419 to green and merge it (standard D-078 flow;
   `scripts/ci-watch.sh` + `Monitor`, per the very rule this PR fixes).
2. If the arena run (task `b3amrenrs`, or its `arena-runs/` output directory
   if still present locally) produced a verdict, record it in the local
   incident journal entry; if `profit`, nothing else is needed since the fix
   already shipped. If `zero`/`harm`, that reopens the artefact-type question
   for this rule specifically (escalate a rung, per the harden skill) — but
   does not by itself invalidate PR #419, which stands on its own incident
   evidence.
3. #416's next unit of work is still Phase 1 of the published plan (the
   raw-fd-bypass retrieval mechanism in `tests/nbody_bench.rs`) — see the
   prior checkpoint's own "do not let a plain `issue-implement #416` close
   the issue on Phase 1 merge" caution, which still applies unchanged.
4. `arena-runs/` is untracked and uncovered by any `.gitignore` rule as of
   this writing — decide whether to add one before it accumulates further,
   or leave it as session-local scratch consistent with `.harden/`'s own
   local-only status.
