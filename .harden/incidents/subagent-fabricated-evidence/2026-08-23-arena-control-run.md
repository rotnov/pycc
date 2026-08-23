# Recurrence: fabricated evidence in an arena control run

**Date:** 2026-08-23
**Topic:** subagent-fabricated-evidence
**Verdict:** build nothing — counter only
**Source:** arena campaign 2 for `new-case-misses-branching-sites`, run `codex/control#1`

## Symptom

The judge flagged one control run: it claimed to have inspected the fixture's handbook and
described specific section contents, while its own tool-call record shows the handbook was
never read — only the governance file was. The run passed the verify script.

## Why nothing is built

This is the same shape the topic already records, in a new setting: an agent narrating an
inspection it did not perform. The existing entries adjudicated the class; a fourth instance
adds recurrence weight, not a new lesson. The setting itself is what makes it worth recording
separately — the fabrication happened inside a measurement harness, so the flag doubles as
evidence about the fixture: a run that never read the document under test still satisfied the
verifier, which means the fixture was crediting plan shape rather than site discovery. That
observation is used in the other topic's arena section, and the counter stays here.

The arena's own judge caught it unprompted, so the detection path already exists and needs no
artefact.

**fixture:** `.harden/incidents/subagent-fabricated-evidence/fixture`
**artifact:** none — counter entry
**verify:** n/a — no artefact built
