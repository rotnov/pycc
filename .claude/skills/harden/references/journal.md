# Incident journal: layout, routing, mandatory fields

### 7. Record the incident

One file per incident, inside a folder named after the **topic**:

```
<project>/.harden/incidents/<topic>/<YYYY-MM-DD>-<session-suffix>.md
```

The topic folder is the recurrence counter. Two files in it mean the theme came
back; three disqualify a textual artefact. `ls` answers "has this happened
before" with no tooling — which matters, because that is the journal's most
frequent query and a flat date-sorted list cannot answer it.

Before creating a new folder, `ls .harden/incidents/` and try to land in an existing
one. Several people on several machines will otherwise invent `waiting-on-ci`,
`ci-polling` and `async-wait` for one theme, and the structure that was meant to
expose recurrence will hide it instead. Keep the topic list flat.

**Where it goes** follows where the fix physically landed — no judgement call:

| the artefact landed in | journal |
| --- | --- |
| project `AGENTS.md`, project skills, pre-commit, CI, source | `<project>/.harden/incidents/` |
| `~/.claude/settings.json`, a global hook or skill | `~/.harden/incidents/` |
| nothing landed (`none`, or a system outside the project) | project journal |

Project journals are committed: they travel with the fix into the PR and stay
visible to whoever comes next. **Scrub before writing** — entries carry paths,
tool output and sometimes tokens, and committing is publishing.

```yaml
---
id: <topic>/<date>-<session-suffix>
date: 2026-08-09
project: <repo-name>
session: a3f2…
trigger: user            # user | self-post-failure | self-realized | scanner
model: claude-opus-5     # which model made the mistake
effort: high
harness: claude-code     # claude-code | codex | devin
type: verification       # logic|suboptimal|scope|convention|process|verification
termination: hook        # gate ladder (hook|precommit|review-check|rule|system|none|doc)
                         # or the workflow's routing terms (Project-rule|Local-skill|
                         # Local-agent|Upstream-plugin|user-discipline) — both vocabularies
                         # are real answers to "what terminated the trace"
related: []              # ids of earlier incidents on this topic
fixture: skills/arena/fixtures/…   # required unless termination is none/doc
artifact: .claude/settings.json    # required unless termination is none — a path, not prose
verify: arena                      # arena | manual | none | pending
verdict: pending                   # pending | profit | zero | harm | no baseline
---

## What happened
## Why it was not caught
## Artefact and why this type
## Proof
```

**`fixture`, `artifact` and `verify` are mandatory.** Without them nothing can be
reproduced and nothing can be proven — that is a diary entry, not a journal entry.
An incident whose `verdict` is still `pending` is **open**, however well written.
Exemptions: `termination: none` (one-off noise) and `doc` (no behaviour to test).

Enum fields carry evidence after an em dash — `verify: manual — 15 unit cases
prove both directions` is a value AND its proof, and the gate
(`scripts/check-incidents.py`, wired into pre-commit) checks the value part
only. The gate exists because the template alone bound nobody: entries in two
repositories drifted from it within hours of each other on the day it was
tested, and the gate's first live run caught two more silent omissions.

**The pointer runs both ways.** The entry's `artifact:` field names what the
incident shipped; the artifact carries at most a short reverse reference —
`(incident: <topic>)` — and none of the story. The journal is the editor's
depth (why the artifact exists, what proved it); the artifact is the
executor's instruction. Justification prose in a body is water the executor
pays for on every load — move it here and leave the pointer.

**Why `model` and `effort`.** They give the only attribution with an objective
test: reproduce the incident on another model. Reproduces everywhere → the
artefact is at fault. Only on one → the model or its configuration is. Every
other form of attribution is a plausible story with no way to check it.

**Why `trigger`.** It measures autonomy. If every entry says `user`, the mechanism
is a stenographer for human corrections, not a self-improving system — worth
knowing, and only measurable if the field exists.

**Cross-session index.** Append one line per incident to `~/.harden/index.jsonl`.
Parallel sessions in other worktrees cannot see each other's uncommitted files;
the index is the only place where they observe each other, and the only way a
cross-project recurrence (the same theme in two repos) is ever detected. It is
derived data — rebuildable by rescanning the journals.
