# Session handoff: issue #738 (Part 1 of #543) — PEP 765 finally-exit rejection

- Status: PR #741 opened against `main`, head `issue-543-oserror-finally-except-comma`
  at commit `4dbbe20d` (plus this handoff commit). All local gates green before push;
  CI checks were still completing as this entry was written — see the PR's own check
  run history for final status, and do not treat this paragraph as CI evidence.
- What shipped: `return`/`break`/`continue` that would exit a `finally` block (PEP 765)
  is now rejected with a dedicated `L0001` diagnostic ("'<kw>' in a 'finally' block"),
  gated on a valid escape target existing outside the `finally` (`in_finally &&
  in_function` for `return`, `in_finally && in_loop` for `break`/`continue`) — matching
  CPython 3.14's own fatal-error precedence, verified directly against
  `python3.14 -W all` (3.14.6) rather than assumed. Without a valid target anywhere,
  pycc keeps deferring to its pre-existing `L0001` "outside loop"/`T0024` "outside
  function" diagnostics, exactly as CPython does.
  - Primary implementation: `crates/pycc_hir/src/stmt.rs` (new `in_finally: bool`
    threaded context, third member of the D-148/D-149 family) and
    `crates/pycc_hir/src/stmt/exception.rs`.
  - New ADR: `docs/decisions/D-193-reuse-l0001-for-context-invalid-finally-exit.md`,
    indexed in `docs/decisions/README.md`.
  - Docs updated: `docs/DIAGNOSTICS.md` (`L0001` row), `crates/pycc_diag/src/explain.rs`
    (`pycc explain L0001`), `docs/ROADMAP.md` (new dated Update bullet on the
    `#382`/`#540`/`#541` entry — no conformance matrix row moves).
    `docs/PYTHON_STANDARDS.md` deliberately left unchanged: PEP 765's row stays `☐`
    since CPython 3.14 itself still only warns, so a run-and-compare fixture isn't
    meaningful yet (reasoning recorded in D-193).
  - Tests: 14 new/flipped unit tests in `crates/pycc_hir/src/tests.rs`, the flipped
    `try_with_return_in_finally_rejects_with_l0001` in `crates/pycc_types/src/tests.rs`,
    unchanged coverage in `tests/issue_382_exceptions.rs`, and 6 new CLI fixture pairs
    under `tests/diagnostics/` wired into `tests/diagnostics_test.rs`
    (`l0001_return_inside_finally`, `l0001_break_inside_finally`,
    `l0001_continue_inside_finally`, their `*_with_no_enclosing_*` fallback
    companions, and `d0024_return_inside_finally_with_no_enclosing_function`).
- Local gates run and green before push: `cargo test --workspace` (53 result blocks,
  0 failures), `cargo clippy --workspace --all-targets -- -D warnings` (clean, 5
  `#[allow(clippy::too_many_arguments)]` added matching the existing
  `lower_protocol_class` precedent), `cargo llvm-cov --workspace
  --fail-under-lines 100 --fail-under-regions 100` (100.00%/100.00%),
  `cargo doc --workspace --no-deps`, `scripts/check_conformance_breadth.py`,
  `scripts/check_roadmap_evidence.rb`, `scripts/validate_agent_assets.py`,
  `scripts/validate_agent_policies.py`.
- Review loop (D-068/D-155, `ievo:deep-reviewer`): two full rounds. Round 1 found a
  real correctness bug (initial unconditional `in_finally` check misprioritized
  CPython's own diagnostic precedence for the no-valid-target case), a missing
  oracle-evidence record, and a missing ADR — all fixed, confirmed against the
  `python3.14 -W all` oracle directly. Round 2 (verifying round 1's fixes) found a
  doc-drift gap in `docs/ROADMAP.md` (fixed with the dated Update bullet) and one
  informational-only note about a now-synthetic-only codegen test (no action needed,
  correctly assessed as costing coverage with no benefit to delete).
- PR body: ends with "Part 1 of #543; #543 stays open for the remaining PEP 3151 and
  PEP 758 rows.\n\nFixes #738." — confirmed via `gh api graphql`'s
  `closingIssuesReferences` that `totalCount == 1` with exactly issue #738, so merging
  closes #738 only, leaving the parent tracker #543 open.
- Known follow-ups: none new from this PR. #543 (the parent tracker) stays open for
  PEP 3151 and PEP 758.
- Where a fresh session should resume: if this entry is being read before PR #741 has
  merged, re-check `gh pr view 741 --repo rotnov/pycc` for current CI/review state
  before assuming anything below is still accurate — a fresh session should re-verify
  live state rather than trust this snapshot's completion claims. Once #741 is merged
  and its branch deleted, the next PEP-765-adjacent work is picking up the remainder
  of #543 (PEP 3151, PEP 758) via `issue-select`/`issue-to-plan` in the normal way.
