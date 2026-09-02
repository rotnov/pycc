# 2026-09-02-05 -- Issue #869: name the CARGO_TARGET_DIR recovery in the missing pycc_rt diagnostic

## Status: delivered

Worktree: `/Users/denis/projects/pycc-proto/.claude/worktrees/autopilot-2026-09-02-04`,
branch `claude/autopilot-2026-09-02-04`, started from `origin/main` at
`a9dbb61e`. Issue [#869](https://github.com/rotnov/pycc/issues/869)
(milestone v0.4, follow-up filed by the #639 / D-216 iteration) was
resolved via pull request [#874](https://github.com/rotnov/pycc/pull/874),
squash-merged as `488dcbf7`. The pull request's `closingIssuesReferences`
count was verified as 1 before merge; this handoff entry deliberately
carries no closing keyword.

## How this task was run

One autopilot iteration under the standing `fix all opened issues`
directive: D-021 preflight, `issue-select` over the full open-issue list
(98 open issues at `a9dbb61e`; milestone scope v0.4; #869 was the
top-ranked in-scope survivor after #712 closed), then `issue-implement`
with `issue-to-plan` and the implementation each dispatched into an
isolated agent (D-142/D-143).

- Plan: <https://github.com/rotnov/pycc/issues/869#issuecomment-5508260926>
  (two adversarial rounds; round 2 produced no plan-changing findings).
  The issue allowed a reasoned "no"; the plan took the "yes" branch
  because D-216 had documented the `CARGO_TARGET_DIR` recovery in
  `docs/CLI_SPEC.md` only, leaving it undiscoverable from the terminal.
- Changes: both `no pycc_rt build found` literals in
  `crates/pycc_artifact_layout/src/lib.rs` (`find_pycc_rt_lib_dir_in`)
  now end with `, or run pycc with CARGO_TARGET_DIR set to the directory
  Cargo built into.`; the message stays single-line and keeps the
  `Run \`cargo build ... -p pycc_rt\` first` substring verbatim. The four
  missing-build unit tests and the `tests/slice0.rs` cross-target
  end-to-end test assert the new clause. `docs/CLI_SPEC.md` re-quotes the
  message and replaces the "retained verbatim" rationale with the
  two-recovery explanation. `.harden/` carries the round-1 findings pile
  and two recurrence-counter incident notes.
- Gates (local, at `a9dbb61e`): `cargo fmt --all --check`, clippy with
  `-D warnings`, `cargo test --workspace` (4068 passed), llvm-cov 100%
  lines and regions, scripts unittest (981 OK), roadmap-evidence and
  ci-permissions checkers, `cargo doc --workspace --no-deps` -- all green.
  D-068 `ievo:deep-reviewer` round 1: no blockers or warnings; one
  pronoun-antecedent note fixed, one note refuted against the plan's
  rejected-alternatives section.
- CI: full selection green on the initial head; the branch went `BEHIND`
  when #875 (#867, another session) merged, was updated through the
  GitHub update-branch API, and went green again (`CLEAN`) before the
  squash merge. No review threads were opened.

## Deviations and notes

- The dispatched cycle agent was terminated by a session rate limit after
  the PR reached `READY`; the coordinating session performed the branch
  update, the final precondition check, the diff re-read, the merge, and
  this handoff itself.
- Session-file sequence: `02` and `04` today belong to the #866/#867
  iterations run by another session, hence `05` here.

## Paused autopilot

- Directive scope: open-ended (`fix all opened issues`); milestone scope
  v0.4 in effect.
- Last iteration outcome: #869 delivered (#874 -> `488dcbf7`).
- Next step: fresh D-021 baseline from `origin/main`, then re-enter
  `issue-select` (v0.4 first). Another session is delivering #864's
  remaining part (#868); treat it as claimed.
- In-run denylist: none.
- Known follow-ups not filed (D-192 ceiling exceeded, 68 non-milestone
  open issues vs. 20): the `tests/conformance.rs:699-701` "unlike every
  other test in this file" overclaim noted by the #712 iteration.
