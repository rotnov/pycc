# Session handoff: issue #247 — report the actual rustc version, not Cargo's MSRV

- Status: implementation, docs, and a post-review fix round are complete and
  green on branch `issue-247-actual-rustc-version`, based on `origin/main`
  tip `ef581ad1` (the #748 merge commit). PR
  [#751](https://github.com/rotnov/pycc/pull/751) is open,
  `closingIssuesReferences.totalCount == 1` (closes exactly #247), and the
  one automated-review thread is resolved.
- What shipped: `pycc version --verbose` previously printed the `rustc`
  field from `env!("CARGO_PKG_RUST_VERSION")` — the manifest's declared
  `rust-version` (MSRV contract), not the compiler that actually produced
  the binary. A newer installed `rustc` than the declared minimum builds
  cleanly and silently makes the two diverge. A new root `build.rs` shells
  out to the real `rustc` (the `RUSTC` env var Cargo sets for build
  scripts, falling back to `rustc` on `PATH`) at build time and exposes its
  version through a new `PYCC_BUILD_RUSTC_VERSION` compile-time env var
  (`cargo::rustc-env=...`), following the same directive style already used
  by `crates/pycc_codegen/build.rs`. `Cargo.toml` gained `build = "build.rs"`.
  `src/main.rs`'s version arm now reads `PYCC_BUILD_RUSTC_VERSION` instead
  of `CARGO_PKG_RUST_VERSION`. `docs/ROADMAP.md` and `docs/CLI_SPEC.md`
  updated to describe the new build-time-capture behavior.
- Review loop (D-068/D-155 plus one bot-authored round):
  - After the branch was pushed and PR #751 opened, an automated
    `chatgpt-codex-connector[bot]` review left one thread that blocked
    merge via GitHub's required-conversation-resolution setting: the
    initial `tests/slice0.rs` implementation re-invoked `rustc --version`
    itself at test run time as an "independent" check, but Cargo only sets
    the `RUSTC` env var for build-script invocations, not for the test
    binary — so when the compiler is pinned via `.cargo/config.toml`
    rather than the `RUSTC` environment variable, the test's own fallback
    to `rustc` on `PATH` could silently check a different compiler than
    `build.rs` actually captured, an environment-dependent false
    positive/negative independent of any real regression. Reproduced by
    the bot with a configured-rustc-shim example. Fixed in commit
    `1f58feb2`: `build.rs` now also writes the captured version to
    `OUT_DIR/rustc_version.txt` alongside the `PYCC_BUILD_RUSTC_VERSION`
    env var; `tests/slice0.rs` reads that file via
    `include_str!(concat!(env!("OUT_DIR"), "/rustc_version.txt"))` instead
    of re-invoking `rustc` — `env!("OUT_DIR")` resolves to the same
    build-script output directory for every target in the package,
    including integration tests, so this is still build-time evidence
    through a channel independent of `src/main.rs`'s macro, without
    re-resolving the compiler at test runtime. Replied citing the fix
    commit and reasoning, then resolved.
  - The pinned local `ievo:deep-reviewer` pass ran against the full
    two-commit diff (`79ae2798`, `1f58feb2`) after the bot-review fix
    landed and found the `OUT_DIR`/evidence-file mechanism sound (correct
    Cargo semantics, no staleness risk, reasonable panic-based error paths,
    no security/cross-file/leaked-secret issues), with two findings:
    1. *Doc drift* — `docs/CLI_SPEC.md:39-42` still described the
       first-commit mechanism (checking against a "live `rustc --version`
       at test run time"), stale after commit `1f58feb2` replaced it with
       the `OUT_DIR/rustc_version.txt` evidence-file read. Fixed directly
       in this session before merge.
    2. *Completeness* — no `docs/sessions/` handoff entry existed yet for
       this PR. This file is that entry.
- Local gates run: `cargo build -p pycc` succeeded; manual verification —
  `./target/debug/pycc version --verbose` output matched the host's live
  `rustc --version` byte-for-byte; `cargo test --test slice0 version_` (2/0,
  both before and after the `OUT_DIR`-evidence-file rework); `cargo test
  --workspace` run twice (once per commit) — 0 `FAILED` except the
  pre-existing, unrelated `build_and_run_cross_compiled_to_a_different_tier_1_target`
  linker failure in the `slice0` integration test, consistent with every
  other workspace run this session. Grepped for remaining functional
  (non-comment) uses of `CARGO_PKG_RUST_VERSION` — none found. PR #751's
  full CI matrix (`build`, `build-test-coverage`, `cross-compile-build`,
  `cross-compile-verify`, `governance`, `audit`, `classify-changes`,
  `status-page-freshness`, `frontend-perf-gate`/`-measure`, all four
  `native-build-test` platform legs, `ci-gate`) went green on the initial
  head `79ae2798`; the follow-up commit `1f58feb2`'s CI run is being
  confirmed green before merge (see "Not yet done" below — resume here if
  the second run has not yet completed).
- Branch-currency note: the branch was created from `origin/main` tip
  `ef581ad1` (the #748 merge, itself this session's own prior work) and
  confirmed current via `git merge-base --is-ancestor` before both pushes;
  no rebase or merge was needed.
- Not yet done: confirm PR #751's CI is green on the final head (commit
  `1f58feb2` plus the `docs/CLI_SPEC.md` doc-drift fix and this session-log
  file, once committed and pushed), verify `mergeStateStatus: CLEAN` and
  both review threads resolved, merge PR #751 (merge commit, not
  squash/rebase, matching repo convention), delete branch
  `issue-247-actual-rustc-version`, confirm issue #247 closed.
- Where to resume: this file, plus `git log` on
  `issue-247-actual-rustc-version` — commits `79ae2798` (initial fix),
  `1f58feb2` (`OUT_DIR`-evidence-file rework, bot finding), and whatever
  commit follows carrying the `docs/CLI_SPEC.md` fix and this file
  (pinned-reviewer findings).
- Standing `/goal` continuation: this is the third of an ongoing series of
  small, independently-scoped meddylib gap fixes shipped one-PR-per-fix
  (after #744/PR #746 and #720/PR #748). Further iterations should keep
  selecting small, well-scoped, non-`issue-to-plan`-gated issues from the
  tracker rather than stopping after a small number of merges. Candidates
  already surveyed and rejected as too large for a quick single-PR fix in
  this segment (#150, #618, #704) remain available for later work behind
  an `issue-to-plan` gate or once their blocking dependencies land; #676
  remains available pending an advisor-consulted design decision.
