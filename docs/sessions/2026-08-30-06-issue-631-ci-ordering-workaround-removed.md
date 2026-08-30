# 2026-08-30-06 — issue #631 (Part 3 of #20): CI build/test ordering workaround removed

## Baseline

- Default branch: `origin/main` at `8a855d64c0e5eea31af7e51e8b978d7e2d117505`.
- No open pull requests at this checkpoint.
- Task worktree: `/Users/denis/projects/pycc-proto/.claude/worktrees/agent-a74e90810d067f0e2`,
  branch `agent-issue-select-2026`, off that exact tip.

## Selection

`issue-select`'s milestone-scope-first ordering (v0.4 active) reaches #631 as the sole
remaining actionable step of parent issue #20 (`P1: Make pycc_rt a real build/link dependency
and honor Cargo artifact paths`, v0.4, still open). #20 was decomposed per AGENTS.md's own
convention into Part 1 (#629, merged), Part 2 (#630, merged), and Part 3 (#631, this PR). Per
that convention the parent stays open until every sub-issue closes, so #631 is not an
independent unmarked issue competing against other v0.4 P2/P3 peers under D-111's marker
ranking — it is the continuation of already-selected, two-thirds-merged P1 work. Consulted the
advisor on this exact point rather than inventing a D-211 evidence-bound critical-path escape
(no `docs/ROADMAP.md` v0.4 Accept clause names this work, so the escape's quote requirement
cannot be met, and it would be the wrong tool regardless: D-211 ranks *competing* survivors,
and #631 is not a competitor to anything, it is #20's own remaining implementation).

Staleness screen was bounded to the #20/#630/#631 dependency chain (not a full sweep of the
~72-issue non-milestone backlog); no milestone triage assignments were made this run. The
non-milestone ceiling (72, over the 20 cap) is unaffected since this PR closes milestone-scoped
issues.

## The D-103 premise correction

#631's own issue body asserts this must follow the two-PR D-103 stage-then-activate cycle
because `.github/workflows/ci.yml` is a `path` entry in
`tests/fixtures/policy-successor-manifest.json`. Verified against the current tree and found
incorrect, consistent with #275's prior finding (`docs/sessions/2026-08-22-06-issue-275-d103-residue-pr-opened.md`):

- `workflow-policy.yml`'s `audit` job reads the manifest only to materialize a bounded set of
  paths from the PR head tree (`findEntry` throws if a listed path is *absent*); it never
  compares the manifest's `sha256` field against fetched content — `parseManifestPaths` only
  extracts `path`/`source_path`. Renaming, deleting, or moving a listed path still requires a
  same-PR manifest update; *editing* a listed path's content does not, on this account.
- `scripts/check_roadmap_evidence.rb`'s `validate_evidence` dispatches on
  `d171_routed_workflow?` and takes the D-171 structural-validation branch for the current live
  `ci.yml` (it has both `classify-changes` and `governance` jobs), bypassing the whole-file
  `REVIEWED_PERF_CI_WORKFLOW_SHA256S` digest allowlist entirely — that is D-080's own
  still-live two-pull-request digest cycle, and it is genuinely dead code for this tree, not
  merely satisfied.

So no two-PR split was needed for `ci.yml` itself. There *was* one genuine same-PR companion
requirement the issue's own text did not anticipate: `check_roadmap_evidence.rb` pins the
`native-build-test` job's exact step list via `D171_NATIVE_REQUIRED_RUN_STEPS`, which named the
now-removed `cargo build --workspace` step explicitly. Discovered by running the checker
locally after editing `ci.yml` — `ruby scripts/check_roadmap_evidence.rb` failed immediately
with `native-build-test step "cargo build --workspace" does not match the reviewed D-171
routing`. Fixed by removing that entry from the same constant in the same commit (see below);
this is an ordinary in-repo checker-and-workflow co-edit, not the D-103/D-080 staged mechanism.

## Premise verification

Confirmed empirically, not just by reading `pycc_codegen/build.rs`, that removing the ordering
step is safe:

- `cargo build -p pycc_codegen --tests` — exit 0, from a fresh `CARGO_TARGET_DIR`.
- `cargo test -p pycc_codegen` — 398 passed, 0 failed, from the same fresh target dir, no prior
  `cargo build --workspace`.
- `cargo test --workspace` — full workspace suite, same fresh target dir, exit 0.

All three ran under an isolated `CARGO_TARGET_DIR` outside the repo tree, confirming
`pycc_codegen/build.rs`'s own `cargo build -p pycc_rt` (landed by #630) is what actually
produces the runtime archive that CI's now-removed step used to pre-build.

## What changed

- `.github/workflows/ci.yml`: removed the `cargo build --workspace` step from both
  `build-test-coverage` and `native-build-test`, each immediately preceding that job's
  `cargo test --workspace` step. Left a comment explaining why the plain `cargo test` call now
  suffices. The coverage job's *isolated* `run_isolated "$TRUSTED_CARGO" build --workspace`
  (inside the D-014 hard-gate sandbox, a different step entirely) is untouched — it exists to
  produce the coverage-instrumented build, not to order an artifact ahead of a later test run.
- `scripts/check_roadmap_evidence.rb`: removed the now-orphaned `"cargo build --workspace" =>
  { "run" => "cargo build --workspace" }` entry from `D171_NATIVE_REQUIRED_RUN_STEPS`.
- `scripts/test_check_roadmap_evidence.rb`: replaced
  `test_repository_ci_runs_the_self_tests_and_checker`'s ordering assertion (which asserted the
  removed step's index preceded `cargo test --workspace`'s) with a `refute_includes` proving the
  step is gone, plus a plain presence check for the `cargo test` command it used to precede.
- `tests/fixtures/policy-successor-manifest.json`: refreshed the `sha256` fields for
  `.github/workflows/ci.yml`, `scripts/check_roadmap_evidence.rb`, and
  `scripts/test_check_roadmap_evidence.rb` to match their new content. This field is not read
  by any current gate (confirmed by inspection of `workflow-policy.yml` and both checkers) but
  every prior PR touching these paths has kept it in sync as a documentation-hygiene practice,
  most recently `8a855d64` (#849); this PR follows the same practice.
- No other documentation described the manual build-then-test ordering as current-state text
  (checked `docs/TESTING.md`, `docs/ROADMAP.md`, `docs/DELIVERY_PLAN.md`, and the D-091/D-184
  decision records); `docs/TESTING.md`'s coverage-gate paragraph refers to the untouched
  in-sandbox `run_isolated ... build --workspace` step and remains accurate.

No Rust source changed, so D-014's coverage gate does not apply to this change; verified this
explicitly rather than assuming it (`git diff --stat` touches only a workflow file, a Ruby
checker, its test, and a JSON fixture manifest).

## Gates (all run by this session as sole writer)

- `cargo build --workspace` — clean.
- `cargo clippy --workspace --all-targets -- -D warnings` — clean (pre-existing, unrelated
  `slice1_codegen_depth.rs` string-literal warnings only; exit 0).
- `cargo doc --workspace --no-deps` — clean (pre-existing, unrelated private-intra-doc-link
  warning in `pycc_types/src/env.rs:308` only).
- `ruby scripts/check_ci_permissions.rb .github/workflows` — passed for 10 files.
- `RUBYOPT="-E UTF-8" ruby scripts/check_roadmap_evidence.rb .` — passed.
- `RUBYOPT="-E UTF-8" ruby scripts/test_check_roadmap_evidence.rb` — 244 runs, 1241 assertions,
  0 failures, 0 errors (one failure surfaced before the test fix above; fixed and re-run green).
- `RUBYOPT="-E UTF-8" ruby scripts/test_check_ci_permissions.rb` — 39 runs, 171 assertions, all
  green.
- `python3 scripts/test_classify_ci_changes.py` — 23 tests, OK.
- `python3 scripts/generate_decisions_index.py docs/decisions docs/decisions/README.md --check`
  — up to date (no new ADR needed; this implements D-184's already-recorded consequence rather
  than making a new irreversible design choice).
- Cargo test suites: see Premise verification above (isolated verification directory, separate
  from the gates run in the actual worktree).

## Review

D-068 pinned reviewer (`ievo@ievo-skills` `deep-reviewer`), one round: 2 findings, both
`note`-level, no blockers, diff reported as ready to commit as-is.

1. `D171_NATIVE_REQUIRED_RUN_STEPS`'s presence-only checks don't guard against the removed
   `cargo build --workspace` step being reintroduced into `native-build-test` later (unlike
   `build-test-coverage`, which now has an explicit absence assertion). Addressed: added a
   sibling `refute_includes` for `native-build-test`'s own command list in
   `test_repository_ci_runs_the_self_tests_and_checker`, mirroring the existing
   `build-test-coverage` one.
2. A blank line was lost between two adjacent comment blocks in `ci.yml` at both edit sites
   (`build-test-coverage` and `native-build-test`), making the boundary between "why nothing is
   here" and "why the next step exists" slightly ambiguous. Addressed: restored both blank
   lines.

Both fixes verified green afterward: `ruby scripts/check_roadmap_evidence.rb`,
`ruby scripts/check_ci_permissions.rb`, and `ruby scripts/test_check_roadmap_evidence.rb`
(244 runs, 1245 assertions, 0 failures/errors) all still pass.

## Where to resume

This PR is expected to carry `Fixes #631` and `Fixes #20` (the last of #20's three
decomposition parts). If it has not merged yet, check its current CI and review-thread status
before doing anything else. Once merged, `docs/ROADMAP.md`/`docs/DELIVERY_PLAN.md` need no
edit — #20 is not roadmap-visible acceptance evidence for any milestone criterion, it is an
internal build-correctness fix. Re-enter `issue-select` at step 1 with a fresh baseline for the
next iteration.
