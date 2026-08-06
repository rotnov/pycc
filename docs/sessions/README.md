# Agent session log

Per-session handoff snapshots for autonomous agent sessions working on this
project (see [D-066](../decisions/D-066-maintain-an-agent-retrospective-log-and-a-session.md),
narrowed by [D-130](../decisions/D-130-decompose-the-session-handoff-log-into-per.md)).
Distinct from `docs/AGENT_RETROSPECTIVE.md`: this directory is "what state
is the work in and what's next," not "what went wrong."

Each file is one dated checkpoint snapshot — a PR opened or merged, a
milestone reached, or a session handoff — named `YYYY-MM-DD-NN-<slug>.md`,
extending the dated-slug convention already used for
`docs/superpowers/specs/` and `docs/superpowers/plans/` with a two-digit
`NN` sequence number. `NN` disambiguates same-day entries by true creation
order — `01` is the oldest checkpoint written that day, ascending to the
newest — since several dates in this project's history have many same-day
checkpoints (2026-07-26 alone has 14) and the date prefix alone does not
distinguish their order. When adding a new entry, use the next unused `NN`
for that date (or `01` if it is the first entry of the day). A snapshot
identifies the exact commit and repository state actually inspected, and
distinguishes uncommitted or unmerged work from delivered work.

This file is the only static document in the directory — a purpose
statement, not an index. It is never appended to. To resume: list only the
dated entries (excluding this file) sorted by filename — the date-then-`NN`
prefix makes lexical order match true chronological order — and read the
most recent few, the same way a fresh session already discovers the newest
file under `docs/superpowers/plans/` via `ls` rather than a maintained
index.

```bash
ls docs/sessions/[0-9]*.md | sort | tail -5
```

Like `docs/AGENT_RETROSPECTIVE.md`, this directory is reviewed for factual
accuracy, links, privacy, and safe handoff instructions, but it is not a
merge gate, CI-enforced, or machine-generated, and its entries do not create
implementation requirements — promote a lesson or snapshot claim into the
owning policy, ADR, or specification before treating it as normative.
