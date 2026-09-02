# 2026-09-02-04 -- Issue #867 (Part 2 of #864): HIR per-item diagnostic collection

## Status: delivered by the pull request that carries this file

Worktree: `/Users/denis/projects/pycc-worktrees/issue-867-hir-multi-diag`,
branch `feat/issue-867-hir-multi-diag`, developed on `origin/main` at
`a9dbb61eea9f25d1ed1d3ce186787851140e7746` (#872 and #873 had merged after Part 1;
the branch was fast-forwarded before implementation and `origin/main` had
not moved again when the pull request opened). Implements
[#867](https://github.com/rotnov/pycc/issues/867), Part 2 of
[#864](https://github.com/rotnov/pycc/issues/864); #864 stays open for
Part 3 ([#868](https://github.com/rotnov/pycc/issues/868), type-checker
per-function collection). Decision record
[D-219](../decisions/D-219-hir-per-item-diagnostic-collection-with-poisoned-binding-cascade-suppression.md).
Plan: <https://github.com/rotnov/pycc/issues/867#issuecomment-5508277857>.

## How this task was run

Standard `issue-implement` flow under D-127/D-142/D-143: preflight and
staleness triage in the orchestrating session (premise reproduced on
`157ca610`, the tree right after Part 1 merged), `issue-to-plan` dispatched
in an isolated agent (three adversarial review rounds), implementation
dispatched in a second isolated agent, then the D-068 loop, the harden
batch, and this file in the orchestrating session.

## What changed

- `crates/pycc_hir/src/module.rs` (new; `lower_checked` first moved there
  as a pure extraction from `lib.rs`, verified line-set identical): new
  `lower_all(&ModModule) -> Result<HirModule, Vec<Diagnostic>>` walks the
  module one top-level item at a time (`lower_top_level_item`), records one
  diagnostic per failing item, and suppresses cascades through a poisoned
  set of class/type-alias names whose definitions failed (rules P1-P7 in
  D-219; a successful rebinding un-poisons). The post-loop phases (class
  slice rebuild, exception-tag assignment, `Exception.__init__` seeding)
  run only when the walk collected nothing. `lower_checked` stays as the
  first-element wrapper, so its call sites are untouched; the first
  diagnostic is byte-identical to the pre-change output and the list is not
  re-sorted. Cascade detection parses the shared message builders
  `unknown_annotation_name_message` (`func.rs`) and `unknown_base_message`
  (`class/mro.rs`).
- `src/frontend.rs`: `lower_frontend` calls `pycc_hir::lower_all`, so
  `check`, `build`, and `run` report every HIR diagnostic per file; an HIR
  failure still stops before the type checker.
- Tests: 21 unit tests in `crates/pycc_hir/src/module/tests.rs`; the
  `c0001_issue_864_repro` fixture now expects two `C0001`s; new fixture
  `c0001_hir_cascade_suppressed`; new CLI test in
  `tests/issue_864_multi_diag.rs` covering `check --error-format json` and
  `build` stderr.
- Docs: `docs/CLI_SPEC.md`, `docs/DIAGNOSTICS.md`, `docs/ROADMAP.md`
  (pipeline row), `docs/ARCHITECTURE.md`, `docs/TYPE_SYSTEM.md`,
  `.claude/skills/pycc/SKILL.md`, new D-219, regenerated
  `docs/decisions/README.md`; module-walk comments across `pycc_hir` and one
  in `pycc_types` re-pointed at `module::lower_all`.

## D-068 review and harden batch

Three rounds of the pinned `ievo:deep-reviewer` over the full
merge-base..HEAD range. Round 1: three stale doc comments naming
`lower_checked` as the walker (fixed at the three cited sites) and two
refuted findings. Round 2: the same class at ten more sites plus one
over-narrow gloss from the round-1 fix (fixed with a whole-crate sweep).
Round 3: clean. All seven findings are in `.harden/findings/issue-867.jsonl`.
The harden batch promoted the fix-extent lesson (derive a fix's extent from
a tree-wide search, not the reviewer's cited sites -- it had lived only in
`docs/AGENT_RETROSPECTIVE.md` since 2026-08-29 and recurred) into
`issue-implement` step 5, extended the step-5 brief to cover git-only
claims, and recorded four incident entries under `.harden/incidents/`.

## Gate results (from the committed tree, exit status captured directly)

- `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets --
  -D warnings`, `cargo build --workspace`: exit 0.
- `cargo llvm-cov --workspace --fail-under-lines 100 --fail-under-regions
  100`: exit 0, TOTAL 100.00% regions / functions / lines, 0 missed; no
  exemption added.
- `scripts/` unittest suite, `validate_agent_policies.py`,
  `validate_agent_assets.py`, `check_harden_findings.py`,
  `generate_decisions_index.py --check`, `check_roadmap_evidence.rb` + test,
  `check_status_page_freshness.rb origin/main HEAD`, `check-site.sh`,
  `manage_ci_bypass.py status` (protection matches baseline): all exit 0.
- `cargo doc --workspace --no-deps`: the four pre-existing private-link
  warnings only.

## Acceptance evidence

Read-only `pycc check` over the 40-file external corpus that motivated #864:
every file now reports its full HIR diagnostic list (for example one module
went from one diagnostic to 19: 18 `C0001` plus one `T0049`), and the first
diagnostic of every file is byte-identical to the pre-#867 binary's output
(0 mismatches across the 40 files).

## Known state at drafting time

- Pull request [#874](https://github.com/rotnov/pycc/pull/874) (#869, head
  `b1781d65`, MERGEABLE) was open; it edits `docs/CLI_SPEC.md` too, so
  whichever merges second may need a trivial rebase of that file. No
  decision-number clash: it claims no ADR.
- Issue #867 had exactly one comment (the plan) when this file was written.

## Where to resume

After this pull request merges: run `issue-to-plan` on #868 (Part 3)
against the tree as it then stands, then implement it; close #864 only when
#868 has merged. Then the standing external-corpus coverage loop continues.
