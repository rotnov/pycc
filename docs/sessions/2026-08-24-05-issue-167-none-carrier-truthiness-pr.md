# Session handoff: issue #167 — D-075 None-carrier truthiness test hardening

- Status: implementation, a same-change file decomposition, and two review
  fix rounds are complete and green on branch
  `issue-167-none-carrier-truthiness-test`, based on `origin/main` tip
  `435d1d52` (the #751 merge commit) and later fast-forward-merged with
  `origin/main` mid-PR to pick up #732/#750. PR
  [#752](https://github.com/rotnov/pycc/pull/752) is open,
  `closingIssuesReferences.totalCount == 1` (closes exactly #167), and both
  review threads are resolved.
- What shipped: D-075 promises that a `None` value crossing the user-function
  ABI is the canonical LLVM `i8 0` unit carrier, but no existing test
  actually observed that carrier value — `none_typed_parameters_cross_the_user_function_abi`
  only printed the parameter, which renders the literal `"None"` from the
  static `Ty::None` type tag regardless of the underlying bit pattern, and
  the codegen unit test `reading_a_none_typed_parameter_slot_emits_its_unit_carrier`
  discarded its loaded value with `let _ = value;` and asserted nothing. Both
  stayed green under the exact mutation this issue describes (flipping the
  `None` call-result carrier's `Scalar::Bool(const_int(0, false))` to
  `const_int(1, false)` in `crates/pycc_codegen/src/lib.rs`). Fix: a new test
  `a_none_call_result_crossing_the_abi_carries_a_falsy_unit_value` branches on
  the parameter's truthiness (`if value: print(1) else: print(0)`) instead of
  printing it — pycc lowers that through the same `truthy` path as any other
  typed value, so a flipped carrier bit changes which branch prints. Verified
  locally that this fails under the issue's exact mutation and passes once
  reverted. The codegen unit test's comment was corrected to stop claiming
  other tests verify the exact carrier value and instead point at the new
  end-to-end regression that does. No production code changed.
- Same-change file decomposition (AGENTS.md "Keep source files decomposable",
  D-185): `tests/slice1_codegen_depth.rs` (1,650 lines) and
  `crates/pycc_codegen/src/tests.rs` (11,826 lines) are both oversized files
  this PR's own work touched, and neither has an existing D-185 tracking
  issue (#544-#552 cover only non-test crate `lib.rs`/`class.rs`/`explain.rs`/`expr.rs`
  files). An automated `chatgpt-codex-connector[bot]` review flagged this as
  a P1 finding on both files. Resolution, reasoned independently per D-127
  (no existing carve-out applies to either file):
  - `tests/slice1_codegen_depth.rs` — this PR's diff there was structural (a
    new test function), so the touched part was extracted: both
    `none_typed_parameters_cross_the_user_function_abi` and the new
    `a_none_call_result_crossing_the_abi_carries_a_falsy_unit_value` moved to
    a new standalone `tests/issue_167_none_carrier_abi.rs`, following this
    repo's existing per-issue test-file convention (`tests/issue_378_dataclasses.rs`
    and siblings — each with its own duplicated `pycc_bin`/`write_fixture`/`build_and_run`
    helpers; no shared `tests/common` module exists in this repo to reuse
    instead). A placeholder comment was left at the removal site naming both
    moved functions and the new file.
  - `crates/pycc_codegen/src/tests.rs` — this PR's diff there was a 4-line
    comment correction only (verified via `git diff origin/main...issue-167-none-carrier-truthiness-test --
    crates/pycc_codegen/src/tests.rs`), with no structural change, so there
    was no "part it touches" to extract; the file was left as-is. Replied to
    the bot's thread explaining both halves of the resolution, then resolved
    it once the extraction commit landed.
- Second review round: the pinned local `ievo:deep-reviewer` (D-068/D-155)
  ran twice. First pass (against the initial single-commit diff) flagged one
  low-confidence doc-drift note on `crates/pycc_codegen/src/tests.rs:1556-1557`
  that its own sandbox couldn't confirm against git — independently verified
  via `git diff` that the flagged lines were pre-existing, untouched context
  outside this PR's diff, so no action was needed. Second pass (after the
  file-decomposition commit, since a structural test move isn't a doc-only
  change exempt from re-review) found one real finding: the comment fixed in
  the first commit named the moved test's new location by function name, but
  still pointed at the old `tests/slice1_codegen_depth.rs` file path instead
  of the new `tests/issue_167_none_carrier_abi.rs` after the extraction.
  Fixed in a follow-up commit.
- Local gates run: `cargo build --workspace` clean; `cargo test --test
  issue_167_none_carrier_abi` (2/2 passing, including the new regression);
  `cargo test --test slice1_codegen_depth` (67 tests, all passing, 0 lost —
  confirmed the two extracted tests are the only count delta); `cargo test
  --workspace` run twice (once per extraction commit) — 0 `FAILED` except
  the pre-existing, unrelated `build_and_run_cross_compiled_to_a_different_tier_1_target`
  local-sandbox linker failure in the `slice0` integration test, consistent
  with every other workspace run this session. `cargo test -p pycc_codegen
  reading_a_none_typed_parameter_slot_emits_its_unit_carrier` re-run after
  the stale-reference fix. PR #752's full CI matrix (`audit`,
  `classify-changes`, `cross-compile-build`, `cross-compile-verify`,
  `governance`, `status-page-freshness`, `frontend-perf-measure`,
  `frontend-perf-gate`, all four `native-build-test` platform legs,
  `build-test-coverage`, `ci-gate`) went green on the final head `8cacaaa3`,
  confirmed stable across two consecutive polls after the head stopped
  changing.
- Branch-currency note: the branch was created from `origin/main` tip
  `435d1d52`; `origin/main` advanced past that (via #732/#750) while CI ran
  on the first commit, so the branch was fast-forward-merged with
  `origin/main` (a clean, conflict-free `git merge`) before the
  decomposition commit was pushed.
- Where to resume: nothing — this PR is ready to merge. `git log` on
  `issue-167-none-carrier-truthiness-test` — `a2689bcf` (initial regression
  test + comment fix), a merge commit with `origin/main`, `e0e9b86b`
  (file-decomposition extraction), `8cacaaa3` (stale-reference fix from the
  second reviewer pass).
- Standing `/goal` continuation: this is the fourth of an ongoing series of
  small, independently-scoped meddylib gap fixes shipped one-PR-per-fix
  (after #744/PR #746, #720/PR #748, #247/PR #751). Further iterations
  should keep selecting small, well-scoped, non-`issue-to-plan`-gated issues
  from the tracker rather than stopping after a small number of merges.
  Candidates already surveyed and rejected as too large for a quick
  single-PR fix in earlier segments (#150, #618, #704/#705/#711/#714, #246,
  #676) remain available later behind an `issue-to-plan` gate or once
  blocking dependencies land; #424 (trivial `docs/DIAGNOSTICS.md` `--fix`
  claim) remains a fallback trivial candidate.
