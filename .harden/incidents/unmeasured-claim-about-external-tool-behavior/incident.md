# Incident: unmeasured-claim-about-external-tool-behavior

**Date:** 2026-08-20
**Topic:** unmeasured-claim-about-external-tool-behavior
**Verdict:** build nothing (two escalations exhausted; no rung below review reaches this class)

## Symptom

Batch pass over `.harden/findings/issue-629.jsonl` (13 findings, 5 review
rounds, one issue). Six findings cluster into one class: a universal claim
about how a tool outside the diff behaves, written as settled fact in
normative prose, and false when measured.

- Round 1 — a gate's own doc comment asserted a stripping property broader
  than the code implemented.
- Round 2 — three members. The strongest was **not a documentation defect at
  all**: the prose asserted that the external build tool exports no
  environment variable for a configuration key, and the implementation was
  built to match that assertion. Measurement showed the variable is honored,
  so the shipped resolver had a live behavioral gap. The other two asserted
  that a boundary case was handled "the way the tool handles it" and that a
  relative value "matches" the tool's own resolution; both were false as
  stated, and one of them described the tool exiting non-zero where the prose
  claimed a fallback.
- Round 3 — the replacement clause written to fix round 2 was itself reasoned
  rather than measured, and false for exactly the binaries the change's own
  contract covers.
- Round 4 — while removing round 3's overbroad clause, one of the four
  parallel copies of the passage drifted into a *new* universal about the
  complete recipient set, falsified by a mechanism the other three copies had
  correctly scoped away from.

Every member was caught by the pinned review loop before merge, and every one
was fixed on the branch. None reached the default branch.

## Root cause

Two causes compound, and only the second is specific to this class.

The first is ordinary: four rounds each rewrote the *same* passage, and the
passage exists in four near-parallel copies (a decision record, a
specification page, a roadmap line, a source doc comment). A rewrite that
lands correctly in one copy and drifts in another is a sweep failure, and it
is why round 4 exists at all.

The second is the class itself. A claim about the diff's own code is checked
by the compiler, the tests, and the coverage gate. A claim about how a
*different* tool behaves has no such backstop: it reads as authoritative,
costs one command to verify, and is instead derived from familiarity with
that tool. The decision record produced by this very change records the
lesson in its own text — and the round-3 and round-4 violations were written
into and beside that record, after it was written.

## Termination point

None reached, after two escalations.

**Escalation 1 — a rule.** Rejected on evidence already in hand, without
spending an arena campaign. The artefact a rule would produce is prose in a
governance file; the strongest available instance of exactly that artefact —
the lesson written into the decision record the author was actively editing —
failed to prevent two subsequent violations by that same author in that same
sitting. A governance file read once at session start is strictly weaker than
a sentence in the file under the cursor. `references/artefact-types.md` is
explicit that text cannot fix compliance, and this is compliance: the content
existed and was correct.

**Escalation 2 — a mechanical rung.** No rung exists. `precommit` and
`review-check` artefacts are commands with binary outcomes; deciding whether
a natural-language sentence about a third-party tool's behavior was *measured
or merely believed* is not a decidable property of the text. A gate could
detect neither the false sentence nor the true one.

Detection worked at the intended rung every round. What failed is authoring,
and the harness for authoring is the same review loop that already caught it.
Per `references/batch.md` step 4, the class is recorded rather than promoted.

## Artefact

**Type:** none (class recorded, no promotion)
**File:** n/a
**Change:** none.

This entry is a counter. A future member of this class that the review loop
*misses* — reaching the default branch rather than being caught on the branch
— changes the finding from "authoring is noisy, detection holds" to
"detection is insufficient", which is a different class with a different
answer. Recurrence caught at review does not, by itself, reopen the ladder.

## Related

`.harden/incidents/own-change-falsifies-adjacent-prose/` is adjacent but
distinct, and both were checked before this entry was written. That topic
covers prose the diff *falsified without touching* — the change made a
neighbouring true sentence false. This one covers prose the author *wrote or
rewrote deliberately*, about a tool outside the diff, and got wrong. Both are
recorded `build nothing` for related reasons, which is itself a signal: if a
third topic in this family appears, they should be consolidated and the
combined counter re-evaluated against the ladder as one class.

## Fixture

None. No artefact was built, so there is nothing to prove.

## Arena verdict

Not run — this entry ships no artefact. Escalation 1 was rejected on the
measured failure of an equivalent artefact already present in the tree
(above), not on an arena campaign.

## Verify

`verify: manual` — every member was verified fixed on the task branch before
this entry was written. The round-2 behavioral member is pinned by four unit
tests over the new precedence level; each documentation member was replaced
with a sentence whose claim was measured on the authoring host, and the
measurements are recorded in the decision record the change ships.

## Method note

The clustering for this batch was performed inline in the orchestrating
session, not through a dispatched tracer. `references/batch.md` specifies one
batched dispatch; the deviation was deliberate — the session held all 13
findings and the full five-round review history in context, and a fresh agent
would have re-read the tree to rediscover them. Recorded here rather than
left implicit, because an inline trace and a dispatched one look identical in
the output.
