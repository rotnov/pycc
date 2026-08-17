# Design decisions (ADR log)

One file per decision that is hard to reverse. Statuses run `proposed` →
`accepted` → `superseded by D-0YY`; changing an accepted decision means a new
entry, never an edit — see [TEMPLATE.md](./TEMPLATE.md).

The index lives in `INDEX.md` beside this file and is generated from each `D-*.md`
file's frontmatter — never edited by hand. Run the generator after adding or
changing an entry; `--check` verifies it is current and runs in pre-commit:

```bash
uv run .claude/skills/adr/scripts/generate-index.py docs/decisions docs/decisions/INDEX.md
```
