# 2026-08-29-05: issue #676 — reject cross-MRO attribute redeclaration with a differing type (D-210)

## Overall status

Implementation of issue #676 is complete on branch `feat/issue-676-bool-widen-mro`,
originally planned against `origin/main` at `bad27d1c753027e9b36d0ae5bce8f31deccdf60b`.
Before this PR opened, `origin/main` advanced with PR #835 (issue #784), which
independently claimed decision number D-209 for an unrelated change. The
branch was rebased onto the new tip (`ccd30e9c`) and its own decision was
renumbered D-209 → **D-210** in the same change; PR #835's own D-209 file and
references were left untouched. This entry lands in the pull request that
delivers the work (D-192).

## What the PR contains

Implemented per the plan published on issue #676's own comment thread by
`issue-to-plan`:

- `crates/pycc_types/src/lib.rs`: new `check_incompatible_attribute_redeclarations`,
  walking each class's C3 MRO (`HirClassDef::mro`) and rejecting a
  differing-type attribute redeclaration across the hierarchy with a new
  error diagnostic, **T0052**. Wired into `check()`.
- `crates/pycc_types/src/constraints.rs`: the same check also wired into
  `checked_function_signatures`, since `pycc build`'s `check_and_resolve`
  calls that path directly and never calls `check()` — the two call graphs
  are mutually exclusive from `src/main.rs`'s own dispatch, so either path
  alone would leave a hole.
- `crates/pycc_diag/src/explain.rs`, `docs/DIAGNOSTICS.md`: new T0052
  registry entry.
- `docs/decisions/D-210-reject-cross-mro-attribute-redeclaration-with-a.md`
  (new, renumbered from D-209): records the diagnose-vs-coerce decision —
  coercion is unsound in general (a bare, non-derived base-class instance is
  never retyped), not merely inelegant, so diagnosing is the only sound
  choice.
- `docs/decisions/D-187-widen-a-bool-into-an-int-declared-attribute.md`:
  residual bullet resolved, pointing at D-210.
- `docs/ROADMAP.md`, `docs/TYPE_SYSTEM.md`: updated to describe the new
  differing-type rejection.
- `docs/decisions/README.md`: regenerated (`generate_decisions_index.py`).
- `crates/pycc_types/src/tests.rs`: 15 new tests — bool/int and float/int
  both directions, diamond sibling-base conflict, identical-type dedup and
  base-only non-regressions, the D-187 same-class widening non-regression,
  the `checked_function_signatures` call site exercised directly, real-source
  end-to-end fixtures via both `check_and_resolve` and the `pycc check` CLI
  path, a bare-`Base()`-instance D-187 counter-example pin, and a dataclass
  MRO test confirming field-name conflicts are resolved upstream by
  `pycc_hir::class`'s merge/dedup before T0052 ever runs.

## Verification performed (local, macOS)

- `cargo build --workspace`, `cargo test --workspace` (3962 passed, 0
  failed), `cargo clippy --workspace --all-targets -- -D warnings`,
  `cargo doc --workspace --no-deps`: all exit 0.
- `cargo llvm-cov --workspace --fail-under-lines 100 --fail-under-regions
  100`: genuine 100.00% lines (31390/31390) and 100.00% regions
  (48603/48603) on the final, post-rebase tree — a real gap was hit and
  fixed during round-1 implementation (a defensive `else { continue }`
  branch had no covering test before it was replaced with `.expect(...)`
  per this crate's own convention).
- `sh scripts/check-site.sh`, `RUBYOPT="-E utf-8" ruby
  scripts/check_roadmap_evidence.rb`: pass, post-rebase (the ROADMAP.md
  sentence added for this issue had to be trimmed once to stay under
  issue #207's 264 KiB aggregate byte budget — see below).
- `python3 scripts/generate_decisions_index.py docs/decisions
  docs/decisions/README.md --check`: up to date.

## Review loop and findings batch

Three rounds of the D-068/D-155 pinned reviewer (`ievo:deep-reviewer`):

- **Round 1:** 5 findings. Fixed: ROADMAP under-scoping the fix to bool/int
  only when float/int is also affected; a doc comment overclaiming
  dataclass-field-merge scope without a covering test; the hand-rolled
  `else { continue }` skip branch replaced with `.expect(...)`; the
  `pycc check` CLI path lacking its own end-to-end fixture. Refuted: a
  3+-class MRO chain's diagnostic message naming only the nearest two
  classes — a message-precision note, not a correctness/contract defect.
  `/harden batch` clustered the 5 findings into 5 classes: 2 matched
  existing `.harden/incidents/` topics with no new artefact warranted
  (`documentation-sweep-stops-at-the-changed-file`,
  `new-case-misses-branching-sites`), 2 became new singleton topics
  (`doc-comment-overclaims-unqualified-scope`,
  `hand-rolled-skip-branch-defies-expect-convention`), 1 needed no
  artefact (the refuted finding).
- **Round 2:** 1 finding — the round-1 doc-comment fix asserted the
  dataclass-conflict claim without a test; fixed with a real
  parse/lower/`check_and_resolve` pipeline test.
- **Round 3:** clean — zero actionable findings, independently corroborated
  by a manual re-read of the diff's core hunks (the dispatched reviewer's
  own context had no Bash tool and reconstructed scope from file contents
  rather than literal diff hunks; the orchestrating session covered that
  gap directly).

A self-discovered, unrelated regression surfaced during the round-1 fix:
adding the ROADMAP.md sentence pushed the file over issue #207's
`check-site.sh` 264 KiB aggregate byte budget — a separate site-publish
gate outside the Rust workspace gate set, so it wasn't caught by the
immediate build/test/clippy/coverage re-verification. Trimmed twice and
re-verified; logged as a process lesson in `docs/AGENT_RETROSPECTIVE.md`
per D-145.

All findings and dispositions are in `.harden/findings/issue-676.jsonl`.

## Rebase note (D-192 shared-numbering hazard)

At snapshot time before rebase, `origin/main` had already merged PR #835
(issue #784), which claimed D-209 for an unrelated lock-file-liveness
decision. The rebase resolved append-only-log conflicts in
`docs/AGENT_RETROSPECTIVE.md` (kept both branches' entries), hit no
conflict in `docs/ROADMAP.md` (non-overlapping insertion points), and
regenerated `docs/decisions/README.md` fresh rather than hand-merging it.
This is the second time in as many days two concurrently-worked issues have
raced for the same decision number (#833/#784 raced for D-208/D-209
earlier); worth watching whether decision-number reservation needs a
lighter-weight claiming mechanism if the rate keeps up, but two data points
is not yet enough to justify one.

## Follow-ups

- The adversarial-advisor round during `issue-select`'s pick of #676 also
  surfaced an unmarked-issue-starvation observation (#737 and #733,
  unrelated to this issue): under the milestone-membership-first ordering
  rule, an in-scope unmarked issue can outrank an out-of-scope P1
  indefinitely. The advisor's own caution was that #733's fix needs
  measured evidence, not a single anecdote — noted here for a future
  `issue-select` round to weigh explicitly, not acted on in this PR.

## Resume pointers

- Plan: issue #676's comment thread (published verbatim by `issue-to-plan`).
- Design: `docs/decisions/D-210-reject-cross-mro-attribute-redeclaration-with-a.md`.
- Code: `crates/pycc_types/src/lib.rs::check_incompatible_attribute_redeclarations`,
  `crates/pycc_types/src/constraints.rs::checked_function_signatures`.
- Tests-and-docs map: `docs/DIAGNOSTICS.md` (T0052), `docs/TYPE_SYSTEM.md`.
