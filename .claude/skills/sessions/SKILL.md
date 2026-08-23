---
name: sessions
description: Use at a checkpoint the agent itself creates — a /harden cycle shipped an artefact, a multi-commit task is finished, or the user winds the session down — to write a handoff snapshot a fresh session can resume from. Also sets the handoff log up in a project that lacks one, auditing for an existing practice first.
---

# Sessions: the handoff log

One snapshot per checkpoint under `docs/sessions/`: what state the work is in
and what a fresh session should pick up. **In this repository the trigger is
narrower than the generic one below:**
[D-192](../../../docs/decisions/D-192-bound-the-tracker-with-milestone-at-filing-a.md)
allows at most one file per *merged pull request*, and routes everything else a
checkpoint would have captured to `docs/AGENT_RETROSPECTIVE.md`. Distinct from `.harden/incidents/`,
which records what went wrong — this records where things stand.

## If the project has no handoff log yet

**Audit before touching anything — an existing mechanism must survive you:**

1. Search the governance file for the practice's vocabulary (session, handoff,
   snapshot, journal, retrospective).
2. Look for existing directories: `docs/sessions/`, `docs/handoff/`, a session
   log kept elsewhere under another name.
3. Read what exists for policies your defaults would contradict.

Any hit → stop and report; their edition wins unless the user explicitly says
otherwise. Nothing found → install (the bundle lives in the `harden` skill
beside this one; use whichever skills root the project has):

```bash
uv run .agents/skills/harden/scripts/install-feature.py sessions
```

Then perform the printed after-steps and finish with `--check`.

## Writing a snapshot

The drafter fills the mechanical half — branch, recent commits, dirty files —
and leaves headings for what a script cannot know:

```bash
uv run .claude/skills/sessions/scripts/session-snapshot.py --slug <what-this-checkpoint-is>
```

Fill the three narrative sections yourself: the state of the work, what is in
flight, what the next session should do first. Filename is
`YYYY-MM-DD-NN-<slug>.md` — NN is picked by the script so two agents writing
at once never touch the same file.

- **A new checkpoint is always a new file.** Never append to a previous
  snapshot: it is a record of a moment.
- **Lessons from failures go to `.harden/incidents/`, not here** — this log
  records state, the journal records what went wrong and the guard built.

## Why the trigger is a list of acts

The checkpoints above are moments the agent creates with its own hands, never
events that merely happen to it. "Context is about to compact" is a more
precise trigger and invisible to the agent — catching it needs a harness hook,
and hooks exist in a minority of harnesses. An act-trigger works in every one,
because the agent is standing in the moment when it fires (D-009). The cost is
honest: a session that never reaches any of those moments leaves no snapshot.
