---
id: process-record-written-without-read-back/2026-09-02-issue-866
date: 2026-09-02
project: pycc
session: 2c68147a
trigger: self-post-failure
model: claude-fable-5-1
effort: medium
harness: claude-code
type: process
termination: precommit
related: []
fixture: scripts/test_check_harden_findings.py — constructed git checkouts, both directions
artifact: scripts/check_harden_findings.py
verify: manual — 18 unit cases: an excluded-then-`git add -A` pile, an untracked pile, a malformed disposition, a note-less refutation and a `fixed` line without a `fix_commit` are rejected; a tracked well-formed pile and every checked-in pile pass
verdict: profit
---

# Incident: process-record-written-without-read-back

**Batch:** `.harden/findings/issue-866.jsonl`, findings 3 and 4 (both `fixed`)

## What happened

Two writes to the task's findings pile were accepted without reading the
result back. (3) `git add -A` silently skipped the new
`.harden/findings/issue-866.jsonl`: the checkout's machine-local
`.git/info/exclude` lists `.harden/` (a local testing note), although the
directory is tracked on `origin/main` with fifteen earlier piles, so
`git status` was clean and the pile was absent from the diff the round-2
reviewer read. (4) The round-1 entry for a refuted finding was written with
`disposition: fixed` and a note that was a refutation reason, so the batch
pass would have counted a reviewer error as a fix. Both surfaced only
because the round-2 reviewer read the file and the diff.

## Why it was not caught

The step 5 rule "stage all changes, including new files" is prose about
untracked files and cannot see an *excluded* path -- `git status` does not
list it either. The pile schema in `references/batch.md` defines the two
disposition values but nothing validates a line against them. Gap type:
trigger for (3) -- the existing guard exists and cannot fire; absence for
(4). Combined: no read-back of the pile at all.

## Artefact and why this type

`scripts/check_harden_findings.py`, run at step 5.5 before the batch:
`git ls-files --error-unmatch` on each pile plus a per-line schema check
(required keys, `disposition` in the enum, a `refuted` line carries a
note, a `fixed` line carries a `fix_commit` -- the last added after the PR
#871 Codex review observed that without positive evidence for a fix, the
exact observed corruption (4) still passed). Static rung: a command over static state detects the class, it binds
every harness and the human, and it lives in the `scripts/` suite CI already
discovers (`test_check_harden_findings.py` runs every checked-in pile
through it). The prose alternative -- "confirm with `git ls-files`" beside
the stage-everything rule -- was rejected: it is the rung already chosen
for the untracked-file sibling and this incident showed text cannot see a
mechanism invisible to `git status`. The machine-local exclude itself is
user discipline outside the tree: it will repeat (3) for every `git add -A`
in every worktree of that checkout until the entry is removed, and the
session report names it.

## Proof

`python3 -B -m unittest scripts/test_check_harden_findings.py` -- both
directions proven on constructed checkouts, including the exact observed
failure (an `info/exclude` entry followed by `git add -A`). Sweep result:
the sixteen checked-in piles split into eight written before
`references/batch.md`'s line schema existed (`status`/`finding`/
`description` keys; issue-197 through issue-719), snapshotted in the
checker's `LEGACY_SCHEMA_PILES` and exempt from the schema half only, and
eight that conform once `line` is treated as optional (three doc-level
piles omit it legitimately). The snapshot only shrinks; every new pile is
held to the schema.
