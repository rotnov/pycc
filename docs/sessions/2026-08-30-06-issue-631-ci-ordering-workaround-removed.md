# Session handoff: #631 CI cargo build/test ordering workaround removed (closes #631, closes #20)

## Status: PR opened, carries `Fixes #631` and `Fixes #20`. Activation half of a two-PR stage-then-activate sequence.

This session is Stage 2 (activation) of a two-pull-request sequence for
issue #631, which the original PR #850's own body identified as "the final
part" of #20. Stage 1 (PR #851, "Stage the D-171 native-build-test
required-steps removal") merged to `main` as commit `5faa7f7d4f1044049774
121c626d0c1ed1c0451e`. Stage 1 removed the `"cargo build --workspace" =>
{...}` entry from `D171_NATIVE_REQUIRED_RUN_STEPS` in
`scripts/check_roadmap_evidence.rb`, purely additively: the live `ci.yml`
was left unchanged and still passed the checker against the old,
step-carrying shape. This PR activates the change in the live workflow.

## Why the split was necessary

An earlier attempt (PR #850, closed/superseded) tried to remove the
`cargo build --workspace` step from `.github/workflows/ci.yml` and relax
`scripts/check_roadmap_evidence.rb`'s `D171_NATIVE_REQUIRED_RUN_STEPS`
pin in the same commit. That failed the base-branch `audit` job: `audit`
is a `pull_request_target` job that validates a PR's *head* `ci.yml`
against `main`'s *already-merged* checker, so it only ever sees the
base's copy of `check_roadmap_evidence.rb` — which, at PR #850's time,
still required the `cargo build --workspace` step to exist in
`native-build-test`. Removing the step from `ci.yml` while the merged
checker still demanded it made `audit` fail on `main`'s own outdated
constant, regardless of what the PR's own diff did to the checker.

The fix follows the established D-103 coexist-then-activate pattern used
elsewhere in this repository for exactly this class of problem: split into
two ordinary PRs.

- **Stage 1 (PR #851, merged)**: relax the checker only. Removes the
  now-orphaned `D171_NATIVE_REQUIRED_RUN_STEPS` entry so the required-step
  pin no longer demands the step, while leaving `ci.yml` itself unchanged
  (it still has the step, which is harmless — the checker no longer
  requires it but does not forbid extra steps either). `audit`, seeing the
  base's pre-Stage-1 checker, was unaffected by this PR's diff to the
  checker script; its concern is the workflow file shape, not the checker
  script's own content, so this passed cleanly.
- **Stage 2 (this PR)**: activate. Removes the two now-redundant
  `cargo build --workspace` steps from `ci.yml`'s `build-test-coverage`
  and `native-build-test` jobs. `audit` now validates this PR's head
  `ci.yml` against `main`'s checker, which (after Stage 1's merge) no
  longer requires the step — so the removal passes.

## What changed in this PR

`.github/workflows/ci.yml`: removed the standalone `cargo build
--workspace` step in both `build-test-coverage` (after "Build pycc_rt for
x86_64-apple-darwin") and `native-build-test` (after "Verify oracle
version"), replacing each with an explanatory comment. These steps existed
only to pre-build the `pycc_rt` runtime artifact before `cargo test
--workspace` ran later in the same job. Issue #20's parts 1 (#629) and 2
(#630) made `pycc_codegen/build.rs` build `pycc_rt` itself as a real Cargo
build dependency, so a plain `cargo test --workspace` now triggers the
full workspace build on its own — a reintroduced missing build dependency
fails these jobs directly instead of hiding behind a leftover pre-built
artifact. (The unrelated `cargo build --workspace` step inside
`build-test-coverage`'s isolated coverage boundary, and the `governance`
job's `cargo build --workspace for offline alpha skill contract evals`
step, are untouched — neither is the ordering workaround this issue
targets.)

`scripts/check_roadmap_evidence.rb`: **not touched** in this PR — Stage 1
already removed the `D171_NATIVE_REQUIRED_RUN_STEPS` entry. Verified
directly (`grep -n "cargo build --workspace" scripts/check_roadmap_evidence.rb`
on the fresh branch tip) that only the unrelated `governance`-job entry
(`D171_GOVERNANCE_AGENT_STEPS`) remains, confirming Stage 1's premise still
holds.

`scripts/test_check_roadmap_evidence.rb`: in
`test_repository_ci_runs_the_self_tests_and_checker`, replaced the
ordering assertion (`assert_operator commands.index("cargo build
--workspace"), :<, commands.index("cargo test --workspace -- --include-
ignored")`) — which would fail once the step is actually gone from
`ci.yml` — with presence/absence checks on both jobs: `refute_includes`
the removed command, `assert_includes` the `cargo test --workspace --
--include-ignored` command, for both `build-test-coverage`'s existing
`commands` list and a newly extracted `native_commands` list (previously
this test never separately inspected `native-build-test`'s command list).
This addresses the forward-looking landmine PR #851's own deep-reviewer
flagged: the assertion reads the *live* `ci.yml`, so it would have broken
the moment this PR's `ci.yml` change landed if left as an ordering check.

`tests/fixtures/policy-successor-manifest.json`: updated the `sha256`
fields for `.github/workflows/ci.yml` and
`scripts/test_check_roadmap_evidence.rb` to their new content digests
(hygiene only, matching the established convention from prior sessions in
this repository — e.g. `docs/sessions/2026-08-30-05-issue-614-llvm-
install-timeout-activate.md` recorded the same field as functionally
unenforced but kept current for hygiene). `scripts/check_roadmap_evidence.rb`'s
own manifest entry was already updated by Stage 1 and needed no further
change here.

## Documentation impact: none beyond this handoff

This is a CI-hardening/cleanup change with no user-visible compiler
behavior, CLI flag, diagnostic, or language semantic change. `docs/
ROADMAP.md` and `docs/DELIVERY_PLAN.md` describe compiler/CLI capability,
not CI workflow internals, so neither needs an edit. No specification
under `docs/SPEC.md` covers CI workflow internals. Verified by grepping
both documents and `docs/decisions/*.md` for "#631" and "#20" and finding
no roadmap/plan prose describing this specific CI-ordering detail.

## Local gates

- `LANG=en_US.UTF-8 LC_ALL=en_US.UTF-8 ruby scripts/check_roadmap_evidence.rb .`:
  pass ("Roadmap evidence policy passed."). Same locale note as prior
  sessions: this environment's default locale trips the markdown
  blockquote scan with `invalid byte sequence in US-ASCII` without the
  `LANG`/`LC_ALL` override.
- `LANG=en_US.UTF-8 LC_ALL=en_US.UTF-8 ruby scripts/test_check_roadmap_evidence.rb`:
  pass, 244 runs / 1245 assertions / 0 failures / 0 errors.
- `LANG=en_US.UTF-8 LC_ALL=en_US.UTF-8 ruby scripts/check_ci_permissions.rb`:
  pass, 10 workflow files.
- `LANG=en_US.UTF-8 LC_ALL=en_US.UTF-8 ruby scripts/test_check_ci_permissions.rb`:
  pass, 39 runs / 171 assertions / 0 failures / 0 errors.
- `python3 scripts/test_classify_ci_changes.py`: pass, 23/23.
- `python3 scripts/generate_decisions_index.py docs/decisions docs/decisions/README.md --check`:
  pass, up to date.
- `cargo doc --workspace --no-deps`: succeeds; one pre-existing warning
  (`bind_class` linking to private `Self::bind_synthetic_class` in
  `crates/pycc_types/src/env.rs`), unrelated to this diff and not newly
  introduced by it.
- `cargo clippy --workspace --all-targets -- -D warnings`: exit 0. A
  handful of pre-existing `escaped newline` warnings in
  `tests/slice1_codegen_depth.rs` print but do not fail the build (not
  introduced by this diff, which touches no Rust source).
- No Rust source file changed by this diff (`git diff --stat` against the
  branch base shows only `.github/workflows/ci.yml`,
  `scripts/test_check_roadmap_evidence.rb`, and
  `tests/fixtures/policy-successor-manifest.json`), so the D-014
  100%-line/region-coverage gate does not apply — verified directly from
  the diff's file list rather than assumed.
- Empirically re-verified the underlying premise (that the removed step
  is genuinely redundant) from a clean, isolated `CARGO_TARGET_DIR` with
  no prior `cargo build --workspace` invocation:
  - `cargo build -p pycc_codegen --tests`: succeeds, building `pycc_rt`
    and all of `pycc_codegen`'s other workspace dependencies from
    scratch via `pycc_codegen/build.rs`'s own `cargo build -p pycc_rt`
    call.
  - `cargo test -p pycc_codegen`: 398 passed, 0 failed.
  - `cargo test --workspace`: exit 0, every `test result:` line across
    all crates and doc-tests reads `ok`, zero `FAILED` lines anywhere in
    the full output (the `error[...]` diagnostic text visible in the log
    is expected compiler self-test output — pycc's own diagnostics test
    suite intentionally exercises rejection paths like unsupported
    dataclass/enum shapes and out-of-range integer literals — not a test
    failure).

## Local pinned reviewer (`ievo:deep-reviewer`)

Dispatched and awaited synchronously in this session. See the PR
description / coordinating report for the exact verdict; any actionable
finding was addressed before push.

## Confirmed: this PR closes #631 and #20

`gh api graphql` confirmation of `closingIssuesReferences` (expected
`totalCount: 2`, nodes `{20, 631}`) is run after opening the PR — see the
PR itself for the exact query result.
