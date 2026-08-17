# Decision entry template

```
---
id: D-0XX
title: "Title, stated as the decision itself"
status: proposed
---

## D-0XX: Title, stated as the decision itself

- Status: proposed
- Context: what forces the choice — the constraint, the measurement, the failure
- Decision: what we do
- Alternatives: what was rejected, and why it lost
- Consequences: what gets easier, what gets harder, what becomes irreversible
```

The frontmatter is required: `scripts/generate-index.py` reads it to
build the index table in `README.md`, and rejects a file that omits it. The
`status` field mirrors the first word of the body's `- Status:` line. Quote the
`title` as YAML (escape `"` as `\"`, `\` as `\\`).

Statuses run `proposed` → `accepted` → `superseded by D-0YY`. **Changing an
accepted decision means writing a new entry, not editing the old one** — the log
is a record of what was believed when, and rewriting it destroys the only thing
it is good for.

**Alternatives is the load-bearing section.** A decision with no rejected
alternative was not a decision, it was the only thing anyone thought of; say so
plainly rather than inventing a strawman to reject.

After adding or changing a file, regenerate the index:

```bash
uv run .claude/skills/adr/scripts/generate-index.py docs/decisions docs/decisions/INDEX.md
```
