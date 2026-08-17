# Artefact types, the detectability ladder, and seams

### 3.5 Choose the artefact type — before opening any file

Step 4's trace answers **who owns** the failure. This step answers **what kind of
guard** to build. Two orthogonal axes; skipping this one is how everything ends up
as prose in `AGENTS.md`.

Frame it with four questions. Without the frame a brainstorm degenerates into
plausible options with no selection criterion:

1. **Where is the source?** — agent / tool / system / process / data.
2. **What removes the cause rather than the symptom?**
3. **Who is entitled to do it?** — the agent itself / a seam / a human / the team.
4. **What will show it helped?** No metric → do not ship the artefact.

The criterion the answer usually reduces to: **what detects this failure.**

| detected by | artefact | lands in | reaches |
| --- | --- | --- | --- |
| a command over static state (`grep`, linter, test) | pre-commit hook / CI gate | `.pre-commit-config.yaml`, CI workflow | **every agent, and humans** |
| reading the diff | a reviewer checklist item + its canary | the reviewer agent | every agent (via CI) |
| the fact of a tool call | **seam** (see below) | `.claude/settings.json` + hook script | **that harness only** |
| nothing mechanical, but worth knowing | documentation | `AGENTS.md` / project docs | every agent that reads it |
| someone already solved this class | **a maintained external skill** | `.agents/skills/` + a link | every agent that reads it |
| not reproducible | **build nothing** | — | — |

**The external-skill rung comes with its own procedure, and it is not optional:**
find, audit, prove in the arena, only then install — see
[external-skill.md](external-skill.md). Installing a skill runs another party's
instructions with your permissions, and an install count measures how many people
clicked, not what the content does.

The last row is mandatory. Without it the workflow always produces something, and
a share of failures is plain stochastic noise where the right answer is silence.
That missing row is how a governance file grows 130-fold in fifteen months.

## Reach: who the artefact actually binds

The last column is a selection criterion, not trivia. Work gets split across
harnesses — one agent for one task, another for the next — and a guard living in
one harness's configuration does nothing for the others.

Reach mostly follows the ladder: the more mechanical the artefact, the wider it
binds. `precommit` is enforced by git, so it covers every agent and every human
who commits; a seam is enforced by one harness's hook configuration and covers
that harness alone. **Where two rungs both fit, take the one that reaches
further** — usually `precommit` over a seam, and for the same reason a fix to a
single harness's skill is the narrowest artefact there is.

**Whether you need reach is a measured question, and the arena already answers
it.** It runs every installed harness; look at the *control* column:

- control fails on **every harness** → the class is general. A fix to one harness's
  skill or hook is knowingly partial, whatever its verdict says.
- control fails on **one** → the class is specific to it, and a narrow artefact
  is the honest answer. Nothing to mirror elsewhere: the other harnesses do not
  have the problem.

The degenerate case — the same failure in two harnesses' own files — means the
edit lands in both, and both belong in the same change. That is copying, and
copies drift; prefer a rung that makes copying unnecessary.

**Seams.** A seam is an executable interception that makes the wrong path
impossible, rather than discouraged. It is the only artefact that works against
the class "the right tool exists and is not used" — measured case: a project skill
for waiting on CI existed, and the wrong call was made 847 times anyway.

Four subtypes:

| subtype | what it does |
| --- | --- |
| redirect | blocks the wrong call, names the right one |
| auto-complete | silently performs the step that was skipped |
| precondition | checks the operation is meaningful before it starts |
| release | clears blocking state (stale lock, held resource) |

Hard limits, or a seam becomes a hole: it may only **narrow**, never widen
(`deny` is allowed, `allow` is not — a seam that grants permissions is a machine
for weakening the perimeter that reports success); the action must be idempotent;
it must be removable in one line; and it must be proven by the arena.

**Some sources are outside the agent entirely.** When the cause is a noisy CI
gate, an uncacheable job, a broken version checker — the artefact is a fix to the
system, or an escalation to a human with options. Neither is a rule, and forcing
them into one produces prose nobody can act on.
