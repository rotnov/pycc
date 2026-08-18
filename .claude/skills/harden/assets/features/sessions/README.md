# Session handoff log

One snapshot per checkpoint: what state the work is in and what a fresh session
should pick up. Distinct from `.harden/incidents/`, which records what went
wrong — this directory records where things stand.

Each file is named `YYYY-MM-DD-NN-<slug>.md`. `NN` is a two-digit sequence for
the day, ascending in creation order, because several checkpoints can land on one
date and the date alone cannot order them.

**A new checkpoint is always a new file.** Never append to a previous snapshot:
it is a record of a moment, and two agents working at once must never contend for
the same file. This README is the only static document here and is never appended
to.

## When a snapshot gets written

At a moment the agent creates itself, never at one that merely happens to it:

- a `/harden` cycle ships an artefact,
- a task spanning several commits is finished,
- the user winds the session down.

That choice is deliberate. A trigger like "context is about to compact" is more
precise but invisible to the agent — catching it needs a harness hook, and hooks
exist in one of the three harnesses this project uses. A trigger the agent
performs with its own hands works in all three, because the agent is standing in
the moment when it fires.

The cost is honest: a long session that never reaches any of those moments leaves
no snapshot. Run one by hand when that happens.

```bash
uv run .claude/skills/sessions/scripts/session-snapshot.py --slug what-this-checkpoint-is
```

`.claude/skills/sessions/scripts/session-snapshot.py` drafts the mechanical half — branch, commit,
uncommitted paths, recent commits, incidents still carrying `verdict: pending` —
and leaves *In flight*, *Next* and *Open questions* as headings for whoever is
finishing the session.

A snapshot left with its headings unfilled is still useful — it dates the
repository state — but it is not a handoff. Say so in the entry rather than
leaving empty sections that read as "nothing in flight".

## Resuming

There is no index to keep current; the date-then-`NN` prefix makes lexical order
chronological, so the directory is the index.

```bash
ls docs/sessions/[0-9]*.md | sort | tail -5
```

Snapshots are not normative. A lesson worth keeping goes to `.harden/incidents/`,
a decision worth keeping goes to `docs/decisions/` — a claim that stays here only
records that someone believed it at that moment.
