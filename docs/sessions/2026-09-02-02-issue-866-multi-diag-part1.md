# 2026-09-02-02 -- Issue #866 (Part 1 of #864): report every frontend diagnostic per pass

## Status: delivered by the pull request that carries this file

Worktree: `/Users/denis/projects/pycc-worktrees/issue-864-multi-diag`, branch
`feat/issue-864-multi-diag-part1`, developed on `origin/main` at
`afc0c13b8c4644f513f89b8036fa413ef9d6d34d` and rebased before the pull
request opened onto `751f10c7c5d255f2079033fe4f350e111a7b8932` (#865 and
#870 merged in between; the only conflict was the generated
`docs/decisions/README.md`, regenerated). Implements
[#866](https://github.com/rotnov/pycc/issues/866), Part 1 of
[#864](https://github.com/rotnov/pycc/issues/864); #864 stays open for
Parts 2 ([#867](https://github.com/rotnov/pycc/issues/867), HIR
per-top-level-item collection with poisoned-binding cascade suppression) and
3 ([#868](https://github.com/rotnov/pycc/issues/868), type-checker
per-function collection preserving solver-first selection). Decision record
[D-217](../decisions/D-217-report-every-frontend-diagnostic-per-pass-with.md).

## How this task was run

Standard `issue-implement` flow under D-127/D-142/D-143: the orchestrating
session did the D-021 preflight and staleness triage (premise reproduced on
`afc0c13b`; the only commit touching the issue's files since filing was
`7107fc20`, the workspace-wide rustfmt reformat), then dispatched
`issue-to-plan` in an isolated agent. That run verified #864's three-seam
decomposition against the tree, opened #866/#867/#868 in milestone v0.4,
published the Part 1 plan on #866 (three adversarial review rounds) and a
decomposition pointer on #864, and recorded nine corrections to the issue's
premises -- most consequentially that ruff 0.0.6's `Parsed::errors()` is in
*discovery* order, not source order, so re-sorting would have changed the
first diagnostic. Implementation ran in a second isolated agent.

## What changed

- `crates/pycc_parser/src/lib.rs`: new `parse_all` returning every
  `Parsed::errors()` entry as `L0001` in ruff's order (`Err` never empty by
  construction); `parse` becomes a first-element wrapper so its 75 callers
  are untouched.
- `src/frontend.rs` (new, extracted from `src/main.rs`, which drops from
  1,009 to 916 lines): `FrontendFailure::Compile { diagnostics:
  Vec<Diagnostic>, source }` (the `Box` is gone), the three `*_frontend`
  functions, `render_all`, and both reporters. `check` prints every
  diagnostic to stdout (human renders concatenated; JSON one object per
  line, `format_version` unchanged); `build`/`run` print the same set to
  stderr and still stop before MIR.
- Tests: two new snapshot fixtures (`l0001_two_syntax_errors`,
  `c0001_issue_864_repro` -- the latter pins that the parent's reproduction
  still yields exactly one `C0001` until Part 2), registered in
  `tests/diagnostics_test.rs`; new `tests/issue_864_multi_diag.rs` (JSON
  Lines structure, multi-file concatenation in both formats, unreadable
  middle path still exit 2, `build` stderr equals `check` stdout); five
  parser unit tests including the discovery-order test and `parse ==
  parse_all[0]`. No existing `.expected.*` fixture changed; the first
  diagnostic was verified byte-identical against the pre-change binary.
- Docs: `docs/CLI_SPEC.md` (check row, exit-1 prose, stdout/stderr split,
  JSON Lines, ordering rule), `docs/DIAGNOSTICS.md` (Quality-bar bullet),
  `docs/ROADMAP.md` (pipeline-row clause only), `.claude/skills/pycc/SKILL.md`
  (the "one current frontend diagnostic" sentence), new D-217, regenerated
  `docs/decisions/README.md`.

## D-068 review

Round 1 (pinned `ievo:deep-reviewer`, full merge-base..HEAD range): two
findings, no blockers. (1) This session file was missing from the range --
a deliverable of the PR step, written here. (2) The `FrontendFailure::Compile`
doc comment implied an empty-list `check` exit would be acceptable;
tightened in `16867136` (pre-rebase `c5302d00`). Findings recorded in
`.harden/findings/issue-866.jsonl`. Round 2 and the harden batch ran after
this file was drafted; their outcome is in the pull-request body.

## Gate results (from the committed tree, exit status captured directly)

- `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets --
  -D warnings`, `cargo test --workspace` (4068 passed): all exit 0.
- `cargo llvm-cov --workspace --fail-under-lines 100 --fail-under-regions
  100`: exit 0, TOTAL 100.00% regions / functions / lines, 0 missed; no
  exemption added.
- `scripts/` unittest suite (963 tests), `validate_agent_policies.py`,
  `validate_agent_assets.py`, alpha-skill evals (both clients),
  `check_scratch_dir_usage.py`, `check_conformance_breadth.py`,
  `check_ci_permissions.rb` + test, `check_readme_coverage_badge.rb` + test,
  `generate_decisions_index.py --check`, `check_roadmap_evidence.rb` + test,
  `check_status_page_freshness.rb origin/main HEAD`: all exit 0.
- `cargo doc --workspace --no-deps`: no new warnings (the four pre-existing
  private-link warnings are unchanged from the base revision).

## Known state at drafting time

- Pull request [#865](https://github.com/rotnov/pycc/pull/865) was open
  (head `9d696c45`, BLOCKED) while this branch was developed; it claimed
  D-216 and edited `docs/CLI_SPEC.md`, `docs/ROADMAP.md`, and
  `docs/decisions/README.md`. It merged as `ab9beef3` before this pull
  request opened, so the rebase regenerated `docs/decisions/README.md`;
  D-217's number stayed valid. No pull request was open at rebase time.
- The Pages workflow's first run on PR #871 failed `scripts/check-site.sh`'s
  llms.txt aggregate budget by 104 bytes: `origin/main` at `751f10c7` left 9
  bytes under the 264 KiB ceiling and this branch's `docs/ROADMAP.md` clause
  is 113 bytes. Resolved per D-127 by
  [D-218](../decisions/D-218-raise-llms-txt-aggregate-budget-to-272-kib.md)
  (264 -> 272 KiB, D-200's step repeated) rather than by trimming, and by
  adding `check-site.sh` to `issue-implement`'s step 4 gate list for diffs
  touching a manifest-listed document. The #802 umbrella item on shrinking
  the budgeted documents stays open.
- Motivation, for the record: a read-only `pycc check` sweep over an
  external ~59k-line corpus found 96% of first diagnostics were `import`
  statements, hiding everything after them; Part 2 is the part that
  actually unblocks that sweep.

## Where to resume

After this pull request merges: run `issue-to-plan` on #867 (Part 2) against
the tree as it then stands, then implement it; then #868. Close #864 only
when both are merged. Nothing else is pending from this task.
