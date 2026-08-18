# Todo

One file per piece of deferred work. `T-0XX-<slug>.md`, format in
[TEMPLATE.md](./TEMPLATE.md).

The index lives in `INDEX.md` beside this file, generated from each entry's
frontmatter — never edited by hand:

```bash
uv run .claude/skills/adr/scripts/generate-index.py docs/todo docs/todo/INDEX.md --prefix T- --label Todo
```

Generated rather than maintained for the reason that applies doubly here:
statuses change constantly, and a hand-kept index would be stale within a day
while still reading as authoritative.

## What belongs here, and what does not

- **Here**: work that was raised and deliberately postponed, with enough context
  to restart cold.
- **`docs/decisions/`**: a choice that was made. A todo entry may produce one.
- **`.harden/incidents/`**: something that went wrong and the guard built for it.
- An older flat backlog, if the project keeps one, stays as an archive: entries
  are not migrated wholesale — one moves here when it is next touched, revised
  rather than copied.

## Why a separate skill writes these

Because "note it and stop" has no moment an agent can recognise on its own,
while "the user asked to defer this" does. The `todo` skill is invoked by that
request, records the entry, and ends — making the boundary an action with a
visible result rather than a rule about restraint.
