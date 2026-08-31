# 2026-08-31 — #24 Part 1: format the workspace with the pinned rustfmt

## Overall status

One dispatched task on branch `claude/issue-24-rustfmt-gate`, delivering
[PR #860](https://github.com/rotnov/pycc/pull/860) against `main` at
`ae80a461` — Part 1 of 3 for #24 ("P2: Add a rustfmt CI gate and format the
merged Rust sources"). At the time this entry is written the PR's head is
`eba80ef2`, `mergeable: MERGEABLE`, `mergeStateStatus: BLOCKED` only because
`build-test-coverage`, one `native-build-test` matrix leg, and
`frontend-perf-measure` are still `IN_PROGRESS`; every check that has finished
so far (`classify-changes`, `status-page-freshness`, `audit`, `governance`,
two `native-build-test` legs, `cross-compile-build`, `cross-compile-verify`)
is `SUCCESS`. This session is watching the remaining checks to completion via
the sanctioned `ci-watch.sh` skill before merging; the merge step and this
file's own final state are recorded in the task's closing report, not here,
since D-192 requires this snapshot to land in the same PR that delivers the
work rather than be edited afterward.

## What was delivered

- Ran `cargo fmt --all` under the pinned toolchain (`rust-toolchain.toml`,
  1.97.1) across the whole workspace: 51 files, 1025 insertions(+), 622
  deletions(-), pure whitespace/line-wrap/brace-placement diff with no
  literal, operator, argument-order, or assertion content change.
- `cargo fmt --all -- --check` moved from 4731 divergent lines (exit 1) to 0
  (exit 0).
- Re-verified semantic equivalence: `cargo build --workspace`,
  `cargo test --workspace` (all 72 test binaries, 0 failed),
  `cargo clippy --workspace --all-targets -- -D warnings` (0 warnings),
  `cargo llvm-cov --workspace --fail-under-lines 100 --fail-under-regions 100`
  (TOTAL 100.00% lines / 100.00% regions, unchanged), and
  `ruby scripts/check_ci_permissions.rb` (unaffected, confirming no CI file
  was touched — none was; this PR is deliberately scoped away from
  `.github/workflows/ci.yml` and `scripts/check_roadmap_evidence.rb` per
  D-080's stage-then-activate requirement, since a single PR that both edits
  `ci.yml` and updates its own pinned digest is self-defeating against the
  `pull_request_target` `audit` job).
- Dispatched the D-068 pinned local reviewer (`ievo:deep-reviewer`) against
  the full staged diff (`/tmp/staged_diff.txt`, 4893 lines): **0 findings**,
  confirmed formatting-only.
- PR body originally read "**This PR does not close #24.**" — this is
  exactly the AGENTS.md closing-keyword negation trap (GitHub's
  closing-keyword scan ignores surrounding English). Caught by this session's
  own mandated `gh api graphql … closingIssuesReferences` check
  (`totalCount: 1`), corrected to "**#24 stays open after this merges.**",
  re-verified `totalCount: 0`.

## Known follow-ups

- **PR-B**: stage the new `ci.yml` digest in
  `scripts/check_roadmap_evidence.rb` (D-080), once this Part 1 lands.
- **PR-C**: activate `cargo fmt --all -- --check` as a required CI gate in
  `ci.yml`, plus contributor/agent documentation for the local command.
  PR-C should re-run `cargo fmt --all` immediately before activating the
  gate rather than assume the tree formatted here is still clean — nothing
  in CI enforces formatting yet, so `main` can drift back out of format
  before PR-C lands.
- Issue #24 stays open until both land.

## Judgment call recorded

`docs/TESTING.md`'s sanctioned coverage invocation
(`run_isolated "$TRUSTED_COV" llvm-cov …`) runs inside a `sudo -u nobody
env -i` sandbox with isolated HOME/Cargo-home/temp/target directories, and
the document provides no separate plain-local-dev command. This session ran
`cargo llvm-cov --workspace --fail-under-lines 100 --fail-under-regions 100`
directly, unsandboxed, with an isolated `TMPDIR`, as an adequate local
approximation for a pure formatting diff — such a diff cannot plausibly
change line/region hit counts, and CI's own `build-test-coverage` job (run
inside the real sandbox) is the authoritative gate this PR is blocked on
before merge.
