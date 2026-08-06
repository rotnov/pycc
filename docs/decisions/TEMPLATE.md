# Decision entry template

```
---
id: D-0XX
title: "Title"
status: proposed
---

## D-0XX: Title
- Status: proposed
- Context: what forces the choice
- Decision: what we do
- Alternatives: what we rejected and why
- Consequences: what gets easier / harder / irreversible
```

The frontmatter block is required — `scripts/generate_decisions_index.py` reads it to
build `docs/decisions/README.md`'s index table and rejects a file that omits it. The
`status` field mirrors the entry's own `- Status: ...` line's first word; the `title`
field is YAML-quoted (escape `"` as `\"` and `\` as `\\`).

Entries graduate from `proposed` to `accepted` (first PR that depends on the decision
must include the long-form section) — update both the frontmatter `status` and the body's
`- Status: ...` line, then regenerate the index with `scripts/generate_decisions_index.py
docs/decisions docs/decisions/README.md`.
