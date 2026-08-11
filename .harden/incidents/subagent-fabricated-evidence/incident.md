# Incident: subagent-fabricated-evidence

**Date:** 2026-08-11
**Topic:** subagent-fabricated-evidence
**Verdict:** shipped (manual verify)

## Symptom

During the ci-temporary-bypass flow for PR #458, Gate 1 dispatched a fresh
`subagent_general` to adversarially verify whether relaxing the `audit`
check was justified. The subagent returned REFUTED, citing:

> A `[ci-bypass]`-prefixed incident issue is already open. The `gh issue
> list` output shows issue #459 with title "[ci-bypass] audit relaxed" in
> OPEN state.

Direct verification (`gh issue view 459 --json state`) confirmed issue
#459 was **CLOSED**, not OPEN. The subagent fabricated or misread the CLI
output. Had the REFUTED verdict been accepted, the ci-temporary-bypass
fail-closed design would have blocked a legitimate merge based on
non-existent state.

## Root cause

The ci-temporary-bypass skill's Gate 1 step instructs the caller to treat
any non-CONFIRMED verdict as REFUTED and stop. It does not instruct the
caller to independently verify factual claims that drive the REFUTED
decision. A subagent running in a fresh context with no access to the
caller's conversation can hallucinate or misread command output, producing
a REFUTED verdict based on fabricated evidence.

## Termination point

`Local-skill`: `.claude/skills/ci-temporary-bypass/SKILL.md`, Gate 1
section, "On REFUTED" paragraph.

## Artefact

**Type:** rule (local-skill edit)
**File:** `.claude/skills/ci-temporary-bypass/SKILL.md`
**Change:** Added a verification step between Gate 1's REFUTED verdict
and the caller's acceptance. The caller must independently re-run any
command the subagent cited before accepting a REFUTED verdict. If the
claim doesn't hold, discard the REFUTED verdict and re-evaluate inline.

## Fixture

`.harden/incidents/subagent-fabricated-evidence/fixture/`
- `task.md`: presents a REFUTED verdict citing a fabricated open issue
- `control.md`: skill without the verification step
- `patch.md`: skill with the verification step
- `verify.py`: checks for evidence of independent verification

## Arena verdict

**Diverges across harnesses.** devin=profit (0/3 → 1/1), claude=harm,
codex=zero, grok=excluded. Cause understood: sandbox lacks `gh`, so
agents cannot execute the verification command. The agents correctly
recognize the need to verify (per judge notes) but can't execute in the
sandbox. verify=manual for this limitation.

## Verify

`verify: manual` — the arena cannot properly test command-based
verification in a sandbox without `gh`. In the real incident, the
verification step (running `gh issue view 459 --json state`) immediately
exposed the fabricated claim. The rule's effectiveness depends on the
caller having access to the cited command, which is the normal operating
environment for ci-temporary-bypass.
