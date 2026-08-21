# 2026-08-21-10 — #544 Part 2 merged; autopilot paused

## Where the tree is

- Default branch tip: `e02b5bc4` — "refactor(types): extract the generic-monomorphization seam
  into monomorphize.rs (Part 2 of #544) (#684)".
- **Zero open pull requests.**
- `tests/fixtures/policy-successor-manifest.json`: 49 targets, **0 mid-transition** — no D-103
  staging cycle is in flight, so no candidate pull request inherits an `audit` block.
- Branch protection matches the documented baseline (strict, contexts `audit` + `ci-gate`,
  `enforce_admins` true, 0 required approvals, conversation resolution required, no force pushes
  or deletions). No `[ci-bypass]` incident is open.

Every fact above was re-resolved against the live remote immediately before this file was
committed, per D-130.

## What this iteration delivered

**PR #684, merged as `e02b5bc4`** — Part 2 of the D-185 decomposition of
`crates/pycc_types/src/lib.rs`. The generic-monomorphization seam moved into a new sibling
`crates/pycc_types/src/monomorphize.rs`. `lib.rs`: **9,035 → 6,485** lines; the new file is 2,604.

Three things in this round are worth carrying forward as method, not just as outcome:

1. **The published seam table was wrong at one end, and re-deriving it caught that.** Part 1's
   comment gave the monomorphization seam as roughly 5983–8539. The lower bound held; the upper
   one landed *inside* the doc comment for `enum_member_attr_type`, an enum-lowering helper that
   merely mentions `monomorphize`. The real end is `fn monomorphize`'s closing brace at 8522. A
   seam table is a starting point for the next extraction, never a boundary to trust.
2. **Part 1's coverage argument does not transfer, and this was checked rather than assumed.**
   Part 1 was safe because `tests.rs` is `#[cfg(test)]` and outside the coverage denominator.
   `monomorphize.rs` is production code inside it, and region coverage for generic functions
   depends on which instantiations get emitted. The gate was run: 100.00% lines / 100.00% regions,
   with the region total identical to the pre-move baseline and lines `+9`, fully accounted for by
   three rustfmt signature reflows. CI's own `build-test-coverage` confirmed it independently.
3. **Relocation purity was proven by byte diff, not asserted.** Diffing the moved range in the
   merge base against the new file's body yields exactly 12 mechanical hunks: 11 `fn` →
   `pub(crate) fn` widenings and one `class::` → `crate::class::` path qualification. This is the
   evidence D-185's "no unrelated logic rewritten" property needs, and it is cheap enough to
   produce for every future part.

CI: 14 checks passed, 2 correctly skipped (the two `pages-*` gates, no site change). No review
threads were opened. The pinned reviewer (`ievo@ievo-skills 0.78.8`) returned clean on 11 of 12
checklist points with one `note`-severity finding, verified pre-existing and routed to #685.

## Issues touched

- **#544** — stays open, narrowed by comment with a re-anchored residual-seam table derived on
  `e02b5bc4`. Next part's likeliest target is the constraint solver (~1,900 lines from 988);
  note that a `solver.rs` already exists, so it needs a distinct module name.
- **#685** — filed: an orphaned doc-comment run leaves `monomorphize` undocumented. Pre-existing
  and byte-identical at `6d47d24a`, so it is not a defect of #684. Left unassigned to any
  milestone, with that reasoning stated in the body: it advances no v0.3 **Accept:** criterion.
- **#150** — assigned to the v0.3 milestone during `issue-select`'s inventory pass. It was the
  only one of 60 unmilestoned non-P1 issues with a clear fit; the rest fall into the
  CI-governance, agent-tooling, website, and decomposition families AGENTS.md names as
  cross-cutting.

## Milestone status

**v0.3 is not met.** `docs/ROADMAP.md`'s progress line still reads 29 of the required 37
`PYTHON_STANDARDS.md` matrix rows at `◐` or better; the 8-row gap is tracked by #572 and its parts
#578 / #579 / #580. There is no `Update (<date>): met.` note. The diagnostics-registry and
`pycc explain` conjuncts of the **Accept:** bullet have not been separately re-verified this
session.

## Paused autopilot

- **Directive:** project-local `/next-milestone` with no arguments, delegating to `issue-select`'s
  loop.
- **Active milestone:** v0.3, not met (above).
- **Last iteration's outcome:** #544 Part 2 merged as `e02b5bc4`; issue narrowed, not closed.
- **Next step:** re-enter `issue-select` step 1 from `e02b5bc4`.
- **In-run denylist that must carry forward: #20, #631, #604.**

## Standing structural finding, not acted on

`issue-select` step 5 ranks the priority marker first and treats active-milestone membership only
as an intra-tier tie-break. Every unmilestoned P1 therefore outranks every v0.3-milestoned P2 — and
the conformance rows that would close v0.3's 8-row gap live only in the latter. The loop, run as
written, cannot reach v0.3. This is recorded here as a `/harden` or ADR candidate for the third
consecutive iteration. It has again **not** been used to override the written ordering: overriding
a written scoring rule on an advisor's say-so is the move this project already rejected once, and
the fix belongs in the skill or a decision record, not in an individual run's judgment.
