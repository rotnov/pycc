# 2026-08-22-05 — Codex sub-agent mapping merged; autopilot paused mid-loop

## Overall status

Default branch tip: `085dc859e80c37a2190fdaa90209757d3d4d707b`. Zero open pull requests.

Milestone **v0.3 is not met**. `python3 scripts/check_conformance_breadth.py` reports
`32 evidence-backed rows, all declared (2 accepted as whole-PEP, 30 subset)` against the
Accept threshold of 37, leaving a 5-row gap. The `issue-select` loop therefore continues
rather than handing off to `next-milestone`'s completion step.

## Delivered since the previous checkpoint

Two pull requests merged, both from this session.

**[#721](https://github.com/rotnov/pycc/pull/721)** — `6f262be1`, Part 4 of
[#544](https://github.com/rotnov/pycc/issues/544): extracted expression inference into
`crates/pycc_types/src/expr.rs`. #544 stays open per D-185 and was narrowed by a comment.
`crates/pycc_types/src/lib.rs` is now 3615 lines, still over the ~1,000-line threshold, and
`tests.rs` is 25561 lines (tracked separately by
[#695](https://github.com/rotnov/pycc/issues/695)).

**[#722](https://github.com/rotnov/pycc/pull/722)** — `085dc859`, part of
[#53](https://github.com/rotnov/pycc/issues/53): mapped Claude's `Agent` tool onto Codex's
`spawn_agent`/`wait_agent` in the `improve-codebase-architecture` and `codebase-design` Codex
adapters, with a mandatory named fallback for genuine unavailability. Added the opt-in canonical
frontmatter marker `requires-agent-dispatch: true`, gating a new branch in `validate_skill_parity`
with failure-path coverage (`scripts/test_validate_agent_assets.py`, 138 → 142 tests). Recorded as
`D-190`. `closingIssuesReferences.totalCount` was confirmed `0` before merge.

## What a fresh session most needs to know

**#53 is narrowed, not finished, and the narrowing is the interesting part.** Four of its five
completion criteria are delivered. The third — an isolated Codex behavioral smoke driving a live
client — is **not reachable with this repository's test surface**, and the evidence is worth not
re-deriving: `scripts/run_alpha_skill_evals.py` is an offline deterministic-predicate runner. Its
own `--client` help states it "does not launch the named client or a language model"; its module
docstring says the checks run "without invoking an LLM"; every skill-adapter runner has signature
`(case, skill_text) -> None`; and the only `subprocess.run` path is the `run_pycc_*` family, which
executes the compiler. This is the same blocker class already recorded against
[#259](https://github.com/rotnov/pycc/issues/259). #53 stays open tracking that criterion alone.

**Two structural invariants constrain any future adapter work**, both enforced today and both easy
to break by accident:

- a Codex adapter must contain **exactly one** backtick-quoted `.claude/skills/<name>/SKILL.md`
  reference matching its own skill name — enforced twice, by `validate_skill_parity`'s
  `references != [(expected, name)]` and by `CANONICAL_REFERENCE` in `run_alpha_skill_evals.py`;
- `canonical_skill()` in `run_alpha_skill_evals.py` resolves a Codex adapter only to follow that
  pointer and then returns the **Claude** file's text. The dual-client eval runs therefore prove
  pointer integrity, **not** adapter-behavior equivalence. Adapter body contracts belong in
  `validate_agent_assets.py`, which does read the adapter's own text.

**A correction worth carrying forward:** the premise that Codex exposes no sub-agent capability is
false. `codex features list` reports `multi_agent` as `stable` and enabled. This was observed on the
locally installed Codex **0.148.0** while CI pins `CODEX_CLI_VERSION: "0.145.0"`; no artifact claims
confirmation at the pinned version, and the mandatory fallback is what keeps the adapters correct
across that gap.

**[D-103 is superseded by D-172.](../decisions/README.md)** An earlier belief that manifest
membership automatically implies a two-PR stage-then-activate cycle is wrong. A manifest hit still
routes a PR through the stricter base-owned audit path, so it remains a deprioritizer, not a hard
tax. `.claude/skills/issue-implement/SKILL.md` and `.claude/skills/issue-select/SKILL.md` still
describe the retired cycle as live policy and need correcting; `AGENTS.md` is already updated.

## Paused autopilot

The standing directive is `/next-milestone` invoked with no arguments, which adopted **v0.3** and
handed off to `issue-select`'s loop. The loop is **paused mid-iteration**, not terminated.

- **Directive scope:** work v0.3 to completion, then record `Update (<date>): met.`, update
  `README.md`, close the milestone, dispatch `hook-install-check.yml`, record the Tier-1 run in
  `docs/DISTRIBUTION.md`, and tag `v0.3.0`.
- **Active milestone:** v0.3, **not met** — 32 of 37 rows at `085dc859`.
- **Last iteration's outcome:** #53 selected, narrowed, implemented, merged as `085dc859`.
- **Next step:** re-enter `issue-select` step 1 with a fresh baseline from `085dc859`.
- **In-run denylist, which must carry across the session boundary:** `#20` and `#631`
  (deprioritized per #20's own last comment), and `#604` — whose original stop reason was **not
  recovered** across a context boundary and is recorded as unrecovered rather than reconstructed.

## Known follow-ups

- The five-row v0.3 gap: four rows come from [#542](https://github.com/rotnov/pycc/issues/542) and
  [#543](https://github.com/rotnov/pycc/issues/543), both gated on
  [#541](https://github.com/rotnov/pycc/issues/541); the fifth is sourced by
  [#698](https://github.com/rotnov/pycc/issues/698)'s ranked shortlist.
- `validate_skill_parity`'s two conditional branches both match literals anywhere in the document,
  so an adapter can satisfy them while asserting the opposite. Requiring the literals to co-occur
  within one paragraph would close it for both branches at once. Not a regression — the new branch
  copies the pre-existing technique deliberately.
- The deprioritization of [#44](https://github.com/rotnov/pycc/issues/44),
  [#45](https://github.com/rotnov/pycc/issues/45), [#82](https://github.com/rotnov/pycc/issues/82),
  [#275](https://github.com/rotnov/pycc/issues/275) and
  [#558](https://github.com/rotnov/pycc/issues/558) rested on the D-103 belief corrected above and
  should be re-evaluated.
- `black --check` fails on the pristine tree, independent of any change; no workflow invokes it.
