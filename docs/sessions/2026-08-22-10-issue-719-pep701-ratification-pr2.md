# 2026-08-22-10 — issue #719 phase 2: PEP 701 row ratified

## Repository state at this checkpoint

- `origin/main` = `a8fce4f5ed1353a1e1e5bff071234dceeb589814`.
- One open pull request: [#731](https://github.com/rotnov/pycc/pull/731), head
  `4b8b3153`, not a draft, `MERGEABLE`. It is this checkpoint's own work and is
  **not yet merged** — everything below distinguishes what is on `main` from
  what is still in flight.

## Milestone status

v0.3's Accept criterion is ≥ 37 `docs/PYTHON_STANDARDS.md` matrix rows at `◐` or
better. On `main` today the count is **32 of 37**. PR #731, once merged, takes it
to **33 of 37**, leaving a 4-row gap. The milestone is **not met**; the autopilot
loop continues.

## What this checkpoint delivered

Issue [#719](https://github.com/rotnov/pycc/issues/719) (PEP 701, formalized
f-string grammar) was deliberately split into two pull requests because of how
[D-102](../decisions/D-102-extend-tests-conformance-rs-for-pr-9-s-9-new-pep.md)
phases its evidence: a matrix row flips only once its fixture has been observed
green on a real, **already-completed** CI run across all five Tier-1 targets in
both build profiles, recorded by hand after the fact. A same-pull-request flip
has no completed run to cite; adding a commit to insert one produces a new head
whose run is then itself uncited. The three prior hand-flips (PEPs 3135, 560, and
the earlier batches) are unanimous precedent for the split.

**Phase 1** — PR [#730](https://github.com/rotnov/pycc/pull/730), merged as
`a8fce4f5`. Authored `tests/fixtures/pep_0701_fstring_grammar.py` (85 lines) and
registered `pep_0701_fstring_grammar_matches_cpython_3_14_7_byte_for_byte` in
`tests/conformance.rs`. It carried no `Fixes #719`; `closingIssuesReferences`
confirmed `totalCount: 0`. It deliberately did not move the row counter.

**The ratifying evidence** — run
[32566309109](https://github.com/rotnov/pycc/actions/runs/32566309109), the
completed, fully successful `main` push run for `a8fce4f5`. Green on all five
Tier-1 targets: four from the `native-build-test` matrix
(`x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu`, `x86_64-apple-darwin`,
`x86_64-pc-windows-msvc`) and the fifth, `aarch64-apple-darwin`, from
`build-test-coverage` on `macos-14`. Both build profiles come from the single
registered test, which calls `run_conformance_fixture_with_profile` twice.

**Phase 2** — PR [#731](https://github.com/rotnov/pycc/pull/731), open at this
checkpoint, carrying the real `Fixes #719` (`closingIssuesReferences`
`totalCount: 1`, naming 719). Documentation and manifest only; no Rust source or
fixture changed. It flips the PEP 701 row to `◐`, flattens its fixture path from
the planned `py312/…` to the real flat path in the same commit (the guard test
resolves the cited path), records the hand-flip as policy rule 10, adds the
breadth-manifest row with 8 proven categories and 4 `core` gaps, and edits the
single `**Conformance progress**` roadmap headline in place. The `**Accept:**`
bullet is untouched: both its `≥ 37` and its `39 distinct PEP numbers` are
[D-153](../decisions/D-153-correct-v0-3-s-conformance-target-before-any-v0.md)
acceptance targets, not progress counts.

Because rule 10 shifted every matrix row by 24 lines, all 32 pre-existing
`matrix_line` values in the manifest were recomputed mechanically by importing
the checker's own `parse_matrix`, never by hand — the same method commit
`99681477` used.

## Verification

All gates ran captured to a log with their own exit status read, never through a
pipeline, and all returned 0: the breadth checker and its 54-test self-test, the
roadmap-evidence checker and its 220-run self-test, the full `scripts/` unittest
suite, the agent-asset and agent-policy validators, the conformance matrix guard,
`cargo test --workspace`, and clippy with warnings denied. `cargo llvm-cov` was
deliberately not run and the judgment stated rather than silently skipped: the
diff touches no Rust source or fixture, so coverage cannot move. PR #731's own CI
was green on every required check at head `44a4c507`.

The pinned reviewer (`ievo@ievo-skills` 0.78.8 `deep-reviewer`) raised four
findings on `44a4c507`. Three were real accuracy defects in descriptive text and
were fixed in `4b8b3153`: two manifest `proven` category labels described
constructs the fixture does not contain (both multi-line cases use triple-quoted
f-strings, and `implicit_concat` joins two f-strings rather than an f-string and
a plain string literal), and the roadmap's gap sentence named three `core` gaps
with definite-article phrasing while the manifest records four. The fourth was
the reviewer disclosing its own lack of git access — no action.

The authoring host has CPython 3.14.6 while `oracle_python_bin()` hard-asserts
3.14.7, so the fixture test stays `#[ignore]`d locally. The function was not
weakened; the pinned-oracle observation comes only from CI.

## Follow-ups opened or still open

- [#729](https://github.com/rotnov/pycc/issues/729) — P3, decompose
  `tests/conformance.rs` (1304 lines) per the ~1,000-line threshold. Records why
  the obvious extraction is CI-red.
- [#720](https://github.com/rotnov/pycc/issues/720) — the `=` debug specifier
  divergence, one of the PEP 701 row's recorded `core` gaps.
- [#698](https://github.com/rotnov/pycc/issues/698) — no longer on v0.3's
  critical path for the row count (#719 supplied the fifth row), but still the
  only systematic assessment of the 53 never-evaluated `☐` rows.

## Paused autopilot

The standing directive is `/next-milestone` with no arguments, active milestone
**v0.3**, handed off to `.claude/skills/issue-select/SKILL.md`'s loop. Last
iteration's outcome: #719 delivered in two pull requests, phase 2 open and green
but not yet merged.

**Next step for a resuming session:** confirm #731 merged (and #719 closed by it),
then re-run the milestone evidence check — 33 of 37 is not met, so re-enter
`issue-select` at step 1 with a fresh baseline.

**This run's denylist**, which must carry across the session boundary: `#20`,
`#631`, `#604`, `#558`. #604's original stop reason was not recovered across a
context boundary and is recorded as unrecovered rather than reconstructed.

**First item of the next iteration:** within-scope starvation.
[D-191](../decisions/D-191-milestone-membership-ranks-first-in-issue-select.md) fixed cross-scope starvation, but inside the scope the
ordering is still priority-marker-first, and v0.3's critical path
#541 → #703 → #542 → #543 — which carries exactly the four rows that take 33 to
37 — is unmarked and therefore sorts below roughly 22 in-scope P2s.
