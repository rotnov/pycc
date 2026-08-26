# Session handoff: issue #803 — renumber duplicate D-201/D-202 and add a uniqueness gate

## Status

Implemented against `origin/main` at `ff1d952d0988d7af0c421891b9814b154abbd154`
on branch `fix/issue-803-decision-renumber`, milestone v0.3. This entry
lands with this issue's own merge (D-192); issue #803 closes with it.

## What happened

PR #780's merge (`587c8af1`, 2026-08-26) landed two new decision files whose
`D-NNN` numeric prefixes collided with two pre-existing, already-accepted
decisions on `main`:

- `docs/decisions/D-201-optional-t-flow-sensitive-narrowing-part2.md` (new,
  from #780) collided with the pre-existing
  `docs/decisions/D-201-shared-pycc-scratch-crate-and-lint-gate-for.md`
  (from commit `03a0efd4`, issue #781).
- `docs/decisions/D-202-kill-prescan-for-re-enterable-narrowed-bodies.md`
  (new, from #780) collided with the pre-existing
  `docs/decisions/D-202-pep-654-except-star-and-exceptiongroup.md` (from
  commit `bd503a78`, issue #542).

`docs/decisions/README.md` regenerated green with both duplicate pairs
because `scripts/generate_decisions_index.py` indexed by filename with no
uniqueness check on the numeric prefix.

## Renumbering

The two pre-existing files keep their numbers unchanged. The two newer
(#780) files were renamed to the next free numbers — D-203 was already
claimed by issue #800's work, so:

- `D-201-optional-t-flow-sensitive-narrowing-part2.md` → `D-204-...md`
  (`Optional[T]` flow-sensitive narrowing, Part 2 of #747)
- `D-202-kill-prescan-for-re-enterable-narrowed-bodies.md` → `D-205-...md`
  (kill-prescan for re-enterable narrowed bodies)

Both files' YAML `id:` frontmatter and `## D-20N: ...` heading were updated
to match the new filename number.

## Classifying every in-repo `D-201`/`D-202` reference

`D-201` and `D-202` are heavily overloaded strings in this repository (the
pycc_scratch and PEP 654 decisions are both cited across many source files,
tests, and docs). Every hit from `grep -rn 'D-201\b'` / `'D-202\b'` across
the tree was read in context individually before deciding whether to
change it. Renumbered (narrowing/kill-prescan → D-204/D-205):

- `crates/pycc_types/src/narrow.rs`, `crates/pycc_types/src/tests.rs` (5
  narrowing-related occurrences, one kill-prescan occurrence),
  `crates/pycc_mir/src/stmt.rs`, `crates/pycc_hir/src/lib.rs` (one
  narrowing self-reference, one explicit kill-prescan filename reference).
- `tests/fixtures/conformance-breadth-manifest.json`,
  `tests/fixtures/pep_0604_union.py` (all `D-201, #769` narrowing citations).
- `docs/PYTHON_STANDARDS.md`, `docs/TYPE_SYSTEM.md` (both the D-201
  narrowing citations and the one D-202 kill-prescan markdown link),
  `docs/ROADMAP.md` (the one narrowing-Part-2 paragraph).

Left unchanged (pre-existing decisions, correctly still D-201/D-202):

- `docs/ARCHITECTURE.md`, `scripts/check_scratch_dir_usage.py`,
  `docs/decisions/D-203-...md`, `docs/decisions/D-085-...md` — all cite
  D-201 as the shared `pycc_scratch` crate/lint-gate decision.
- `crates/pycc_types/src/exception*.rs`, `crates/pycc_mir/src/exception.rs`,
  `crates/pycc_hir/src/exception.rs`, `crates/pycc_hir/src/class.rs`,
  `crates/pycc_codegen/src/{lib,exception,tests}.rs`,
  `crates/pycc_rt/src/exception.rs`, `tests/issue_542_except_star.rs`,
  `tests/issue_702_user_exceptions.rs`, `tests/conformance.rs`,
  `docs/RUNTIME.md`, and four separate `D-202` mentions in `docs/ROADMAP.md`
  — all cite D-202 as the PEP 654 `except*`/`ExceptionGroup` decision.
- Representative ambiguous case resolved by reading, not guessing:
  `crates/pycc_types/src/tests.rs` has both narrowing-D-201 test comments
  *and* one kill-prescan-D-202 test comment
  (`a_match_capture_pattern_that_reuses_a_narrowed_name_inside_a_while_loop_is_rejected`,
  "reproducing D-202's own loop-reentry counterexample") in the same file;
  each occurrence was renumbered independently by line, not by a blanket
  file-wide substitution.

## Session-log correction

`docs/AGENT_RETROSPECTIVE.md` (a D-066 journal, normally reviewed for
factual accuracy but not treated as a policy document) had two `D-201`
citations and one `D-202` citation in its 2026-08-25 entry about the
three-round D-068 review of #780's narrowing feature. All three
unambiguously cite the renumbered decisions (the narrowing feature itself,
and the kill-prescan fix that closed round 3) rather than the pre-existing
pycc_scratch/except-star decisions, so they were corrected to D-204/D-205
as a deliberate factual fix — the entry's own historical lesson content is
unchanged, only the now-stale decision numbers it cites.

No other `docs/sessions/*.md` file was touched. The ones that mention
D-201/D-202 (`2026-08-25-02-issue-781-scratch-dir-abstraction.md`,
`2026-08-26-07-issue-782-batch-b-root-crate.md`,
`2026-08-26-08-issue-782-batch-c-tests.md`,
`2026-08-26-04-issue-800-pr1-d203-checker.md`,
`2026-08-26-05-issue-800-pr2-d203-activation.md`) either describe the
pre-existing pycc_scratch decision or describe the *duplicate pair itself*
as an open defect (issue #803) — both are accurate as written and were left
alone as historical records per AGENTS.md.

## Uniqueness gate

`scripts/generate_decisions_index.py` gained `check_unique_ids(entries)`,
called from `generate_index()` before the table is built, raising
`ValueError` on any two files claiming the same `D-NNN` id. `main()` now
catches that `ValueError` and returns exit code 1 (printing the message to
stderr) in both plain-generate and `--check` mode, so a future filename
collision fails closed instead of silently emitting a README with two rows
for the same number. `scripts/test_generate_decisions_index.py` gained four
tests: `generate_index` raising on a constructed duplicate-id fixture, and
`main()` returning 1 (with no README written) in both modes on the same
fixture.

## Gates (all green at this snapshot, macOS local run)

- `python3 -B -m unittest discover -s scripts -p 'test_*.py'`: 958 tests
  (was 954), OK (skipped=6), ~48 s — includes the four new tests.
- `python3 scripts/generate_decisions_index.py docs/decisions
  docs/decisions/README.md` then `--check`: regenerated clean, then
  confirmed up to date.
- `cargo check --workspace`: clean (comment-only Rust changes).
- This change touches no Rust code behavior, only documentation and a
  Python tooling script — the D-014 100% Rust coverage gate
  (`cargo llvm-cov ... --fail-under-lines 100 --fail-under-regions 100`) is
  not implicated and was deliberately not run locally to avoid wasted time;
  it still runs in CI for the whole workspace and must stay green there
  because nothing in this change alters compiled behavior.
- D-068 pinned local reviewer (`ievo` `deep-reviewer`) run against the
  working-tree diff before opening the PR; actionable findings addressed
  before merge (see PR body/thread for specifics).

## Where to resume

Nothing pending from this issue. If a future decision renumbering is
needed, `check_unique_ids` in `scripts/generate_decisions_index.py` is now
the enforcement point — extend its tests alongside any change there.
