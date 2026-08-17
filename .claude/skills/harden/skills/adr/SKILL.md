---
name: adr
description: Use when a hard-to-reverse choice is being made or has just been made — an architecture pick, a policy, a convention worth outliving the session — to record it as a decision entry. Also sets the decisions log up in a project that lacks one, auditing for an existing practice first.
---

# ADR: record the decision, or set the log up

One file per hard-to-reverse choice, under `docs/decisions/`, with a generated
index. Decisions otherwise survive only in commit bodies, where they are found
by archaeology.

## If the project has no decisions log yet

**Audit before touching anything — an existing mechanism must survive you:**

1. Search the governance file for the practice's vocabulary (decision, ADR,
   supersede) — a hand-written rule means the project runs its own edition.
2. Look for existing directories: `docs/decisions/`, `docs/adr/`, `adr/`.
3. Read what is already there for policies your defaults would contradict.

Any hit → stop and report what exists; their edition wins unless the user
explicitly says otherwise. Nothing found → install the bundle (it lives in the
`harden` skill beside this one — use whichever skills root the project has):

```bash
uv run .agents/skills/harden/scripts/install-feature.py adr
```

(`.claude/skills/harden/…` where that is the root.) Then perform the printed
after-steps and finish with `--check`. The installer injects the one routing
line into `AGENTS.md` — where it lives, what regenerates it, and that this
skill is the way to work with it. Never paste mechanics into the governance
file; that is how one measured `AGENTS.md` reached 158 KB.

## Writing an entry

New file `docs/decisions/D-0XX-<slug>.md`, next free number, from
`docs/decisions/TEMPLATE.md`. The frontmatter (`id`, quoted `title`,
`status`) is read by the index generator and is not optional.

- **Title is the decision itself**, not the topic: "Runs execute inside
  Docker", not "About sandboxing".
- **Context**: the constraint, measurement or failure that forces the choice.
- **Decision**: what we do.
- **Alternatives is the load-bearing section**: what was rejected and why it
  lost. No rejected alternative means it was not a decision — say so plainly
  rather than inventing a strawman.
- **Consequences**: what gets easier, what gets harder, what becomes
  irreversible.

Then regenerate the index — a hand-kept one drifts, and a drifted index reads
as authoritative:

```bash
uv run .claude/skills/adr/scripts/generate-index.py docs/decisions docs/decisions/INDEX.md
```

## Changing a decision

**Never rewrite an accepted entry.** Write a new one and mark the old
`superseded by D-0YY` — the log records what was believed when, and rewriting
destroys the only thing it is good for. Statuses run `proposed` → `accepted` →
`superseded by D-0YY`.

## When NOT to file one

Reversible choices, implementation details a diff explains, anything the code
already states. A log where everything is a decision is a log nobody reads.
