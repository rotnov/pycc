---
id: D-215
title: "Stage a tolerant rustfmt CI-gate check ahead of activating the rustfmt job (issue #24, Part 2)"
status: accepted
---

## D-215: Stage a tolerant rustfmt CI-gate check ahead of activating the rustfmt job (issue #24, Part 2)

- Status: accepted
- Context: issue #24 asks for a required `cargo fmt --all -- --check` CI gate, using the
  exact pinned toolchain from `rust-toolchain.toml`, wired into `ci-gate`'s `needs:` and
  failure condition the same way `pages-performance`/`pages-accessibility` are. Part 1
  (#860) already formatted the merged Rust sources; this decision covers only the gate
  itself. `.github/workflows/workflow-policy.yml`'s `audit` job checks out
  `scripts/check_roadmap_evidence.rb` at `ref: ${{ github.sha }}` under
  `pull_request_target`, which always resolves to `main`'s HEAD, never the PR branch's --
  so one pull request that both edits `ci.yml` and teaches the checker to accept the new
  shape can never pass its own audit (the same D-080/D-090 constraint).

  `ci.yml` is a D-171-routed workflow: `scripts/check_roadmap_evidence.rb`'s
  `d171_routed_workflow?` detects the `classify-changes`/`governance` jobs and delegates
  to `validate_d171_ci_routing` instead of the older whole-file
  `REVIEWED_PERF_CI_WORKFLOW_SHA256S` digest allowlist that D-080/D-090/D-091 and their
  predecessors used (that allowlist is only reachable through
  `validate_d171_ci_routing`'s synthetic "unrouted" reconstruction, which replaces
  `ci-gate` with the frozen `D199_PAGES_ACCESSIBILITY_CI_GATE_JOB` before delegating, so
  a live-routed `ci-gate`'s actual shape is never checked against it). Within
  `validate_d171_ci_routing`, `ci-gate`'s `needs:` and failure-condition `if:` are
  asserted with `==` against `D171_CI_GATE_NEEDS`/`D171_CI_GATE_FAILURE_CONDITION`, which
  are themselves computed from the `D171_OPTIONAL_ROUTING` map (job name -> classifier
  output + needs expression) rather than hand-maintained literals. Adding `"rustfmt"` to
  `D171_OPTIONAL_ROUTING` in this pull request would therefore make the checker require
  `ci-gate` to already depend on a `rustfmt` job that does not yet exist in `ci.yml` --
  breaking every push to `main` between this merge and the later activation merge. The
  D-080/D-090 whole-file-digest staging pattern does not transfer here either: it stages
  a second accepted digest for the *whole* workflow file, which has no equivalent once a
  routed workflow's `ci-gate` shape is derived structurally rather than pinned by hash.
- Decision: add a new, independent, tolerant check --
  `validate_optional_rustfmt_gate` -- called from `validate_d171_ci_routing` alongside
  (not replacing) the existing D-171 checks, and leave `D171_OPTIONAL_ROUTING`,
  `D171_JOB_NAMES`, and `D171_CANDIDATE_CHECKOUT_JOBS` untouched in this PR. The new
  check accepts exactly two shapes for every D-171-routed workflow:
  1. no `rustfmt` job present and `ci-gate`'s `needs:` does not list `rustfmt` (today's
     `main`, and every push between this merge and activation); or
  2. a `rustfmt` job present that matches the frozen `D215_RUSTFMT_JOB` constant
     byte-for-byte, and `ci-gate`'s `needs:`/failure condition extended by exactly the
     clauses that `D171_OPTIONAL_ROUTING.merge("rustfmt" => ["compiler",
     "classify-changes"])` would have produced, computed the same way
     `D171_CI_GATE_NEEDS`/`D171_CI_GATE_FAILURE_CONDITION` are computed today rather than
     hand-duplicated as a second literal.

  Any other shape (job present but malformed, job present but not wired into `ci-gate`,
  `ci-gate` referencing `rustfmt` without the job existing, etc.) is rejected. The
  target job design: `runs-on: ubuntu-latest` (no LLVM/Homebrew, it never compiles),
  `needs: classify-changes`, `if: needs.classify-changes.outputs.compiler == 'true'`
  (same routing as `build-test-coverage`), the pinned checkout
  (`actions/checkout@d23441a48e516b6c34aea4fa41551a30e30af803 # v6`,
  `persist-credentials: false`), the house `Show pinned toolchain` / `rustup show` step,
  then `rustup component add rustfmt` and `cargo fmt --all -- --check` with no
  `continue-on-error`. The complete intended final `ci.yml` bytes are preserved verbatim
  in `tests/fixtures/d215-rustfmt-gate-ci.yml` so the later activate pull request can
  `cp` them in without re-deriving the shape, and `scripts/test_check_roadmap_evidence.rb`
  gains tests exercising both the absent and the present branch of the new check.
  `REVIEWED_PERF_CI_WORKFLOW_SHA256S` is not extended: that allowlist is unreachable for
  a routed `ci-gate`, so a digest entry there would be dead weight, not a real trust
  anchor, for this change.

  The activate pull request folds `"rustfmt" => ["compiler", "classify-changes"]` into
  `D171_OPTIONAL_ROUTING` (which then supersedes `D215_RUSTFMT_JOB` and the tolerant
  check's presence branch automatically through the existing D-171 machinery), deletes
  `validate_optional_rustfmt_gate` and its constants as dead code, and applies the exact
  fixture bytes to `ci.yml` -- all three in the same commit, since after that flip the
  tolerant path can never again be exercised and D-014's coverage gate does not apply to
  Ruby, so an unreachable branch there is a review defect rather than a coverage failure.
- Alternatives:
  - Stage a second whole-file digest in `REVIEWED_PERF_CI_WORKFLOW_SHA256S`, per the
    pre-D-171 D-080/D-090/D-091 pattern -- rejected: `d171_routed_workflow?` bypasses that
    allowlist entirely for the live workflow, so the digest would never be consulted and
    would misrepresent the actual trust mechanism to a future reader.
  - Add `"rustfmt"` to `D171_OPTIONAL_ROUTING` directly in this staging PR and accept
    that `main` fails validation until the activate PR lands -- rejected: this is exactly
    the release-blocking breakage the two-PR split exists to avoid; a red `main` between
    merges is not an acceptable cost of staging.
  - Collapse the job to a single `run:` step (folding `rustup show` and
    `rustup component add rustfmt` into the same block as `cargo fmt --all -- --check`)
    so it could reuse the existing single-step `D171_PAGES_GATE_RUNS` pattern instead of
    a new dedicated shape check -- rejected: it would hide the toolchain-selection step
    that every other Rust-toolchain job in `ci.yml` surfaces by name in its own log line,
    for no benefit since a dedicated shape check is no more code than reusing the
    single-step pattern would have been.
- Consequences: this pull request changes no live CI behavior -- `.github/workflows/ci.yml`
  is untouched, and every workflow lacking a `rustfmt` job (including today's `main`)
  keeps passing `validate_d171_ci_routing` unchanged. The tolerant check is deliberately
  temporary scaffolding: it must be deleted, not merely superseded, by the activate pull
  request, or it becomes permanent dead code the moment `D171_OPTIONAL_ROUTING` gains the
  `rustfmt` entry. Until activation, issue #24 stays open with the gate itself
  unenforced; only its design, fixture, and staged checker support are landed here.
