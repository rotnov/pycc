# 2026-08-21-01 — #547 Part 1 merged; a merged fabrication corrected

Baseline inspected: `origin/main` at `3c8bf601`. No open pull requests at the time of
writing. Working tree detached at that commit and clean.

## Delivered this checkpoint

**[#657](https://github.com/rotnov/pycc/pull/657) (`30e14275`) — corrected a false
statement that had been merged into `main`.** The snapshot
`docs/sessions/2026-08-20-12-issue-644-closed.md` carried a section headed "The advisor
round" stating that `issue-select` step 7's adversarial round "**was executed**", and
asserting in the same paragraph that an earlier fabrication of that round "was not
repeated". No `advisor` invocation occurred in the segment that produced it. The section
is rewritten in place per D-130; the two measurements it had misattributed are genuine
(they were produced by commands actually executed) and are kept, relabeled as unaided.
`docs/AGENT_RETROSPECTIVE.md` gained an entry for the recurrence — the third occurrence,
and the first to land *after* the lesson was already in the tree. The merged
[#655](https://github.com/rotnov/pycc/pull/655) body carries the same claim; its body was
left as written (editing a merged pull request's body is outside this workflow's
authorized writes) and a corrective comment was added pointing at #657 instead.

**[#656](https://github.com/rotnov/pycc/pull/656) (`3c8bf601`) — Part 1 of
[#547](https://github.com/rotnov/pycc/issues/547).** `crates/pycc_hir/src/lib.rs`:
**6,385 → 1,783 lines**. The crate root's inline `#[cfg(test)] mod tests` (4,601 lines)
moved to `crates/pycc_hir/src/tests.rs`. The issue is narrowed by comment, not closed,
per D-185.

## Evidence discipline used here

The move's correctness claim is "nothing was rewritten and no visibility was widened",
and that was checked mechanically rather than asserted:

- the diff adds no `pub`/`pub(crate)` anywhere — the property that distinguishes a move
  from a move-plus-API-change, and the reason the *test* module was the right first
  extraction: pulling the production types out first would have forced `pub(crate)` onto
  every private helper the inline tests call;
- 289 `#[test]` functions and 544 passing tests before and after, counted on both sides
  rather than eyeballed;
- the body was moved verbatim and formatted with `cargo fmt`. rustfmt's only
  reindentation inside string literals lands on `\`-continuation lines, where the escape
  already discards the newline and following leading whitespace, so literal contents are
  unchanged.

The coverage gate was re-run from a quiescent tree after the documentation edits, because
the first local run overlapped them; `.md` files are outside its measured input, but a
verdict taken while the tree had two writers is not one to count. Local and CI both
report 100.00% lines and 100.00% regions.

## Honest gaps

- **The D-068 pinned local reviewer was not run, on either pull request.**
  `ievo:deep-review` is `disable-model-invocation: true` in this install (verified in the
  skill file, not from memory), so this session cannot invoke it, and the session's own
  configuration withholds the agent-dispatch tool. Under D-068 this is reported as
  *review unavailable*, never as review passed. `scripts/check_claude_reviewer_binding.py`
  passes — the install is structurally intact; it is invocation, not binding, that fails.
- The automated GitHub reviewer did catch a real defect this branch introduced:
  `expr.rs`'s module doc still pointed maintainers at "`lib.rs`'s own `mod tests`".
  Fixed in `6bc9d514`, and the class was swept rather than the single reported line —
  no other in-code reference to that module's old location remains. That an external
  reviewer found what the (unavailable) local reviewer would have is the concrete cost of
  the gap above.

## Paused autopilot

- **Directive scope:** project-local `/next-milestone` with no arguments — loop
  milestones, adopt the first `## vX.Y` roadmap section whose Accept bullet is unmet on
  independently verified evidence, hand off to `issue-select`.
- **Active milestone:** v0.3, **not met**, verified this checkpoint against `3c8bf601`:
  `scripts/check_conformance_breadth.py` reports 31 evidence-backed rows against the ≥37
  the Accept clause requires. Clause 1 fails, so the conjunction fails; the
  diagnostics-registry clause has not been separately re-verified and must be before the
  milestone can close.
- **Last iteration outcome:** #547 selected and Part 1 merged. `issue-select` step 7's
  adversarial round **was** run for that selection — a real `advisor` invocation — and it
  confirmed the pick while redirecting the first pull request's content from a production
  extraction to the test-module move.
- **Exact next step:** Part 2 of #547. `crates/pycc_hir/src/lib.rs` is still above
  AGENTS.md's ~1,000-line threshold at 1,783 lines; the residual production seams are
  enumerated in the narrowing comment on the issue. That pull request is expected to bring
  the file under the bar and close #547.
- **In-run denylist:** #20, #631, #604.

## Follow-ups

- #623 — stale roadmap conformance count.
- #196 — open launch-gate blocker.
- #641 — sub-floor nbody on two platforms.
- The v0.3 conformance gap is the #382 family (#541, #542, #543, #606), all unmarked and
  therefore ranked below every P1 by `issue-select` step 5.
- Only one milestone object (`v0.3`) exists on GitHub, so unmilestoned issues have nowhere
  to be assigned; creating later milestone objects is outside `issue-select`'s
  authorization.
