# Incident: failure-expecting-test-passes-on-the-wrong-guard

**Date:** 2026-08-20
**Topic:** failure-expecting-test-passes-on-the-wrong-guard
**Verdict:** escalated to a static gate; artefact owned by a tracked issue

## Symptom

Batch pass over `.harden/findings/issue-197.jsonl` (23 findings, 3 review
rounds, one issue). Four findings across rounds 2 and 3 cluster into one
class: a test that asserts a validator *rejects* a deliberate mutation, and
passes because the validator exited non-zero — but on a different guard than
the one the test was written to exercise.

The mechanism is shared mutable fixtures. Each mutation block restores only
the file it is about to mutate; dirt written into any other fixture survives
until that other file's own next restore, and every intervening block inherits
it. Round 2 found two such sites. The round-2 fix restored at the source of
the dirt and was verified per-mutation. Round 3 found the class again, and
instrumenting every guarded invocation with a line marker plus captured stderr
showed 18 vacuous sites, not 2 — essentially the whole pre-existing section.

## Root cause

A failure-expecting test whose only assertion is a non-zero exit status
cannot distinguish "rejected for the reason under test" from "rejected for
any other reason". The expected message appearing somewhere in the run is not
evidence of causation. Under a suite that exits at first failure, deleting
guards one at a time to prove causation is O(n) and was never run to
completion; the cheap general form is to instrument every guarded invocation
and read which message each one actually produced.

Gap type: **absence** — nothing in the tree required a failure-expecting test
to bind an expected message.

## Termination point

`precommit` (static gate). The prior occurrence of this topic terminated at
`review-check` and recorded `build nothing`, on the reasoning that the
mechanical alternative would not have caught its own instances. That
reasoning does not carry here: "a guarded invocation asserts no expected
message" is a purely syntactic property of the test file, and a checker for
it would have caught every member of this class, including both round-2
blockers, before review saw them. Per that incident's own closing note, a
second occurrence disqualifies the review tier and forces the next rung.

## Artefact

A checker over the repository's shell test suites: every guarded invocation
whose failure branch reports "the validator accepted X" must bind the
diagnostic it expects, not merely a non-zero exit. Landed as a ratchet — the
18 currently-vacuous sites enter an explicit allowlist that shrinks to zero as
they are fixed, so the gate is green from the first commit and no new
occurrence can be added.

**Deliberately not landed in the pull request this batch came from.** The gate
is only satisfiable alongside the 18 fixes, which change a shared harness
invariant and will require new guards in the validator itself where un-masking
reveals a property with no check at all. Both halves are owned by
https://github.com/rotnov/pycc/issues/644, which carries the marker table and
the invariant proposal. This is a routing decision with a named owner, not a
deferral.

## Fixture

None yet — the artefact lands with #644, and its fixture belongs to that
change. The arena does not apply: a static gate is a command with a binary
outcome, proven by feeding it deliberate violators, not by measuring agents.

## Verify

`verify: pending` — proof is the checker rejecting a message-less guarded
invocation with a non-zero exit and accepting a bound one with zero, recorded
against #644.
