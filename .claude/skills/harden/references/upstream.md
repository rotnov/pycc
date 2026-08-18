# Delivering a generic finding upstream

Some findings are not about the project — they are about this bundle: harden's
cycle, its arena, its references, its scripts, or any practice skill that
rides with it (`adr`, …). Left in a field project's journal they die there;
every install keeps meeting the same defect.

## Is it generic?

Two tests, either suffices:

- the fix lands in the skill's own files (`SKILL.md`, `references/`,
  `scripts/`), not in the project's;
- the failure reproduces in a fixture that mentions nothing project-specific —
  if you cannot scrub the project out of `task.md`, it is not generic.

Project-specific lessons stay in the project's journal by design. Measured
across two real projects: their journals' lesson overlap is zero. **What
travels is the check, not the lesson** — a gate is universal where a rule is
local.

## The unit: a feature proposal, packed with its arena

An upstream delivery is one issue containing:

1. **What happened** — the incident, scrubbed of project specifics.
2. **The check** — the complete fixture inline, each file fenced: `task.md`,
   `control.md`, `patch.md`, `verify.py`, `setup.py` if state is seeded.
3. **The verdict** — the arena report of the field run: pass rates control vs
   patch per harness, judge scores, tokens, placement.
4. **Environment** — harness versions and models from the report header.

A proposal whose fixture the maintainer can run, and whose verdict
reproduces, is a patch. Anything less is an opinion.

## Where

Start from [proposal-template.md](proposal-template.md) — copy it, fill every
section, then:

```bash
gh issue create -R rotnov/harden --title "<the change, one line>" --body-file proposal.md
```

The inbox is `rotnov/harden` — issues only, the code is not published there.
Scrub before filing: the fixture and report must carry no paths, names or
data from the host project beyond what the failure itself requires.

**Nothing is delivered until the incident says so.** A field incident whose
`termination:` is `Local-skill` must carry an `upstream:` field — the issue
URL once filed, `pending` until then. The entry is incomplete without it,
exactly like a missing `fixture:`. This is what makes delivery checkable:
`install-skill.py --harvest` lists every skill-scoped incident as `filed` or
`NOT FILED`, and the inbox itself labels an arriving issue `incomplete` when
a required section is missing — a proposal is not delivered by being sent,
it is delivered by being runnable.

When the canonical repository is present on the same machine, its operator
can also pull field changes directly — `uv run scripts/install-skill.py
<project> --harvest` shows what the field copy changed and which of its
incidents terminated at the skill. The issue is still the durable record;
the harvest is the fast path.
