# 2026-09-11 (02) — #423 / #371: point source comments at `docs/decisions/`

Status: delivered by the pull request that carries this file (squash-merged
into `main`); base `origin/main` `19e8b834`.

## What changed

`docs/DECISIONS.md` was deleted by D-151's migration to one file per
decision under `docs/decisions/D-NNN-<slug>.md` (index
`docs/decisions/README.md`). Eleven live source comments still cited the
deleted path. Each now cites the decision's own file, relative to the
repository root, with the comment re-wrapped to its file's existing width
and no neighbouring code touched:

| Hit | Now cites |
| --- | --- |
| `crates/pycc_codegen/src/lib.rs` (tuple string conversion and tuple truthiness arms) | `docs/decisions/D-116-tuple-v0-2-scope-int-bool-float-elements-only.md` |
| `crates/pycc_std/src/lib.rs` crate doc comment | `docs/decisions/D-136-pycc-std-is-a-plain-data-crate-math-sys-symbols.md` |
| `crates/pycc_types/src/tests.rs` (`int` passed where `float` is annotated) | `docs/decisions/D-086-two-type-boundary-strictness-decisions-equality.md` |
| `scripts/manage_ci_bypass.py` (stacking-race residual window) | `docs/decisions/D-125-session-driven-temporary-ci-check-relaxation.md` |
| `scripts/test_check_roadmap_evidence.rb` (Ubuntu frontend-perf digest fixture) | `docs/decisions/D-112-move-frontend-perf-measure-frontend-perf-gate.md` |
| `tests/conformance.rs` (PEP 585 tuple fixture) | `docs/decisions/D-116-tuple-v0-2-scope-int-bool-float-elements-only.md` |
| `tests/conformance.rs` (PEP 709 fixture) | `docs/decisions/D-120-the-pep-709-fixture-demonstrates-loop-variable.md` |
| `tests/fixtures/nbody.py` (dropped local annotations) | `docs/decisions/D-088-correct-v0-2-s-acceptance-criteria-before-any-v0.md` |
| `tests/slice0.rs` (`pycc_testkit` deferral) | `docs/decisions/D-018-pycc-testkit-deferred-past-pr-1-pr-2.md` |
| `tests/slice1_codegen_depth.rs` (dict mutation during iteration) | `docs/decisions/D-123-dict-str-int-ships-a-d-k-v-insert-or-update.md` |

Two hits named no decision number and needed the owning decision located:

- `tests/fixtures/nbody.py` said "matching `docs/DECISIONS.md`'s PR-9 scope
  note". D-088's third numbered point carries that note nearly verbatim:
  "PEP 526 ... is **not** free: verified empirically that `pycc_hir` handles
  `Stmt::Assign` but has no `Stmt::AnnAssign` case at all ... assigned to
  PR-9 as real scope". D-102 (PR-9's conformance-fixture decision) only
  cites PEP 526's `Stmt::AnnAssign` work in passing as already-assigned
  scope, so D-088 is the decision that records the note.
- `tests/slice0.rs` said "pycc_testkit, deferred per DECISIONS.md". D-018
  is that deferral and names `tests/slice0.rs` itself as the hand-written
  stand-in, so it is cited directly rather than the index.

The residual check
`git grep -n "DECISIONS\.md" -- '*.rs' '*.py' '*.rb'` with the exclusions
below prints nothing.

## Deliberately left untouched

- `scripts/migrate_decisions_log.py`, `scripts/rewrite_decisions_references.py`
  and their `scripts/test_*.py` counterparts: the old filename is their
  subject matter (they are the D-151 migration tooling), not a dangling
  reference.
- `tests/fixtures/policy-successors/*.rb`: frozen D-103-era snapshot copies
  of past checker revisions. Nothing reads them as code; their value is
  being byte-identical to the revision they snapshot, and rewriting a
  snapshot destroys that value.
- Everything under `docs/`: out of this issue's scope (prose cross-references
  there are D-151's own domain).

`tests/fixtures/policy-successor-manifest.json`'s `sha256` fields already
drift from the live files and were deliberately not regenerated: D-172
retired the exact-byte rule, and the manifest is read as a path inventory
only.

## No new guard

No checker was added to reject a future `docs/DECISIONS.md` citation.
AGENTS.md's D-192 filing bar admits a process guard only when its absence
can cause an incorrect merge decision or hide a compiler defect; a dangling
path inside a comment can do neither. This session file and the retired
path's absence from the tree are the record.

## Collision note

Open PR #996 rewrites other parts of `crates/pycc_types/src/tests.rs`. This
change touches exactly one comment line in that file (the D-086 citation),
which is the cheapest possible conflict for whichever pull request lands
second.

## Gates at the delivered head

All exit 0: `cargo fmt --all -- --check`; `cargo clippy --workspace
--all-targets -- -D warnings` (three pre-existing rustc
`multiple lines skipped by escaped newline` warnings at
`tests/slice1_codegen_depth.rs` lines 738/749/1132, outside this change's
hunk, do not fail the gate); `cargo test -p pycc_std --lib` (31 passed);
`cargo test -p pycc_types --lib` (1625 passed); `cargo test --test slice0`
(87 passed); `python3 -B -m unittest discover` in `scripts/` (1117 tests,
OK, 6 skipped); `test_check_roadmap_evidence.rb` (247 runs, 1294 assertions,
0 failures); `check_roadmap_evidence.rb` ("Roadmap evidence policy passed");
`validate_agent_policies.py` ("agent policies: valid");
`validate_agent_assets.py` ("agent assets: valid").

## Follow-ups

None filed. Comment-only change; no roadmap, delivery-plan, or decision text
changes.

## Where to resume

`git log --oneline -1 origin/main`; open issues in milestone v0.4; PR #996
(part of #695) is the other open pull request touching
`crates/pycc_types/src/tests.rs`.
