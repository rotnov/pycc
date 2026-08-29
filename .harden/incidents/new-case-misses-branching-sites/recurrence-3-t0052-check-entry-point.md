# Recurrence 3: T0052 tested through only one of its two entry points

**Date:** 2026-08-29
**Topic:** new-case-misses-branching-sites (see `incident.md` for the class
definition, the shipped `rule` artefact in
`.claude/skills/issue-to-plan/SKILL.md` step 3, and its `no baseline (twice)`
arena verdict — **do not re-run the arena**; this file only records a further
occurrence of the underlying class, per this journal's append-only
convention, and does not touch the fixture or the shipped artefact)
**Verdict:** no baseline, unchanged — this occurrence adds evidence about the
class's continued recurrence, not a new arena result
**Batch:** `.harden/findings/issue-676.jsonl`, finding 4 (test-coverage), round 1

## Symptom

`T0052` (cross-MRO attribute redeclaration, #676/D-210) is wired into both of
`pycc_types`'s public entry points, `check` and `check_and_resolve` — the
same dispatch shape `incident.md`'s class describes ("an existing document
already branches on" the general rule, here: every diagnostic is expected to
be exercised through both CLI-facing entry points it is reachable from). The
real-source end-to-end fixtures added alongside T0052
(`check_and_resolve_rejects_the_issue_676_*`) covered only `check_and_resolve`
(the `pycc build` path); no equivalent real-source fixture drove `check`
directly (the `pycc check` path) until the deep-reviewer's finding 4 caught
the gap.

## Disposition

Fixed on the branch: added
`check_rejects_the_issue_676_bool_reproduction_fixture_via_the_pycc_check_path`,
mirroring the existing `check_and_resolve` fixtures but calling `check(&hir)`
directly, per this crate's own established `parse` -> `lower_checked` ->
`check(&hir)` pattern used elsewhere in `crates/pycc_types/src/tests.rs`.

## Effect on the termination point

Confirms `incident.md`'s own root-cause framing directly: "the author
enumerates the sites the new case is *about* ... rather than every site that
already dispatches on the general rule." Here the "sites" are the two public
entry points a new diagnostic must be exercised through, not the plan-level
document sections the shipped rule (`issue-to-plan` step 3's affected-site
inventory) currently governs — this occurrence was not planned through
`issue-to-plan` at all (a single-file diagnostic addition fell under
AGENTS.md's "small, single-file, mechanically-scoped fix" carve-out), so the
shipped artefact had no opportunity to fire here one way or the other. This
recurrence is therefore evidence toward the class's persistence, not toward
or against the shipped rule's own unmeasured effectiveness — consistent with
`incident.md`'s "no baseline" verdict, which already anticipates that the
artefact's effect stays unmeasured until a future review catches (or fails to
catch) a case the rule should have prevented.

**fixture:** none — recurrence record only; the existing fixture under
`new-case-misses-branching-sites/fixture` is not touched (a fixture belongs
to the incident that built it, not to a later occurrence)
**artifact:** none new — see `incident.md`'s shipped `issue-to-plan` step 3
rule; this occurrence sat outside that rule's scope (see above)
**verify:** manual — `cargo test -p pycc_types --lib`, full workspace
`cargo test --workspace`, and `cargo llvm-cov --workspace
--fail-under-lines 100 --fail-under-regions 100` all passed after the new
test was added (see `.harden/findings/issue-676.jsonl` finding 4 for the
fix commit)
