# Incident: test-seam-widens-public-api

**Date:** 2026-08-29
**Topic:** test-seam-widens-public-api
**Verdict:** pending (singleton — batch threshold not met, counter seed only)

## Symptom

One finding from `.harden/findings/issue-784.jsonl` (round 1, note): a new
module's injectable test seam (`sweep_stale_roots_in`, `SweepConfig`,
`SweepReport` in `crates/pycc_scratch`) was exported as `pub` although only
the crate's own unit tests consumed the seam — committed public API surface
with no external consumer. Fixed on the branch (`3df0ffac`) by narrowing
the two narrowable items to `pub(crate)` (the report type stays `pub` as a
public function's return type) and trimming the re-export.

## Root cause

A test seam reachable from the crate's own `#[cfg(test)]` modules needs no
visibility beyond `pub(crate)`, but `pub` is the reflexive choice when the
seam is written alongside a genuinely public sibling. Nothing mechanical
flags it: rustc's unused-visibility lints do not cover "public with zero
external consumers" across a workspace.

## Fixture / artifact / verify

None — threshold not met (class size 1, no journal match at recording
time). This entry is the counter: a second occurrence should escalate per
`references/rule-audit.md`'s recurrence check.
