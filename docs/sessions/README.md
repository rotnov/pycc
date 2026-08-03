# Agent session log

Per-session handoff snapshots for autonomous agent sessions working on this
project (see [D-066](../DECISIONS.md#d-066-maintain-an-agent-retrospective-log-and-a-session-handoff-log),
narrowed by [D-130](../DECISIONS.md#d-130-decompose-the-session-handoff-log-into-per-session-files)).
Distinct from `docs/AGENT_RETROSPECTIVE.md`: this directory is "what state
is the work in and what's next," not "what went wrong."

Each file is one dated checkpoint snapshot — a PR opened or merged, a
milestone reached, or a session handoff — named `YYYY-MM-DD-<slug>.md`,
matching the dated-slug convention already used for
`docs/superpowers/specs/` and `docs/superpowers/plans/`. A snapshot
identifies the exact commit and repository state actually inspected, and
distinguishes uncommitted or unmerged work from delivered work.

This file is the only static document in the directory — a purpose
statement, not an index. It is never appended to. To resume: list this
directory sorted by filename (the date prefix makes lexical order match
chronological order) and read the most recent few entries, the same way a
fresh session already discovers the newest file under
`docs/superpowers/plans/` via `ls` rather than a maintained index.

```bash
ls docs/sessions/*.md | sort | tail -5
```

Like `docs/AGENT_RETROSPECTIVE.md`, this directory is reviewed for factual
accuracy, links, privacy, and safe handoff instructions, but it is not a
merge gate, CI-enforced, or machine-generated, and its entries do not create
implementation requirements — promote a lesson or snapshot claim into the
owning policy, ADR, or specification before treating it as normative.
