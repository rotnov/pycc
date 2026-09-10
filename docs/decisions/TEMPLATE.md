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

## Amending an accepted entry

Once the frontmatter says `accepted` (or `superseded`) the entry's existing lines are
frozen: never edit or delete an original line, including to fix a typo or to note that
a circumstance has since changed. CI's `governance` job enforces this with
`scripts/check_decision_immutability.py` (D-240), which compares the file at the pull
request's base with the file at its head and requires every base line to reappear
verbatim and in order. What you may do:

- **Add lines.** Record a correction, a later development, or a clarification as *new*
  lines -- a dated `- Amendment (YYYY-MM-DD): ...` line, or a dated note block -- placed
  by convention directly after the `- Status: ...` line (a style recommendation, not
  enforced). A same-line append (`... original text. (Since changed -- see D-NNN.)`)
  changes the line and is rejected.
- **Transition or narrow the status.** To supersede an entry, replace only the
  frontmatter `status:` line (`accepted` -> `superseded`; never back to `accepted` or
  `proposed`) and/or the first body `- Status: ...` line (`- Status: superseded by
  D-NNN`, with any explanation as inserted continuation lines), and put the substance
  in the new entry. To narrow one clause while the decision stays in force, keep the
  frontmatter `accepted` and rewrite only that first `- Status:` line (`- Status:
  accepted (the ... clause is narrowly superseded by D-NNN ...)`). The replacement
  must start with `- Status: accepted` or `- Status: superseded` (an annotation may
  follow after a space); `- Status: proposed`, `- Status: rejected`, or a blank
  `- Status: ` is rejected. Every other line, including any second `- Status:` line,
  is frozen.
- **Fill in an index-only stub.** A D-151 stub (`Index-only: no long-form entry
  recorded yet.` and no `- Status:` line) may have its five stub body lines replaced by
  the long-form section, provided the new section itself carries a `- Status: accepted`
  or `- Status: superseded` line -- inside the replaced body, before any lines that
  followed the stub; a status line placed after those lines does not count. Deleting
  the stub body, or replacing it with text without such a line, is rejected. Its
  frontmatter and any lines after the stub stay frozen.

Deleting or renaming an accepted or superseded file is a violation. Verify locally with
`python3 -B scripts/check_decision_immutability.py --base "$(git merge-base origin/main
HEAD)" --head HEAD`.
