# 2026-08-31-04: Issue #24 rustfmt gate activation (Part 3 of 3, final)

## Status

Complete. Issue #24 ("P2: Add a rustfmt CI gate and format the merged Rust
sources") is now **CLOSED** with `stateReason: COMPLETED`.

All three parts are merged into `main`:

- Part 1 (PR #860, merge commit `7107fc20`): formatted the Rust workspace
  with the pinned rustfmt toolchain.
- Part 2 (PR #861, merge commit `92c19a20`): staged the tolerant/additive
  `validate_optional_rustfmt_gate` check in `scripts/check_roadmap_evidence.rb`
  plus the target-bytes fixture `tests/fixtures/d215-rustfmt-gate-ci.yml`,
  documented in `docs/decisions/D-215-stage-a-tolerant-rustfmt-ci-gate-check-ahead-of.md`.
- **Part 3 (PR #862, merge commit `b8307ea17dc29ea336605fb0c4e2af3c170e05ec`,
  this session)**: activated the gate.

`origin/main` tip immediately before writing this entry: `b8307ea17dc29ea336605fb0c4e2af3c170e05ec`
(re-fetched and re-verified via `git fetch`/`gh api graphql` right before this
commit).

## What Part 3 did

- Applied `tests/fixtures/d215-rustfmt-gate-ci.yml`'s bytes to
  `.github/workflows/ci.yml` byte-for-byte (verified with `cmp`): adds a
  `rustfmt` job that runs `cargo fmt --all -- --check` under the pinned
  toolchain, and extends `ci-gate`'s `needs`/failure condition to require it.
- Folded `"rustfmt" => ["compiler", "classify-changes"]` into
  `D171_OPTIONAL_ROUTING` in `scripts/check_roadmap_evidence.rb`, and deleted
  the now-dead tolerant-check scaffolding (`validate_optional_rustfmt_gate`,
  `D215_RUSTFMT_CI_GATE_NEEDS`, `D215_RUSTFMT_CI_GATE_FAILURE_CONDITION`,
  `D215_RUSTFMT_CLASSIFIER_OUTPUT`, and the `rustfmt_activated` ternary
  branching in the ci-gate needs/failure-condition checks) exactly as D-215
  planned.
- Kept a permanent, unconditional job-*shape* assertion for the rustfmt job's
  steps, renamed `D215_RUSTFMT_JOB` -> `D171_RUSTFMT_JOB`, since the generic
  `D171_OPTIONAL_ROUTING` loop only ever compares `needs`/`if`, never a job's
  own steps -- every other content-bearing `D171_OPTIONAL_ROUTING` member
  already has a dedicated content validator elsewhere in
  `validate_d171_ci_routing`. Dropping this assertion (a literal reading of
  "delete `validate_optional_rustfmt_gate` and its constants as dead code"
  could have suggested that) would have left a malformed `cargo fmt`
  invocation (missing `--check`, a stray `continue-on-error`) silently
  unchecked. This reasoning is recorded as an "Update (activation, Part 3 of
  #24)" appended to `docs/decisions/D-215-stage-a-tolerant-rustfmt-ci-gate-check-ahead-of.md`
  (the decision's original body was left untouched, per the append-only
  convention for accepted decisions).
- Updated the shared D-171 test-baseline fixture
  `tests/fixtures/policy-successors/ci-d171.yml` to include the same rustfmt
  job (now a mandatory `D171_OPTIONAL_ROUTING` member, so every test built on
  the `d171_workflow` helper needed it), and recomputed the
  `D171_CHANGE_AWARE_CI_WORKFLOW_SHA256` digest that pins that fixture's
  bytes.
- Updated `scripts/test_check_roadmap_evidence.rb`: added `"rustfmt"` to
  `D171_COMPILER_JOBS` (so the existing generic per-optional-job negative
  test now covers rustfmt's `needs`/`if` automatically), bumped the
  checkout-location count in
  `test_d171_rejects_each_mutable_or_credential_persisting_checkout` from 10
  to 11, and rewrote the D-215-specific test block: removed the now-dead
  `d215_rustfmt_activated_workflow` helper and its three dependent tests
  (redundant once the baseline fixture always includes rustfmt), kept
  `test_d215_rustfmt_gate_fixture_is_accepted_by_d171_routing` (retained
  audit evidence, since the fixture stays byte-identical to live `ci.yml`),
  and kept `test_d215_rustfmt_gate_rejects_a_malformed_job_shape` /
  `test_d215_rustfmt_gate_rejects_continue_on_error`, rewritten to mutate
  `d171_workflow` directly. The continue-on-error test's `expected_context`
  changed from "rustfmt job shape" to "failure propagation" because a
  separate, pre-existing generic `d171_require_failure_propagation` check
  (applied to every D-171 job's steps) fires first for that specific
  mutation.

## Gate results (this session, local, before opening the PR)

- `ruby scripts/test_check_roadmap_evidence.rb`: 247 runs, 1270 assertions,
  0 failures/errors.
- `ruby scripts/check_roadmap_evidence.rb`: "Roadmap evidence policy
  passed."
- `ruby scripts/test_check_ci_permissions.rb`: 39 runs, 0 failures/errors.
- `ruby scripts/check_ci_permissions.rb`: "Workflow permission policy
  passed for 10 file(s)."
- `cargo fmt --all -- --check`: clean (the actual point of this issue).
- `cargo clippy --workspace --all-targets -- -D warnings`: clean (exit 0).
- `cargo llvm-cov --workspace --fail-under-lines 100 --fail-under-regions
  100` (run under an isolated `TMPDIR`): 100.00% lines, 100.00% regions,
  0 missed, across the whole workspace. No Rust source was touched by this
  PR, so this run is a confirmation, not new coverage.
- Pinned local reviewer (`ievo:deep-reviewer`, dispatched directly via the
  `Agent` tool's `subagent_type`, per the D-068/D-155 binding): 0 findings.
- CI on PR #862 (watched via
  `.claude/skills/gha-watch-ci-pr/scripts/ci-watch.sh`): READY -- all checks
  green, `mergeStateStatus` CLEAN.
- Final pre-merge check via `gh api graphql`: `mergeStateStatus: CLEAN`,
  `mergeable: MERGEABLE`, 0 unresolved review threads,
  `closingIssuesReferences.totalCount: 1` naming exactly `{24}`.

## One deviation from the literal task instructions (D-127 judgment call)

The dispatching instruction asked for a brand-new isolated worktree at
`/Users/denis/projects/pycc-worktrees/issue-24-rustfmt-gate-activate`. This
session's Bash sandbox hard-refuses any command that targets a directory
outside its own assigned worktree (`.claude/worktrees/agent-a5f4191ba803b2c2f`),
including via `cd`/`git -C` redirection, so that path was technically
unreachable. This session's own assigned worktree was already sitting at the
correct base commit (`92c19a20`, `origin/main`'s tip) with a clean tree, so a
new task branch (`claude/issue-24-rustfmt-gate-activate`) was created there
instead. This preserves the substance of the D-021 preflight (isolated
worktree, latest `origin/main`, clean base, dedicated task branch) even
though it reused an existing worktree rather than creating a new one at the
literal path. No functional difference resulted.

## Where to resume

Nothing outstanding for issue #24 -- all three parts are merged and the
issue is closed. A fresh session picking up autonomous work on this
repository should return to the standing `next-milestone`/`issue-select`
loop rather than anything related to #24.
