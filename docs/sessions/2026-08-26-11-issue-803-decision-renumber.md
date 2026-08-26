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
claimed by issue #800's work, so the first pass (commit `b47821e8`)
renumbered them to D-204/D-205.

Before this PR reached merge, `main` independently gained its own new
D-204 (PR #812, "Widen Optional[T] codegen to float and bool inner
types") — a second, live instance of the exact defect class #803 exists
to close, discovered only because this PR's own CI run against the
updated `main` tip surfaced a real merge conflict on
`docs/decisions/README.md`/`docs/decisions/D-204-*.md`, not by any
proactive check (there is no cross-branch reservation mechanism for
in-flight D-NNN numbers; `check_unique_ids` only fires once both files
land on the same tree). The two files below were therefore renumbered a
second time to the next free numbers as of the rebase onto that
newer `main`:

- `D-201-optional-t-flow-sensitive-narrowing-part2.md` → `D-205-...md`
  (`Optional[T]` flow-sensitive narrowing, Part 2 of #747)
- `D-202-kill-prescan-for-re-enterable-narrowed-bodies.md` → `D-206-...md`
  (kill-prescan for re-enterable narrowed bodies)

Both files' YAML `id:` frontmatter and `## D-20N: ...` heading were updated
to match the new filename number.

## Classifying every in-repo `D-201`/`D-202` reference

`D-201` and `D-202` are heavily overloaded strings in this repository (the
pycc_scratch and PEP 654 decisions are both cited across many source files,
tests, and docs). Every hit from `grep -rn 'D-201\b'` / `'D-202\b'` across
the tree was read in context individually before deciding whether to
change it. Renumbered (narrowing/kill-prescan → D-205/D-206):

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
pycc_scratch/except-star decisions, so they were corrected to D-205/D-206
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

## Second collision and a further gate: filename-vs-frontmatter mismatch

While this PR's CI was running against the rebased `main` tip, an
external `chatgpt-codex-connector` review found a second, independent gap
in `check_unique_ids`: it dedups only by frontmatter `id`, so a file whose
own `D-NNN` filename prefix disagrees with its frontmatter `id` (e.g. a
file named `D-201-....md` whose frontmatter claims `id: D-999`) would
pass silently, recreating the same ambiguous-numbering defect at a
different layer. Fixed by adding `check_filename_matches_id` alongside
`check_unique_ids`, with two new tests in
`scripts/test_generate_decisions_index.py`.

## Merging PR #812 (the concurrent D-204 collision) after the renumber

`git merge origin/main` after the D-204→D-205/D-206 renumbering produced
real, adjacent-content conflicts (not further numbering conflicts) in
`docs/PYTHON_STANDARDS.md`, `docs/ROADMAP.md`, and `docs/TYPE_SYSTEM.md`:
both this branch (Part 2's narrowing) and #812 (Part 3's `float`/`bool`
widening) had independently edited the same PEP 604/`Optional[T]`
narrative paragraphs. Resolved by hand, combining both changes'
substance rather than picking one side, and correcting every lingering
`D-201`/`D-204` reference in #812's own conflicting hunks to the final
`D-205`/`D-204` numbers this branch settled on (#812's own `D-204` decision
for the float/bool widening keeps its number — only the pre-existing
narrowing citations that #812 had inherited from the stale pre-renumber
`main` needed correcting). `docs/decisions/README.md` conflicted too, but
was resolved by regenerating from source (`generate_decisions_index.py`)
rather than hand-merging the generated table, per this repo's own
generated-file convention.

The merged prose pushed `site/llms.txt`'s non-optional aggregate 197
bytes over its 264 KiB CI-enforced budget (issue #207) — the same trap
recorded in `docs/sessions/2026-08-26-12-issue-711-815-method-call-diagnostic.md`.
Trimmed via several rounds of `GITHUB_PAGES=true bash scripts/check-site.sh`
in a tight edit-and-recheck loop, tightening wording in the merged
`docs/PYTHON_STANDARDS.md`/`docs/ROADMAP.md` paragraphs without dropping
any factual content, until the aggregate cleared the budget.

## Where to resume

Nothing pending from this issue. If a future decision renumbering is
needed, `check_unique_ids` and `check_filename_matches_id` in
`scripts/generate_decisions_index.py` are now the enforcement points —
extend their tests alongside any change there. Note the concurrency gap
documented above under "Renumbering": neither check can catch a
same-number collision between two branches that have not yet merged onto
the same tree — only landing order and CI's post-rebase check surfaces
that, as it did here a second time against PR #812.

All local gates green at this final snapshot: `cargo check --workspace`;
`python3 -B -m unittest discover -s scripts -p 'test_*.py'` (960 tests,
OK, skipped=6); `ruby scripts/test_check_roadmap_evidence.rb` (237 runs,
0 failures) and `ruby scripts/check_roadmap_evidence.rb` (passed);
`GITHUB_PAGES=true bash scripts/check-site.sh` (passed, after the trim
above); `ruby scripts/check_ci_permissions.rb` (passed, 10 files). The
D-014 100% Rust coverage gate was not run locally (this change alters no
compiled behavior) but must stay green in CI.
