# Incident: documentation-sweep-stops-at-the-changed-file

**Date:** 2026-08-20
**Topic:** documentation-sweep-stops-at-the-changed-file
**Verdict:** pending (promotion identified, deliberately not built inside the
triggering pull request)

## Symptom

Batch pass over `.harden/findings/issue-629.jsonl` (13 findings, 5 review
rounds). Four findings cluster into one class: a reference or an enumeration
elsewhere in the tree was left describing the pre-change world, while the
file the change was *about* was updated correctly.

- Two test files carried doc comments pointing at functions by their old
  home, after those functions moved from the binary crate into a new library
  module. Both comments named a path that still exists and a symbol that is
  no longer in it — the compiler cannot see either, because both live in
  prose.
- A decision record claimed a deferral was "tracked as its own follow-up"
  when no tracker item existed.
- A specification page enumerated the one environment variable the resolver
  honored, and was not revisited when the resolver gained a second.

None was a code defect. All four were caught by the pinned review loop and
fixed on the branch.

## Root cause

The change's own documentation duty was discharged where the change was: the
new module got its docs, the decision record was written, the roadmap line
updated. Every member of this class sits *outside* that frame — a comment in
an unrelated test file, an enumeration on a page the diff otherwise does not
touch, a forward reference to something that was supposed to exist and does
not. Nothing in the toolchain sees any of them: a stale `path::symbol`
reference inside a `//` comment compiles, tests, and passes coverage exactly
as a correct one does.

## Termination point

**Rung: `precommit`** — a lexical reference-integrity gate, the same shape as
the gate this change already ships. That gate walks an explicit source
allowlist, strips line comments, and fails with `file:line` on a forbidden
literal. The sibling this class needs walks the same allowlist in the other
direction: for every `path/to/file.rs` reference appearing in a comment or a
`docs/` page, assert the path exists; for every `` `symbol` `` attributed to
such a path, assert the path contains it. Both halves are decidable by
reading the tree, which is exactly what makes this class — unlike its
neighbour topic — mechanically reachable.

Scope check, so the estimate is honest rather than optimistic: the project
enforces 100% line and region coverage as a merge invariant, so the gate's
own matcher needs a pure, separately unit-tested core with offending and
clean samples, mirroring what the shipped gate does. It also needs a decision
about attribution syntax before it can match anything, and a repository-wide
sweep of existing references before it can be turned on. That is its own
change with its own review, not an addition to the pull request whose review
surfaced the class.

## Artefact

**Type:** precommit (identified, not built)
**File:** n/a — nothing landed for this class in the triggering pull request.
**Change:** none yet.

`verdict: pending` means this entry is **open**. It closes when the gate is
built and proven, or when a later batch shows the class does not recur and
the cost is not worth paying.

## Fixture

None yet. A static gate is proven by feeding it deliberate violators, not by
an arena campaign: a stale path reference and a stale symbol attribution must
each be rejected with a non-zero exit naming `file:line`, and a clean tree
must be accepted with exit zero. That evidence belongs in this entry when the
gate is built.

## Arena verdict

Not applicable. `precommit` artefacts are commands with binary outcomes; the
arena measures agent behaviour and cannot exercise one.

## Verify

`verify: manual` — all four members were verified fixed on the task branch
before this entry was written: each stale reference was re-pointed at its
current home and the target confirmed to contain the named symbol, the
unfounded tracker claim was replaced with the repository's own
"untracked by any issue yet" wording, and the enumeration was extended to
both levels the resolver now honors.

## Method note

Clustering for this batch was performed inline in the orchestrating session
rather than through a dispatched tracer — see the same note in
`.harden/incidents/unmeasured-claim-about-external-tool-behavior/incident.md`
for the reasoning.
