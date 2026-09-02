# 2026-09-02-06 -- Issue #868 (Part 3 of #864): type-checker per-function diagnostic collection

## Status: delivered by the pull request that carries this file

Worktree: `/Users/denis/projects/pycc-worktrees/issue-868-types-multi-diag`,
branch `feat/issue-868-types-multi-diag`, developed on `origin/main` at
`a65d1a16` (the tree right after Part 2, #875, merged) and rebased onto
`50f1f61f` (#874 and #876 had merged meanwhile; the only overlap was
`docs/CLI_SPEC.md`, in non-adjacent hunks). Implements
[#868](https://github.com/rotnov/pycc/issues/868), the last part of
[#864](https://github.com/rotnov/pycc/issues/864), which this pull request
closes too. Decision record
[D-220](../decisions/D-220-type-checker-per-function-diagnostic-collection-with-a-per-function-solver-first-merge.md).
Plan: <https://github.com/rotnov/pycc/issues/868#issuecomment-5511173041>.

## How this task was run

Standard `issue-implement` flow under D-127/D-142/D-143: preflight and
staleness triage in the orchestrating session, `issue-to-plan` dispatched in
an isolated agent (three adversarial review rounds), implementation dispatched
in a second isolated agent, then the D-068 loop, the harden batch, and this
file in the orchestrating session.

## What changed

- `crates/pycc_types/src/module.rs` (new; the module-level driver first moved
  there as a pure extraction from `lib.rs` and `constraints.rs`, verified
  line-set identical) and `crates/pycc_types/src/constraints/signatures.rs`
  (new; the solver's signature entry points, extracted from
  `constraints.rs`). `check_all` / `check_and_resolve_all` return
  `Vec<Diagnostic>`; `check` / `check_and_resolve` keep their signatures as
  first-element views. The driver carries
  `KeyedDiagnostics = Vec<(Option<usize>, Diagnostic)>` (function index or
  module-level). Pass 2 (signature validation) still stops at the first
  failure; pass 3 (bodies) collects one diagnostic per function;
  `infer_function_signatures_with_solver_all` collects per body; and
  `merge_solver_first` emits the solver's per-function picks first, then the
  checker-only entries for functions the solver did not report (D-220 rules
  C1-C7). Post-solver phases (monomorphize, unroll) run only when nothing was
  collected, so the first diagnostic is byte-identical to the pre-change
  output and the list is never re-sorted or deduplicated. The pre-#868
  single-diagnostic functions survive only as `#[cfg(test)]` wrappers.
- `src/frontend.rs`: `check_frontend` and `resolve_frontend` call the `_all`
  variants, so `check`, `build`, and `run` report every type diagnostic per
  file (after the HIR list from Part 2; an HIR failure still stops before the
  type checker).
- Tests: 26 unit tests in `crates/pycc_types/src/module/tests.rs` (literal
  first-diagnostic pins per phase, keyed collection, every merge arm, `Err`
  never empty); new fixture `tests/diagnostics/t0022_types_per_function`
  (three functions, three diagnostics: T0022, T0021, T0043, all still at
  `:1:1`); `tests/issue_864_multi_diag.rs` gains the CLI JSON, build-stderr,
  HIR-stops-first, and no-panic corpus-sweep tests.
- Process artefacts from the harden batch: see below.
- Docs: `docs/CLI_SPEC.md`, `docs/DIAGNOSTICS.md`, `docs/ROADMAP.md`
  (pipeline row), `docs/ARCHITECTURE.md`, `.claude/skills/pycc/SKILL.md`,
  new D-220, regenerated `docs/decisions/README.md`; comments across
  `pycc_types`, `pycc_mir`, and `tests/` re-pointed at the `_all` entry
  points.

## D-068 review and harden batch

Six rounds of the pinned `ievo:deep-reviewer` over the full
merge-base..HEAD range. Round 1: stale comments naming the deleted
`check_with_environment` plus an unapplied type alias (fixed with a
tree-wide search), and one refuted later-phase-deliverable finding. Round 2:
comments naming the now-`#[cfg(test)]`-only single-diagnostic wrappers as
production callers (35 sites re-pointed; the round-1 search had treated "the
name still compiles" as "still accurate"). Round 3: six more sites of the
same class that the round-2 search missed because it matched backticked
names only (unbackticked names, a path-qualified name, and stale
`lib.rs`/`constraints.rs` location claims). Rounds 4-6: one sentence -- the prose restatement of D-220 rule 4 at seven sites -- was found imprecise three times in a row (a two-way split, then a missing third arm, then a too-narrow second arm plus a fabricated rationale in `check_all`'s doc); the round-6 fix (`ed89b1c8`) stopped paraphrasing and installed one canonical sentence derived from the Decision rule and `merge_solver_first`'s code at all seven sites, verified by the orchestrating session's own diff read, which closed the loop under step 5's two-fix stop rule instead of a seventh reviewer round. All findings
and dispositions are in `.harden/findings/issue-868.jsonl`.
The `/harden batch` pass clustered the pile into three classes and landed one commit (`27c2f6dd`): `.claude/skills/issue-implement/references/review-brief.md` (a brief template carrying every later-phase exclusion; third file under `reviewer-flags-a-later-phase-deliverable`), a sharpened fix-extent sentence in `issue-implement` step 5 naming search forms and the release-build adjudication criterion (fifth file under `documentation-sweep-stops-at-the-changed-file`, with the rustdoc intra-doc-link gate recorded as the deferred static rung), and a new `AGENTS.md` rule under "Keep documentation current" -- quote or cross-reference a canonical rule, never paraphrase it per site (new topic `paraphrase-of-a-formal-rule-drifts-from-its-source`). Two `docs/AGENT_RETROSPECTIVE.md` entries carry the lessons.

## Gate results (from the committed tree, exit status captured directly)

- `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets --
  -D warnings`, `cargo build --workspace`: exit 0.
- `cargo llvm-cov --workspace --fail-under-lines 100 --fail-under-regions
  100`: exit 0, TOTAL 100.00% regions / functions / lines, 0 missed; no
  exemption added.
- `scripts/` unittest suite, `validate_agent_policies.py`,
  `validate_agent_assets.py`, `check_harden_findings.py`,
  `check_scratch_dir_usage.py`, `generate_decisions_index.py --check`,
  `check_roadmap_evidence.rb` + test, `check_status_page_freshness.rb
  origin/main HEAD`, `check-site.sh`: all exit 0.
- `cargo doc --workspace --no-deps`: the four pre-existing private-link
  warnings only.

## Acceptance evidence

Differential sweep of the 143 checked-in `tests/diagnostics` and corpus
inputs against the base tree's binary: 0 first-diagnostic differences
(D-217 rule 2). The new fixture renders three diagnostics for three
functions where the base tree rendered one.

## Deliberately left out

- Real spans: every `pycc_types` diagnostic still carries the D-043
  placeholder and renders at `:1:1`, so the three fixture diagnostics are
  distinguishable only by message. Filed as
  [#877](https://github.com/rotnov/pycc/issues/877) (v0.4),
  cited from D-220.
- The plan's "comment on #544" item (the file-decomposition tracking issue):
  a comment on an issue other than #868 is outside `issue-implement`'s
  authorized writes, so the extraction is recorded in D-220 and the pull
  request body instead.

## Known state at drafting time

- No open pull requests when this file was written; `origin/main` at
  `50f1f61f`.
- Issue #868 had exactly one comment (the plan).

## Where to resume

After this pull request merges, #864 is closed. The standing external-corpus
coverage loop continues: re-run the read-only `pycc check` sweep over the
corpus, which now reports every HIR and type diagnostic per file, and file
one issue per distinct feature gap. #877 is the first candidate the
sweep's own output quality depends on.
