# Incident: ad-hoc-ci-polling-instead-of-skill — recurrence entry 2 (deny rung + watcher repair)

**Date:** 2026-08-29
**Topic:** ad-hoc-ci-polling-instead-of-skill
**Session:** claude-code 07f47146-acf2-472f-8d5e-e9bde0b2201e
**Verdict:** shipped (manual verify)
**Related:** `incident.md` in this topic (parallel delivery, same user complaint, PR #829); machine-local topic `background-child-stop-and-wait` (2026-08-09, notification-suppression sibling)

## Symptom

Same user complaint as `incident.md` in this topic, received by this session
via `/harden` verbatim: "висишь уже 13 часов, в то время как CI упал не
используешь скилл для ci watch pr", clarified mid-turn as "не используешь
готовый инструмент ... пишешь велосипед" (not using the ready-made tool;
reinventing the wheel). This session's concrete failure instance: a
hand-rolled `while true; do gh pr checks ...; sleep 30; done` loop in a
background Bash call (with a `timeout` above the tool's documented maximum)
died silently — output file empty, no notification — while the delivery
PR's CI sat red for ~13 hours.

## Relationship to this topic's first entry

Two parallel sessions ran `/harden` on the same complaint. The first
delivered PR #829: an advisory `PreToolUse` nudge
(`.claude/hooks/ci_watch_nudge.py`, machine-local wiring) that reminds on
any one-shot `gh pr view|checks|list` call, plus this topic's `incident.md`.
This entry is the second, complementary delivery and covers what the nudge
does not:

- the *blocking* rung for the pathological shape itself (a poll loop, not a
  one-off status check), and
- the **content** gap that motivated bypassing the watcher in the first
  place — a real false-terminal defect in `ci-watch.sh`.

## Gap type

**compliance** (as in `incident.md`: the mandating skill was loaded in this
very session's context and was still bypassed on the transport dimension)
**plus content**: `ci-watch.sh` treated an empty `statusCheckRollup` —
GitHub Actions not started yet, or a momentary gap between chained
workflows — as "all checks completed", emitting a false `BLOCKED`/`READY`
and exiting. The watcher looking untrustworthy is what made the hand-rolled
substitute feel justified; fixing only compliance would leave the incentive
to bypass in place.

## Artefacts

1. **Deny hook (machine-local, this machine only):**
   `~/.claude/hooks/deny-handrolled-ci-poll.py`, wired as `PreToolUse`
   (matcher `Bash`) in `~/.claude/settings.json`. Denies (exit 2, stderr
   redirect message — the blocking path is the one whose stderr Claude Code
   feeds back to the model, per this topic's first entry) any Bash command
   combining a `gh` CI query (`gh pr checks|view`,
   `gh run/api ...watch|runs|check-runs|status`) with a `while`/`until`
   loop and a `sleep`; anything invoking `ci-watch.sh` is allowlisted.
   Deliberately not repo-committed: D-023/D-025 make shared hook wiring a
   registered-contract affair with a security review — the repo-side fix
   that travels with the project is the watcher repair below. Complements
   PR #829's nudge: the nudge covers one-shot calls advisorily; this blocks
   the loop shape outright.
2. **Watcher repair (this commit, repo-side, platform-neutral):**
   `.claude/skills/gha-watch-ci-pr/scripts/ci-watch.sh` no longer treats an
   empty `statusCheckRollup` as terminal (after `EMPTY_NOTE_POLLS`
   consecutive empty polls, default 30, it emits one non-terminal NOTE and
   keeps polling), and the `READY`/`BLOCKED` verdicts require the same
   qualifying observation on two consecutive polls, so a momentary
   all-complete gap between chained workflows cannot resolve the watch
   early. `CHECK FAILED`, `MERGED`/`CLOSED`, `CONFLICTS`, and `STALE` stay
   immediate — those states do not regress on their own.

Discovery ran before building: external skills covering this class
(wait-for-ci / watch-ci variants) were found and rejected — the class is
already solved in-repo by `gha-watch-ci-pr`; the right move is to repair
the owned tool, not adopt a parallel one.

## Fixture

The watcher repair's fixture is the skill's own CI-run regression harness,
`.claude/skills/gha-watch-ci-pr/scripts/test-ci-watch.sh` (executed by the
`agent-assets` workflow), extended in this commit with the incident's
reproduction cases: empty-rollup-never-terminal, the between-workflow gap,
the one-time NOTE, and the two-consecutive-poll confirmation (the existing
pending-then-green fixture's expected poll count moves 2 → 3 accordingly).
The hook's fixture is the violator/clean payload set recorded under
`fixture/` beside this entry. Not applicable as an arena fixture — both
artefacts are deterministic binary gates, not agent-behaviour artefacts the
arena's model-vs-model comparison scores (same carve-out as this topic's
first entry).

## Verify

`verify: manual`, both artefacts, both directions:

- **Hook:** 2 violator payloads (including the exact incident loop) →
  exit 2 with the redirect message; 5 clean payloads (one-shot
  `gh pr checks`, the sanctioned `ci-watch.sh` invocation, a log-file
  `until` loop, a non-Bash tool call, malformed JSON) → exit 0. Wiring
  proven end-to-end live: the first in-session Bash smoke test after
  installation was itself intercepted and denied by the hook.
- **Watcher:** stub-`gh` sequences through `test-ci-watch.sh`
  (see Fixture). Pre-fix regression demo: the shipped script emitted a
  false `BLOCKED` on poll 1 against an empty rollup; the fixed script
  rides the same sequence through to `READY`. The defect also fired live
  in this session: a watch on the delivery PR reported `BLOCKED` whose
  real cause was one unresolved review thread — caught by the mandated
  one-shot verification this fix retires.

## Sweep result

Searched the repository for other hand-rolled CI poll loops
(`while`/`until` + `gh pr`/`gh run` + `sleep` across skills, scripts, and
workflows): the only in-repo poll loop over PR checks is `ci-watch.sh`
itself — the sanctioned one. No other artefact re-implements it; the
`autopilot-async-monitoring` skill already mandates the watcher.

## Diff

- `.claude/skills/gha-watch-ci-pr/scripts/ci-watch.sh` — false-terminal fix
  (empty rollup never terminal + NOTE; 2-consecutive-poll confirmation for
  READY/BLOCKED)
- `.claude/skills/gha-watch-ci-pr/scripts/test-ci-watch.sh` — fixture 4
  poll count updated; incident fixtures added
- `.claude/skills/gha-watch-ci-pr/SKILL.md` and
  `.claude/skills/autopilot-async-monitoring/SKILL.md` — the two lines
  describing BLOCKED/READY semantics updated (confirmation + NOTE)
- this entry + `fixture/` (hook payloads)
- `docs/AGENT_RETROSPECTIVE.md` — transport-substitution entry (and a
  second, unrelated-to-this-topic entry on gate-inventory completeness)
- `~/.claude/hooks/deny-handrolled-ci-poll.py` + `~/.claude/settings.json`
  wiring (machine-local, not part of the commit)
