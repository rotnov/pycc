# Incident: ad-hoc-ci-polling-instead-of-skill

**Date:** 2026-08-29
**Topic:** ad-hoc-ci-polling-instead-of-skill
**Verdict:** shipped (manual verify)

## Symptom

User (via `/harden`, verbatim): "висишь уже 13 часов, в то время как CI упал не
используешь скилл для ci watch pr" -- during a multi-round autopilot delivery
session, the agent watched pull-request/CI state with hand-rolled
`gh pr view` / `gh pr checks` calls and ad-hoc `Monitor` bash loops across an
entire session, instead of the project's own
`.claude/skills/gha-watch-ci-pr/scripts/ci-watch.sh`. A `Monitor` task was
lost across an apparent process restart and not re-armed, leaving a stretch
of the session with no active watch while a PR's CI had already failed.

## Root cause analysis

Path taken: **inline** (trivial path) -- a single obvious cause, no
multi-hop tracing needed. This is a **recurrence**, not a fresh lesson: a
prior session had already recorded the identical guidance in long-term
memory (`feedback_use_ci_watch_skill.md`: "use
`.claude/skills/gha-watch-ci-pr/scripts/ci-watch.sh` under `Monitor`") and
this session still defaulted to ad-hoc polling. A textual/memory-level rule
had already failed once; per the skill's own principle, rewording it again
would fail the same way a second time.

## Gap type

**compliance** -- a clear rule already existed (project skill +
long-term memory entry) and was not applied at the point of use. Text
cannot fix a compliance gap; it needs a mechanical rung.

## Termination point

Local harness hook (`hook`), not a project rule file -- `AGENTS.md` already
routes CI-watching to `gha-watch-ci-pr` implicitly via the skill's own
description, and duplicating that as prose would be the same artefact type
that already failed. Per D-023, Claude hook wiring is machine-local only
(gitignored `.claude/settings.local.json`), so this fix binds this
machine/session's future runs, not the repository.

## Artefact

New `PreToolUse` hook on `Bash`, wired from `.claude/settings.local.json`
(gitignored) to `.claude/hooks/ci_watch_nudge.py` (tracked, since the script
itself is inert without local wiring and contains no secrets). It pattern-matches
`gh pr (view|checks|list)` in the command string and writes a stderr
reminder pointing at `ci-watch.sh` run via `Monitor`. It does not block the
call -- a single one-off status check is legitimate; the reminder targets
the ad-hoc *polling loop* pattern the incident actually exhibited.

## Fixture

Not applicable as an arena fixture -- this is a harness hook intercepting a
literal tool call, not an agent-behavior artefact the arena's model-vs-model
comparison is built to score (arena runs are for `hook`/`review-check`/`rule`
artefacts that change what an agent *chooses* to do when read; this hook
instead fires mechanically on the tool call itself, independent of the
model's reasoning). Verified manually instead, per the "static gates are
proven differently" carve-out.

## Verify

`verify: manual` -- ran the hook directly against two payloads:

- violator: `{"tool_name":"Bash","tool_input":{"command":"gh pr checks 827 --repo rotnov/pycc"}}`
  -> emits the ci-watch.sh reminder to stderr, exit 0 (advisory, not
  blocking).
- clean: `{"tool_name":"Bash","tool_input":{"command":"cargo build --workspace"}}`
  -> silent, exit 0.

## Sweep result

N/A for a repo-content sweep -- the artefact is a local hook intercepting
future tool calls, not a diagnostic run against existing files. No
retroactive violations to enumerate.

## Diff

Added (new files, no existing rules removed or modified):

- `.claude/hooks/ci_watch_nudge.py` (tracked)
- `.claude/settings.local.json` (new, gitignored, not committed)
