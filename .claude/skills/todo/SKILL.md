---
name: todo
description: Use when the user defers something — "put it in the backlog", "note it for later", "not now", "write this down and move on". Records the idea with the context needed to restart it cold, then stops. Also use to list what is deferred, or to close an entry that has been done or dropped.
license: MIT
---

# Todo: record deferred work, then stop

Deferral is a decision about scope, not about where to file a note. This skill
records the entry and ends the work — the stopping is the point, not a side
effect.

## If the project has no todo log yet

**Audit before touching anything**: search the governance file for the
vocabulary (todo, backlog, deferred, later) and look for an existing edition —
`docs/todo/`, `TODO.md`, `BACKLOG.md`, an issue tracker named as the deferral
home. Any hit → stop and report; their edition wins unless the user explicitly
says otherwise. Nothing found → install (the bundle lives in the `harden`
skill beside this one; use whichever skills root the project has):

```bash
uv run .claude/skills/harden/scripts/install-feature.py todo
```

Then perform the printed after-steps and finish with `--check`.

## Why this is a skill and not a rule

A rule saying "do not start work that was deferred" has no moment an agent can
recognise. It applies at the instant of judgement, which is exactly the instant
that judgement is unreliable — and the failure that motivated this skill looked
like preparation, not disobedience: files were cleaned up so a deferred gate
would pass, twice, each time with a reason that sounded like diligence.

A request to defer is different. It arrives as text, so it matches a description
and invokes something. The boundary becomes an action with a visible result.

## Record an entry

1. **Find the next id.** `ls docs/todo/T-*.md | tail -1`. Ids are sequential and
   never reused, including for dropped entries — a gap says something was
   considered, which is information.

2. **Write `docs/todo/T-0XX-<slug>.md`** per `docs/todo/TEMPLATE.md`. Fill
   `session` and `decisions`:
   - `session` — the newest entry in `docs/sessions/`, or today's date if none.
   - `decisions` — ids from `docs/decisions/` already settled on this topic.

   These two fields are the difference between a backlog and a list of
   assertions. An entry written today makes sense today; in a month only the
   conclusion survives and reads as arbitrary. The links restore what was known
   when it was written.

3. **Write the body so a cold reader can start.** Name the files, the commands,
   the constraint that was already agreed. "Improve the installer" is not an
   entry; "installer features ship unproven — a routing line is a `rule`, and
   step 6.2 makes the arena mandatory for those" is.

4. **State what you recorded and stop.** Do not make the change, do not prepare
   for it, do not clean anything up so it will be easier later. If part of the
   work seems trivially safe to do now, that judgement is the failure this skill
   exists to prevent — put it in the entry as *Open questions* instead.

## List and close

```bash
grep -l "^status: open" docs/todo/T-*.md      # what is deferred
```

After adding or closing an entry, regenerate the index:

```bash
uv run .claude/skills/adr/scripts/generate-index.py docs/todo docs/todo/INDEX.md --prefix T- --label Todo
```

Closing: set `status: done` and list the decision ids it produced, if any. An
entry that concluded in a decision is done, not superseded — the task was
carried out, and the decision is what it concluded.

Dropping: set `status: dropped` and add one line saying why. A rejected idea
with its reason is worth more than one that vanished, because the reason is what
stops it being raised again.

## Hard limits

1. **Never start the deferred work**, including its preparation, its cleanup, or
   the gate that would check it. "Getting it ready" is doing it.
2. **Do not migrate an archived backlog on your own initiative** — an entry moves
   here when it is next touched. A migration the user asks for is different, and
   is a revision rather than a copy: an entry carried over unexamined arrives
   with a status nobody verified.
3. **One entry per idea.** Two ideas in one file cannot be closed separately, and
   the half that was done hides the half that was not.
4. **Never hand-edit the index table.** It is generated from frontmatter;
   an edited one is stale the moment a status changes.
