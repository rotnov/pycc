# 2026-09-07-05 — #695: extract the constraint-collection tests from `pycc_types/src/tests.rs`

## Overall status

Delivered as one D-185 partial-decomposition pull request against the tracking
issue #695. The issue **stays open** by design: `crates/pycc_types/src/tests.rs`
is still far above AGENTS.md's ~1,000-line decomposability threshold after this
change, so the tracker is narrowed by a comment rather than closed.

Baseline: `origin/main` at `c38bf9747951e84f11c1f5f09f134f693cad2cb7`
("P2: Bind the README comparison table's published verdicts to claims.json
(#682) (#994)"). Branch `autopilot/iter-2026-09-07-31`, worktree
`.claude/worktrees/frosty-hopper-0670b0`. Zero other open pull requests at the
D-078 checkpoint.

## What landed

One cohesion-driven submodule extraction, plus the comment fix the review round
surfaced:

- `crates/pycc_types/src/tests/constraints.rs` (new, 2,988 lines) — the 97
  tests whose names begin `constraint_collection_`,
  `collect_block_constraints_` or `collect_expr_constraints_`, i.e. the tests
  that exercise the constraint-collection pass in the production sibling
  `crate::constraints`. The split mirrors that sibling's own `constraints.rs` +
  `constraints/` layout and follows the established
  `tests/{import_alias,init_rank,protocol_argument,protocol_return}.rs`
  convention (sibling file plus directory, no `mod.rs`).
- `crates/pycc_types/src/tests.rs` — 29,986 → 26,910 lines; gains
  `mod constraints;` and a generalized comment above the `mod` block.

Pure move: no test body, name, attribute or comment inside the moved range
changed; no helper moved; no visibility widened. The child sees the parent's
private items through `use super::*`, exactly as `tests.rs`'s own header
records for the #544 Part 1 relocation.

### Scope note — 97 extracted, not the 127 name-matches

The dispatch brief sized the cluster as "~127 `fn`s whose names match
`constraint`". Inspection showed ~30 of those merely mention the word while
testing other subsystems (`math_sqrt_*`, `math_pi_*`, `enum_marker_*`,
`match_with_constraint`, several `private_*_propagates_nested_constraint_errors`).
Pulling them in would have made the cut *less* cohesive, so the extraction took
the three prefixes above — precisely the tests of `crate::constraints`. This is
the brief's primary (constraint) cut, not its `match_pattern` fallback.

## Evidence

- `cargo test --offline -p pycc_types --lib`: 1625 passed, exit 0, both before
  and after. `-- --list` output is **name-set identical** after stripping the
  `tests::constraints::` prefix (empty `diff`), which proves nothing was lost
  or duplicated — a stronger oracle than the count alone.
- Line-multiset identity between the pre-change file and (post-change parent +
  new child) verified programmatically, modulo blank-line runs, the added
  `mod constraints;` and the new file's `//!` header.
- `cargo fmt -p pycc_types -- --check` 0; `cargo clippy --offline --workspace
  --all-targets -- -D warnings` 0; the three CI preparatory builds 0; D-014
  coverage `cargo llvm-cov --offline --workspace --fail-under-lines 100
  --fail-under-regions 100` **0** (100.00% lines / 100.00% regions).
- `python3 -B scripts/validate_agent_policies.py`, `ruby
  scripts/check_ci_permissions.rb`, `ruby scripts/check_roadmap_evidence.rb`
  all 0.

## Plan gate

The D-021 step-10 plan requirement was exempted for this change by the
orchestrating session: one architectural seam, one file pair, a mechanical test
relocation, with D-014's 100% line+region coverage gate plus the `--list`
name-set diff as the behavior-preservation oracle. `issue-to-plan` was
deliberately not invoked.

## Known follow-ups

- **#695 stays open.** `crates/pycc_types/src/tests.rs` is 26,910 lines. The
  obvious next cuts, by the same cohesion criterion, are the pattern-match
  cluster (~134 `fn`s matching `match_with` / `check_pattern` /
  `check_exhaustive`) and the monomorphization/generic-class cluster.
- **#677** (no gate detects an insertion that re-targets an existing
  doc-comment run) gained its **third** occurrence here; recorded in
  `.harden/incidents/insertion-retargets-a-doc-comment-run/2026-09-07.md`.
  This occurrence widens the required matcher shape twice more: the run was
  `//` rather than `///`, and the retargeted item was a `mod` declaration in a
  sorted declaration list, where insertions are routine.
- Pre-existing, machine-local, **not** caused by this change:
  `scripts/test_check_harden_findings.py::test_real_repository_piles_conform`
  fails in this checkout because seven leftover `.harden/findings/issue-*.jsonl`
  files from earlier autopilot iterations are present but untracked (a
  machine-local `.git/info/exclude` entry hides `.harden/`). A fresh CI clone
  has only tracked files, so the `governance` job is unaffected.

## Where a fresh session should resume

`gh issue view 695 --repo rotnov/pycc` — read the narrowing comments to see
which clusters have already been extracted, then take the next one.
