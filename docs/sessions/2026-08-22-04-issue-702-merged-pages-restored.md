# 2026-08-22-04 — #702 merged, Pages restored, autopilot paused before #698

## Overall status

`main` is at `06ff4c0f`. No pull request is open. Milestone v0.3 is **not met**:
`python3 -B scripts/check_conformance_breadth.py` at `06ff4c0f` reports
`conformance breadth: 32 evidence-backed rows, all declared (2 accepted as
whole-PEP, 30 subset)`, rc=0 — 32 of the 37 rows `docs/ROADMAP.md`'s v0.3
**Accept:** bullet (line 181) requires.

`tests/fixtures/policy-successor-manifest.json` is steady-state at that tip:
49 targets, none mid-transition, so no run-wide `audit` block exists.

## Delivered since the 2026-08-22-03 snapshot

- **[#715](https://github.com/rotnov/pycc/pull/715) merged** as `3b976ea8`
  (Part 2 of #541, closing [#702](https://github.com/rotnov/pycc/issues/702)).
  #541 stays open for Part 3, [#703](https://github.com/rotnov/pycc/issues/703),
  whose blocker is therefore now cleared.
- **[#717](https://github.com/rotnov/pycc/pull/717) merged** as `06ff4c0f`,
  closing [#716](https://github.com/rotnov/pycc/issues/716) — the `Pages` `build`
  failure #710 introduced by editing `site/status/index.html` without advancing
  `site/sitemap.xml`'s `<lastmod>` for `/status/`.

The Pages fix is confirmed by observation, not inference: the post-merge run for
`06ff4c0f` reports `Pages: completed success`, alongside `Main history audit:
completed success` and `Status page freshness: completed success`. That
confirmation mattered because the squash commit itself modifies
`site/status/index.html`, so it redefines the date the freshness check compares
against — no pre-merge run could settle it. See the retrospective entry of the
same date for the trap this exposed.

## Paused autopilot

The standing directive is `/next-milestone` with no arguments, delegating to
`.claude/skills/issue-select/SKILL.md`. The loop is **paused**, not terminated.

- **Active milestone:** v0.3, Accept criteria not met (32 of 37 rows).
- **Last iteration's outcome:** #702 implemented, merged, and its follow-on
  Pages regression filed and fixed.
- **Next step:** `/issue-implement` on
  [#698](https://github.com/rotnov/pycc/issues/698), selected by the full
  `issue-select` pass and confirmed through the mandatory step-7 adversarial
  advisor round. #698 is `P1:` and in v0.3; the only other v0.3 `P1` is #20,
  which is denylisted. #703 is unmarked and therefore ranks below it under step
  5's fixed marker-first ordering, despite sitting on the critical path for four
  of the five remaining rows.
- **In-run denylist (carries forward across this session boundary):** `#20` and
  `#631` — deprioritized per #20's own most recent comment; `#604` — its original
  stop reason was not recovered across a context boundary and is recorded as
  unrecovered rather than reconstructed.

Note that #698 delivers an assessment and one filed implementation issue, not a
conformance row. The per-cycle evidence check will still read 32 of 37 after it
closes; that is the expected outcome, not a failure.

## Where the remaining five rows come from

- Four are tracked: [#542](https://github.com/rotnov/pycc/issues/542) (PEP 654)
  and [#543](https://github.com/rotnov/pycc/issues/543) (PEPs 3151, 765, 758),
  both gated on #541.
- The fifth has no tracker; sourcing it is exactly what #698 exists to do.

## Environmental test failures (reproduce at an untouched base)

- ~48 failures across `tests/conformance.rs` and `tests/nbody_bench.rs`, all
  reading `conformance oracle must be exactly Python 3.14.7, found
  "Python 3.14.6"`.
- Exactly two in `scripts/test_check_pages_performance_budget.rb`:
  `test_resource_budget_fails_when_unexpected_image_added` and
  `test_resource_budget_fails_when_image_added_in_subdirectory`.

Neither class is attributable to any diff; do not spend a session chasing them
without first reproducing against a clean base.
