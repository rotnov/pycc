# Recurrence 2: docs/ROADMAP.md not extended for #676/T0052

**Date:** 2026-08-29
**Topic:** documentation-sweep-stops-at-the-changed-file (see `incident.md` for the
original class definition, root cause, and `precommit` termination point — this
file only records a second occurrence, per this journal's append-only convention)
**Verdict:** pending — unchanged from `incident.md`; this occurrence does not by
itself change the recommended action
**Batch:** `.harden/findings/issue-676.jsonl`, finding 1 (docs-currency), round 1

## Symptom

`docs/ROADMAP.md`'s "Language surface" row documents the D-187/#627
bool-into-int attribute-boundary history and an existing #618/T0051/D-207
sentence in the same row, but had no mention of #676/T0052/D-209 — a new
compile-time rejection that closes D-187's own residual gap — even though the
row is exactly the kind of enumeration `incident.md`'s root cause describes:
"a reference or an enumeration elsewhere in the tree was left describing the
pre-change world, while the file the change was *about* was updated
correctly." The change's own documentation duty (D-209's decision record,
the diagnostic registration) was discharged; the roadmap's cross-reference
enumeration, sitting outside that frame, was not.

## Disposition

Same shape as the original three findings (test doc comments naming stale
symbol homes, a decision record's unfounded tracker claim, a spec's
under-extended enumeration): a plain-text cross-reference or enumeration that
compiles, tests, and passes coverage identically whether it is current or
stale. Fixed on the branch (`docs/ROADMAP.md`, in `cfea9c72`, then restated more
compactly in `821eade8` to also fit issue #207's llms.txt aggregate byte
budget). The budget trim itself gets no incident of its own — it is ordinary
editorial tightening, not a new failure class.

## Effect on the termination point

No change to `incident.md`'s recommended `precommit` rung or its scope
estimate. This is the second occurrence recorded against this topic (the
first being the four `issue-629.jsonl` findings on 2026-08-20); the class
keeps recurring across otherwise-unrelated pull requests, which is evidence
*for* eventually building the lexical reference-integrity gate described
there, not evidence the gap has closed on its own. Still not built inside
this triggering pull request, for the same reason `incident.md` gives: the
gate is its own change with its own review (a pure, separately unit-tested
matcher; an attribution-syntax decision; a repository-wide sweep before
enabling it), not a one-line addition to a pull request whose review
surfaced this instance of the class.

**fixture:** none — recurrence record only, no new artefact
**artifact:** none — see `incident.md`'s `precommit` proposal, still pending
**verify:** manual — the added roadmap sentence and the adjacent trim were
verified against `sh scripts/check-site.sh` (issue #207 llms.txt budget) and
`RUBYOPT="-E utf-8" ruby scripts/check_roadmap_evidence.rb`, both passing
