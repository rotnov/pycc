# Incident: coverage-claimed-without-executing-the-check

**Date:** 2026-08-20
**Topic:** coverage-claimed-without-executing-the-check
**Verdict:** pending (no artefact built)

## Symptom

Two findings from the same PR (#626, issue #624) share one root cause: a
claim about what was covered, written without running the thing that
would settle it.

1. The pinned local reviewer found that a refcounting change had no test
   exercising the generated code at codegen depth — the change was
   described as covered by tests that only reached the layer above.
2. A source comment asserted that a specific construct's handling was
   exercised by a named test; the test was real but did not exercise the
   construct the comment claimed.

Both were caught in review, before merge, and both were fixed on the same
branch.

## Root cause

Writing a coverage claim is cheap; verifying it means running or reading
the cited test and confirming it reaches the code in question. Under time
pressure the claim gets written from intent rather than from evidence.
Gap type: **compliance** — the standard is already stated (this project
treats its 100% coverage gate as a merge invariant satisfied by
meaningful execution, not incidental line hits), and it was not followed.

## Termination point

`review-check`. Both findings were caught by exactly the artefact that
should catch them: the required independent review pass this project runs
before every merge (its D-068 pinned local reviewer).

## Artefact

**None — build nothing, deliberately.**

The detection signal for this class is *reading the diff against the
code it claims to cover*, which is the review tier by definition, and
that tier already fired on both instances. The obvious mechanical
alternative — a checker asserting that every test name cited in a comment
exists — would not have caught finding 2 at all: the cited test existed,
the claim about what it exercised was what was false. A guard that
catches neither instance in the class it is named for is ceremony.

Recorded rather than built, so a recurrence escalates from here instead
of being rediscovered as new. Per the harden ladder, a second occurrence
of this topic disqualifies the review tier and forces the next rung.

## Fixture

None — no artefact to test.

## Verify

`verify: n/a`. Nothing was shipped for this class.
