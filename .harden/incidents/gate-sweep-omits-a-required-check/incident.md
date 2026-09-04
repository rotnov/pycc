---
id: gate-sweep-omits-a-required-check/incident
date: 2026-09-04
project: pycc
session: 2c68147a
trigger: self-post-failure
model: claude-opus-5
effort: high
harness: claude-code
type: process
termination: none — routed to the host journal
related: [coverage-claimed-without-executing-the-check, false-completion-of-list-task]
fixture: none — no artefact was built; see Artefact for why the D-192 bar routes this away from tracked work
artifact: none — build nothing, deliberately
verify: n/a. Nothing was shipped for this class.
verdict: pending
---

# Incident: the local gate sweep reported green without running a required gate

**Batch:** `.harden/findings/issue-918.jsonl`, class D.

## Symptom

The orchestrating session collected eleven gates to green on the implementation
commit `2934ea6a` (a pre-rebase object, rewritten as `d67d5088` on the pushed branch and unreachable from any ref in a fresh clone), listed each with an individually captured exit status, and
declared the change ready for review. `cargo fmt --all -- --check` was red at
that exact commit and was never in the list. Confirmed by re-running fmt in a
throwaway worktree pinned to that commit, so it was not an artefact of later
edits.

## Root cause

**Gap type: content.** `.claude/skills/issue-implement/SKILL.md` (~lines 206-222)
enumerates the pre-review gate set in prose, and that enumeration is provably a
strict subset of what CI requires: `ci.yml:1042-1059` defines a `rustfmt` job
that appears in `ci-gate`'s `needs:`, and the skill's prose does not name
`cargo fmt` at all. A gate that is never invoked cannot fail, so a
hand-assembled sweep produces exactly the same green shape whether it covers ten
of eleven gates or all eleven. The absence is invisible because nothing prints
when a command is not run.

## Termination point

`.claude/skills/issue-implement/SKILL.md`'s gate enumeration.

## Artefact

**None in this pass — build nothing, deliberately.**

The obvious rung is a runner plus a parity test: `scripts/run_local_gates.sh`
holding the gate set under `set -euo pipefail` with each verdict propagated, and
`scripts/test_run_local_gates.py` parsing `ci.yml`, reading `ci-gate`'s `needs:`,
and exiting non-zero on any required job absent from the runner. That test fails
today on `rustfmt`, needs zero `ci.yml` edits (so no D-080 digest staging), and
would live in the `scripts/` unittest suite the `governance` job already runs.

It is nonetheless **not filed as tracked work**, because AGENTS.md's filing bar
for process observations is not met. The bar requires that an observation can
(a) cause an incorrect merge decision or (b) hide a compiler defect. This one
does neither: the merge decision is made on CI, and CI runs `rustfmt` and blocks
the merge. What the omission produced was a false-green *local* report and a
wasted review round — real cost, no incorrect merge. AGENTS.md names the correct
home for exactly this shape: "a retrospective line, not an issue and not an
umbrella checklist item."

That line already exists: `docs/AGENT_RETROSPECTIVE.md`, entry
"2026-09-04 — Reported a gate sweep as complete while `cargo fmt` was never in
it", written when the failure was found. Filing it on the agent-tooling umbrella
(#806) would be the precise defect the bar exists to prevent.

Escalation trigger for a future pass: a second occurrence of a *different*
required gate missing from the same enumeration promotes this to the runner
above, since the enumeration would then be measured wrong twice rather than
incomplete once.

## Fixture

None — no artefact to test.

## Verify

`verify: n/a`. Nothing was shipped for this class.
