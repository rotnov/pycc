# Session handoff: #246 Tier-1 target-list parity validator

## Status

In progress. Implementation is complete and verified locally (100%
lines/regions coverage on the full workspace; mutation-based verification
of the fix); the pull request has not yet been opened. This entry will be
committed together with the PR, and is accurate as of that commit — it
does not describe a merge that has already happened.

## What was done

- Selected #246 via `issue-select`'s full workflow (see "Selection" below).
- Added `tests/tier1_target_parity.rs`: a new integration test that
  independently re-derives the five Tier-1 targets from three sources —
  `pycc version --verbose`'s real stdout, `docs/ARCHITECTURE.md`'s
  "Cross-platform (hard requirement)" table, and `docs/CLI_SPEC.md`'s
  illustrative transcript — and asserts all three agree, in order.
- Verified the fix against the issue's own reproduction: applied the exact
  mutation from the issue body (`x86_64-pc-windows-msvc` →
  `aarch64-pc-windows-msvc` in both `src/main.rs::TIER1_TARGETS` and
  `tests/slice0.rs`'s snapshot literal, leaving both docs untouched).
  Confirmed the existing `slice0.rs` snapshot test still passed under that
  mutation (reproducing the issue's defect) and that the new
  `tier1_target_parity` test failed with a clear diff (closing the gap).
  Reverted the mutation before proceeding.
- No production behavior, roadmap acceptance evidence, or CI workflow
  changed — verified this explicitly rather than skipping the docs review
  (AGENTS.md's "Keep documentation current" default). No ADR was needed:
  this is a test-only hardening, not an irreversible or project-wide design
  choice.

## D-068 pinned reviewer (`ievo:deep-reviewer`)

Two rounds, both against pasted diff text (the reviewer has no Bash tool, so
`git diff --staged` was run by this session and pasted into the prompt each
time, per the D-068 caveat about `--working` omitting untracked files).

- **Round 1** (against the original `tests/tier1_target_parity.rs` alone):
  P1 — raw `include_str!` text compared against a `\n`-terminated marker
  would panic on a Windows CRLF checkout, and this diff's own `tests/`
  addition routes `classify_ci_changes.py` to run the Windows CI leg for the
  first time on this file. Fixed with `.replace("\r\n", "\n")` in both
  `architecture_tier1_targets` and `cli_spec_tier1_targets`, verified by
  rerunning `cargo test --test tier1_target_parity`. P2 — the new guard test
  wasn't catalogued in `docs/TESTING.md` unlike its sibling guard tests
  (`conformance_oracle_guard.rs`, `target_dir_literals.rs`, etc.). Fixed by
  adding the "Mechanical gate: Tier-1 target list..." subsection. P3 —
  `architecture_tier1_targets`'s backtick-harvesting is over-broad (would
  misparse a future backtick token in the table's Notes column); the
  reviewer explicitly called this "optional hardening only... not a
  blocker," so left as-is to avoid scope creep.
- **Round 2** (full three-file staged diff, after the round-1 fixes and the
  `docs/TESTING.md`/session-log additions — run because round 1's own text
  explicitly disclaimed coverage of anything beyond the two files it saw):
  confirmed the round-1 CRLF fix is correctly implemented and the new
  `docs/TESTING.md` text is accurate. Raised one new P1 — `.unwrap_or_else(||
  panic!(...))` in `indented_list_after` is a closure passed to a
  combinator, which `docs/TESTING.md`'s own coverage notes (line ~1063)
  describe as needing to actually execute to count as covered, and a
  Windows checkout that reaches `.find(marker)` only ever succeeds. On
  investigation this finding does not actually apply: `docs/TESTING.md` line
  1051 states test code under `tests/` is excluded from the coverage
  denominator entirely ("the gate measures product code exercised by tests,
  not tests covering themselves"), so `tests/tier1_target_parity.rs` is never
  instrumented in the first place regardless of this pattern — confirmed by
  running the full `cargo llvm-cov --workspace --fail-under-lines 100
  --fail-under-regions 100` gate both before and after touching this line
  and seeing the file absent from the coverage report either way. Left the
  closure as originally written (reverted an interim `.expect(&format!(...))`
  edit which would additionally have tripped `clippy::expect_fun_call` under
  this repo's `cargo clippy --workspace --all-targets -- -D warnings` gate).
  Two P3s from round 2 were adopted anyway as harmless, genuine
  improvements, independent of the coverage question: added a failure
  message with captured stderr to `binary_tier1_targets`'s
  `assert!(output.status.success())` for better debuggability, and confirmed
  (without code change) that the P3 backtick-harvesting note from round 1
  still applies unchanged. A third P3 (TESTING.md's CRLF rationale slightly
  overstates that both `include_str!` call sites needed the fix for
  correctness, when only `cli_spec_tier1_targets`'s did) was left as
  documented, non-blocking prose compression, consistent with the source
  comment already stating the precise distinction.
- No P0 finding in either round. No open actionable finding remains.

## Selection record (issue-select)

- Baseline: `origin/main` at `a6c3c819` (2026-08-27), no open pull requests.
- Milestone scope: v0.4 (active). Full open-issue inventory paginated (123
  open issues total); non-milestone count re-verified at 72, still over the
  D-192 20-item ceiling, so no new non-milestone issue was filed or
  proposed this round.
- Priority markers in v0.4 scope: the sole P1 (#20, "Make pycc_rt a real
  build/link dependency...") stays deferred — its Part 3 (#631) still needs
  the D-172/D-080 two-PR coexist-then-retire workflow-digest cycle
  (`scripts/check_roadmap_evidence.rb`'s frozen constants deep-compare
  `.github/workflows/ci.yml`'s cargo build/test ordering exactly), and #20's
  own remaining scope is itself workflow-adjacent per its own thread — this
  reconfirms the prior round's finding.
- P2 survivors scored for size/scope clarity: #246, #638, #24, #249, #414,
  #573, #585, #614, #618, #619, #636, #676, #693, #707, #714, #733, #824.
  During scoring, **#614** ("Install LLVM 22 can wedge ubuntu-latest — no
  timeout-minutes/wget retry") was checked against the same CI-workflow
  constraint that defers #20/#631. What was actually confirmed, by direct
  re-reading of `check_roadmap_evidence.rb:2198`
  (`validate_source_aware_perf_gate_lifecycle`): that function dispatches on
  which frozen job shape the live `frontend-perf-measure` job currently
  matches (`D56_...`, `D112_UBUNTU_FRONTEND_PERF_MEASURE_JOB`,
  `D203_SCRATCH_DEVDEP_FRONTEND_PERF_MEASURE_JOB`, ...) to pick the
  correspondingly authorized gate job — it is a shape-recognition `elsif`
  chain over that one job, not an assertion that job must stay byte-identical
  forever. Each prior LLVM/runner change (D-112, D-114, D-203) added its own
  new frozen constant and `elsif` branch in the *same* pull request that
  changed the job (D-203 did so in one PR, 2026-08-26). #631 is genuinely
  different: its target, `D91_COVERAGE_SCRIPT`, is compared because it embeds
  the exact `cargo build`/`cargo test` lines #631 proposes to *remove*, which
  is a real two-PR "stop reading them, then remove them" scenario.
  This session did **not** verify whether #614's own planned edit (an
  `install-llvm`-style step, not the `frontend-perf-measure` job) falls under
  this same dispatch mechanism, under a different `check_roadmap_evidence.rb`
  guard, or under the separate "manifest-protected path" staging rule that
  `docs/TESTING.md`'s `target_dir_literals.rs` section cites for
  `.github/workflows/ci.yml` generally (and that citation's own relationship
  to D-172's retirement of whole-file byte staging was not reconciled here
  either). Recording this narrower, confirmed fact instead of an earlier
  drafted overgeneralization ("#614 is not blocked") that mis-extended it: an
  earlier pass in this session had wrongly generalized the #631 two-PR
  pattern onto #614 in the *opposite* direction, and the corrected text above
  fixes that error without asserting the opposite conclusion in its place.
  Whoever picks up #614 next should re-verify against its actual diff before
  relying on either direction.
  - **#246** and **#638** were the two next-best-scoped survivors, matching
    the prior round's own note. #246 was chosen over #638: #246 is a
    pure test/docs-source-of-truth fix with zero production code paths
    touched (no codegen/runtime changes, no new exception-unwinding
    regression oracle to design), while #638 requires a bigint
    exception-edge codegen fix plus a new allocation/lifetime oracle test
    and a D-181 residual-list documentation update — larger scope, held
    for a subsequent round.
  - Other P2 peers (#24 rustfmt-gate-plus-reformat, #573/#585/#676/#693/#707/
    #714 — exception/PEP feature work with larger blast radius, #618 compile-
    time literal-range diagnostic, #619 a new mechanical LLVMString-drop
    checker script, #636 tuple-slot retain balance, #733 issue-select's own
    scoring-order meta-issue, #824 CLI arg-parsing tightening) were all
    larger in scope or required more design judgment than #246's
    self-contained parity test.
- Staleness screen: no provably stale issues found in this pass; tracker is
  well-tended (recent issues, active comment threads).
- Blocker screen: #246 has no dependency on any other open issue, no open
  PR touches `src/main.rs`, `docs/ARCHITECTURE.md`, `docs/CLI_SPEC.md`, or
  `tests/slice0.rs`/`tests/tier1_target_parity.rs`, and it needs no
  workflow-file edit at all.
- Premise verification: reproduced directly against current `main` —
  confirmed the four independent copies (`src/main.rs::TIER1_TARGETS`,
  `docs/ARCHITECTURE.md` table, `tests/slice0.rs` snapshot literal,
  `docs/CLI_SPEC.md` transcript) still exist with no binding mechanism, and
  reproduced the issue's own described mutation gap exactly (see above).
- Adversarial advisor round: consulted the `advisor` tool twice. The first
  round reviewed the full selection record, the initial #614 finding, and
  the #246-vs-#638 tradeoff; it did not object to the #246 pick, but caught
  a defect in this entry as first drafted — it had prematurely recorded
  "Status: Merged" and a completed advisor round before either had actually
  happened, corrected above and in this line. A second round, run after the
  D-014 coverage gate and D-068 reviewer pass on the implemented fix, flagged
  three more issues before commit: (1) the D-068 reviewer's findings needed
  re-verification against the diff as actually implemented plus the
  `docs/TESTING.md` addition, since round 1 of that review explicitly
  disclaimed coverage of anything beyond the two files it saw; (2) this
  entry's #614 paragraph asserted "#614 is NOT blocked" from reading only
  the `elsif` dispatch chain, without checking the fallthrough branch or
  reconciling `docs/TESTING.md`'s separate "manifest-protected path" D-103
  claim against D-172 — an overconfident directional call given what was
  actually verified, corrected above and in Follow-ups; (3) a coverage-report
  detail in a planned PR-body was wrong in mechanism (attributing a `tests/`
  file's exclusion to "the closure being counted as covered" rather than to
  `tests/*.rs` simply not being instrumented at all). All three are addressed
  in this entry and in the PR body; none changes the #246 selection itself.

## Follow-ups

- #614: this session confirmed only that `check_roadmap_evidence.rb:2198`'s
  `frontend-perf-measure` job check is a shape-dispatch `elsif` chain (not a
  byte-exact freeze), following the D-112/D-114/D-203 precedent — it did
  *not* confirm whether #614's actual planned edit is covered by that same
  mechanism, by a different guard, or by the separate D-103
  "manifest-protected path" claim in `docs/TESTING.md`'s
  `target_dir_literals.rs` section (whose relationship to D-172 was also not
  reconciled here). Whoever picks up #614 should verify against its concrete
  diff rather than relying on either directional claim from this entry. It
  remains a plausible next P2 candidate pending that check.
- #638 (bigint exception-edge leak) and #619 (mechanical LLVMString-drop
  guard) remain good next P2 candidates; neither was blocked, both were
  simply larger in scope than this round's pick.

## Where to resume

Re-enter `issue-select` step 1 with a fresh baseline. The v0.4 milestone
scope is unchanged; #614/#638/#619's standing candidacy are the most useful
context for the next round.
