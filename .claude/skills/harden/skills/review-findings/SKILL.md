---
name: review-findings
description: Use when review findings exist and are not yet persisted — a review just produced them, whoever ran it and whyever — to record the pile before reporting. Also the door to wiring a review workflow so findings persist automatically, and to running the harden batch pass over an accumulated pile. Mechanics live in harden's batch reference.
---

# Review findings: collect the pile, then one harden pass

A review-fix loop catches failures one at a time; the class only exists in
the pile. This skill owns the user-invoked moments AND, in free-form
review tasks, usually fires on its description alone — measured across four
harnesses: devin, grok and codex persisted findings unaided 9/9; claude 0/6,
whose minimal-scope reading resisted even a governance routing line (arena
`20260811-012229` — the line verdicted `zero`, so it is not shipped).
Inside a scripted workflow the law is the opposite: a running procedure
outranks every sibling description (measured the same day), so unattended
collection there always rides as a step inside the host workflow itself.

## 1. Wire a host workflow to collect

Add two routing lines to the workflow skill that runs the review loop —
mechanics stay here and in the contract, never inlined there:

- in the fix loop, as each round's verdicts land: *append every finding,
  fixed and refuted alike, to `.harden/findings/<task>.jsonl`, one JSON line
  with exactly these keys — `round`, `file`, `category`, `summary`,
  `disposition` ("fixed"|"refuted"), optional `note` (the refutation
  reason) and `fix_commit`; collection never interrupts the loop*. The keys
  ride IN the wire line, not behind a pointer — field-measured: a wire that
  said "schema: batch.md" produced a dialect (`finding`/`status` for
  `summary`/`disposition`) on its first live round, because a pointer
  behind a running procedure is not read;
- at the loop's end — as its own numbered step between the review loop and
  the pull request, never a tail paragraph: procedures execute steps and
  report by their headings; prose after a loop's closing guidance is what a
  step-runner leaves behind. Bind the step to the loop by the host's own
  step title and both of its end states, and spell the pass's operative
  minimum IN the step — a summary the runner follows instead of the
  reference must carry the acts, not their names (field-measured: "singletons
  seed counters" alone produced a committed findings file and zero
  counters): *however step N's review loop ended — a clean round with no
  actionable findings, or its stop condition — run `/harden batch
  .harden/findings/<task>.jsonl` before opening the pull request: cluster
  the findings, count each class against `.harden/incidents/` topics, give
  every class that repeats or matches a topic the full harden cycle, run the
  mini-trace on any blocker even alone, and seed every remaining singleton
  class as `.harden/incidents/<topic>/` entry with verdict pending;
  artefacts, journal entries and the findings file itself land as commits
  on this same branch and ride into the pull request* — a findings file
  first committed after the merge stays behind on a dead branch
  (field-measured twice, two repos). Never anchor on a bare "the
  loop" — anaphora unbinds over distance: field-reviewed, a page below the
  loop's definition with other loops around, it pointed nowhere.

Audit first, as with any practice: if the host workflow already persists
findings its own way, their edition wins — report, do not duplicate.

The wire must be a commit in the host repo, not a working-tree edit:
autopilots open issues by resetting to a clean tree, so an uncommitted
routing line dies before any run re-reads the skill (field-measured: two
lines survived one stash cycle, then fell to a hard reset at the next
issue start — read zero times).

## 2. Collect by hand

Asked mid-conversation to capture findings from a review that was not wired:
write the same JSONL, one line per finding, `disposition: fixed|refuted`,
refutation reason in `note`. Then stop — collection is not the pass.

## 3. Run the pass

`/harden batch <findings.jsonl>` — clustering, the double recurrence count,
the ≥2-or-journal-match threshold, the promotions-not-prose expectation:
all of it is [harden's batch reference](../../references/batch.md), which is
the single source. This skill adds nothing to it on purpose.
