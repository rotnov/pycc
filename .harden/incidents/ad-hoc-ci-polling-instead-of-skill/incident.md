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
`gh pr (view|checks|list)` in the command string and returns a reminder
pointing at `ci-watch.sh` run via `Monitor`. It does not block the
call -- a single one-off status check is legitimate; the reminder targets
the ad-hoc *polling loop* pattern the incident actually exhibited.

**Revision (same day, post-review):** the D-068 pinned reviewer (via a
GitHub-side automated pass on the delivering pull request) flagged two P2
gaps, both fixed in place rather than reworded:

1. The hook originally wrote the reminder to stderr on a successful (exit 0)
   run. Claude Code only feeds a hook's stderr back to the model on a
   *blocking* (non-zero exit) result -- a successful advisory hook's stderr
   is discarded, so the reminder never actually reached the agent. Fixed by
   emitting `{"hookSpecificOutput": {"hookEventName": "PreToolUse",
   "additionalContext": "..."}}` as stdout JSON instead, which Claude Code
   does surface to the model on every run regardless of exit code. Confirmed
   live: the fixed hook fired on the very next `gh pr checks` call made
   while fixing this incident, and its `additionalContext` appeared in the
   session as a `PreToolUse:Bash hook additional context` system reminder.
2. Per AGENTS.md's "Support Codex and Claude Code" section, a new agent/skill
   surface needs an equivalent Codex entrypoint or a documented fallback.
   Codex CLI has no per-tool-call hook event comparable to Claude Code's
   `PreToolUse` with context injection (its `.codex/hooks.json` surface, per
   `docs/AGENT_TOOLING.md`, is used for session-lifecycle/notification hooks
   such as iEvo's, not per-command advisory injection) -- there is no
   equivalent capability to add. The documented fallback is the project
   skill itself: `.claude/skills/gha-watch-ci-pr/SKILL.md` already has a thin
   `.agents/skills/gha-watch-ci-pr/SKILL.md` Codex wrapper (this repo's
   standard cross-platform pattern, see `docs/AGENT_TOOLING.md`), so a Codex
   session reads the same `ci-watch.sh` guidance this hook enforces
   mechanically for Claude Code -- textual there, mechanical here, per
   AGENTS.md's explicit "provide the equivalent Codex capability or a safe
   documented fallback" clause.

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
  -> emits `{"hookSpecificOutput": {"hookEventName": "PreToolUse",
  "additionalContext": "..."}}` on stdout, exit 0 (advisory, not blocking).
  Also confirmed live in-session (not just via direct stdin replay): a
  subsequent real `gh pr checks` call surfaced the reminder as a
  `PreToolUse:Bash hook additional context` system reminder.
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
