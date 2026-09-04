---
id: decision-number-taken-by-a-merge-mid-review/incident
date: 2026-09-04
project: pycc
session: 2c68147a
trigger: self-post-failure
model: claude-opus-5
effort: high
harness: claude-code
type: process
termination: precommit — delegated to rotnov/pycc issue 929
related: []
fixture: none — the artefact is a static CI step with a binary outcome; proven by feeding it a colliding tree (see Verify), not by the arena
artifact: deferred — one step in ci.yml's governance job, specified in rotnov/pycc issue 929
verify: reproduced — `git archive` of a colliding `docs/decisions` tree plus `generate_decisions_index.py --check` exits 1
verdict: pending
---

# Incident: a decision number resolved correctly, then was taken by a merge landing mid-review

**Batch:** `.harden/findings/issue-918.jsonl`, class E.

## Symptom

This change's decision record was numbered `D-227` against the tree as it stood
when the record was authored — correctly, by the documented procedure of
resolving the next free number at pull-request-open time. PR #928 then merged
mid-review and claimed `D-227` for its own record. Two accepted decision records
would have carried the same id on `main`.

Caught by a hand re-run of the index checker, and renumbered to `D-228` in
`f81d3445`. Nothing mechanical caught it.

## Root cause

**Gap type: trigger.** The defence exists and is complete:
`scripts/generate_decisions_index.py` carries `check_unique_ids` (fail-closed on
a duplicate `D-NNN`, citing #803), `check_filename_matches_id`, and a `--check`
mode. It is wired to nothing.

```
$ grep -rn "generate_decisions_index" .github/workflows/ scripts/*.rb .claude/skills/ AGENTS.md
```

returns three prose mentions and no workflow reference. The checker was written,
tested, and never invoked by any gate.

Reproduced empirically: `git archive`-ing `docs/decisions` from the colliding
commit `e7fe78f7` into a scratch tree and running the checker against it exits 1
with the duplicate named.

## Termination point

`.github/workflows/ci.yml`'s `governance` job — the place the existing checker
should have been called from and is not.

## Artefact

**A `precommit`-tier static gate: one step in `ci.yml`'s `governance` job.** That
job is already in `ci-gate`'s `needs:`, already runs at `contents: read`, and is
already a sequence of `scripts/` checkers of this exact shape.

```yaml
      - name: Check decisions index freshness and id uniqueness
        run: python3 -B scripts/generate_decisions_index.py docs/decisions docs/decisions/README.md --check
```

**CI rather than a local gate, deliberately.** `ci.yml`'s `actions/checkout`
carries no `ref:`, so a `pull_request` build resolves `refs/pull/N/merge` — the
merge result, not the branch head. A CI step therefore observes a collision
introduced by a merge that landed *after* the branch was cut, which is exactly
this failure. A local-only gate cannot see the class at all: nothing in the
branch's own tree is wrong.

**Delegated rather than built here.** Editing `ci.yml` invalidates the whole-file
SHA-256 pin in `scripts/check_roadmap_evidence.rb`'s
`REVIEWED_PERF_CI_WORKFLOW_SHA256S` (constant at line 255, enforced at 2415 and
2788), which invokes the D-080/D-048/D-051 two-sequential-PR staged-fixture
procedure. That is a two-pull-request delivery in its own right and does not
belong inside a compiler change.

Filed as `rotnov/pycc` issue 929, milestone v0.4, carrying the reproduction, the
empty grep, and the D-080 cost so the implementer does not rediscover it. Filed
as a milestoned issue rather than a new standing umbrella: it is one bounded,
closable task, and umbrella issues are containers with no completion state.

This one clears AGENTS.md's filing bar on limb (a) with executed evidence — a
corrupted decision log would have merged with every required check green.

## Fixture

None. A static gate is a command with a binary outcome, not a behavioural change;
the arena has no way to exercise it, and forcing it through would produce
ceremony rather than evidence.

## Verify

`verify: manual`, both directions, to be recorded in the issue 929 activation
pull request: a `docs/decisions` tree carrying a duplicate id must exit 1 with the
id named, and the current `main` tree must exit 0. The negative direction is
already observed (the `git archive` reproduction above); the positive direction is
observed on every clean run of the checker in this session.

`verdict: pending` until issue 929's activation pull request merges.
