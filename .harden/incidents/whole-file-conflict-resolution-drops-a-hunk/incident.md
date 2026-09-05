---
id: whole-file-conflict-resolution-drops-a-hunk/incident
date: 2026-09-04
project: pycc
session: 2c68147a
trigger: self-post-failure
model: claude-opus-5
effort: high
harness: claude-code
type: process
termination: none — counter only (first occurrence)
related: []
fixture: none — no artefact was built on a first occurrence
artifact: none — build nothing, deliberately
verify: n/a. Nothing was shipped for this class.
verdict: pending
---

# Incident: whole-file-conflict-resolution-drops-a-hunk

**Batch:** `.harden/findings/issue-918.jsonl`, class B.

## Symptom

A rebase onto `origin/main` conflicted in `docs/ROADMAP.md`. The conflict was
resolved with `git checkout --ours docs/ROADMAP.md`. That resolved the conflict
and simultaneously discarded a *non-conflicting* hunk from the same commit — the
`protocol *attributes*` narrowing at `docs/ROADMAP.md:151`, which had been
deliberately authored one commit earlier. The loss was silent: the rebase
completed, the tree was clean, and every gate stayed green, because a deleted
sentence breaks nothing mechanical.

Recovered in `fb31f624` only because the diff was re-read against the
pre-rebase head by hand.

## Root cause

**Gap type: absence.** Nothing in this repository's process or tooling compares
a post-rebase commit range against its pre-rebase equivalent.
`git checkout --ours <file>` is whole-file: it takes the base side of the entire
file and drops every hunk the commit being applied contributed, conflicting or
not. The name reads as "keep my side of the conflict"; the behaviour is "discard
this commit's version of this file". Nothing prints.

## Termination point

No artefact exists to terminate at. The conflict-resolution step is not
described in any skill, gate, or governance sentence in this repository.

## Artefact

**None — build nothing, deliberately.** First occurrence, and the detector is
already exact and cheap, so the honest disposition is to record the detector and
count the class rather than ship a guard on a sample of one:

```
git range-diff <old-base>..<pre-rebase-head> <new-base>..<HEAD>
```

Every dropped hunk appears as a `-` line in the range-diff and nowhere else. On
a second occurrence this becomes a concrete rung — the command belongs in the
rebase step of whichever skill performs the rebase, or in a pre-push check when
the branch has been rebased.

## Fixture

None — no artefact to test.

## Verify

`verify: n/a`. Nothing was shipped for this class.
