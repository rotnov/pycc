# 2026-09-02-01: Issue #639 target-dir decision (D-216)

## Status

Complete. Issue #639 ("P3: Decide whether pycc should honor Cargo's
--target-dir flag and config-file build.target-dir") is **CLOSED** with
`stateReason: COMPLETED` by PR #865 (branch commits `4cd73eaa` and
`9d696c45`, squash merge commit `ab9beef32e2c32f1f8e7ee442fd41360f07c076e`
on `origin/main`).

`origin/main` tip immediately before writing this entry: `ab9beef3`
(re-fetched and re-verified right before this commit). This entry is the
only change on top of it.

## What the PR did

- Added `docs/decisions/D-216-close-the-target-dir-flag-and-config-file.md`,
  which answers #639 with "neither": the `--target-dir` command-line flag and
  a `.cargo/config.toml` `build.target-dir` are **permanently not honored**;
  the `CARGO_TARGET_DIR` / `CARGO_BUILD_TARGET_DIR` / manifest-relative
  fallback chain from D-183 is the complete contract. D-216 narrows D-183
  (read side) and D-184 (write side: the build script's stray install into
  `<workspace>/target` under a redirected build is accepted as permanent)
  without superseding either; both earlier ADRs keep their historical #639
  references unedited.
- `docs/CLI_SPEC.md` Environment section and `docs/ROADMAP.md` CLI row now
  cite D-216 instead of "tracked as #639". CLI_SPEC also conditions the
  `no pycc_rt build found` diagnostic on the archive actually being absent
  from the resolved root, since build script and driver resolve the same
  fallback root and therefore agree on where the archive is.
- `crates/pycc_artifact_layout/src/lib.rs`: doc-comment-only D-216 citation
  on `resolve_cargo_target_root`. No behavioral code changed.
- `docs/decisions/README.md` regenerated.
- `tests/issue_630_pycc_rt_build_dependency.rs`: module-doc paragraph only
  (why the excluded inputs are not exercised as processes); no test logic
  changed.

## Decisions and measurements made along the way

- Codex's automatic review on PR #865 left three threads (required
  conversation resolution blocks merge while they are open, which is not
  the empty-rollup `BLOCKED` artifact -- check `reviewThreads` before
  assuming that artifact). Resolved in the PR's second commit `9d696c45`:
  (1) D-216 now evaluates and rejects deriving the target root from a
  build script's `OUT_DIR` and embedding it with `cargo::rustc-env` (the
  shared resolver lives in `pycc_artifact_layout`, which has no build
  script, so every consumer would need its own anchor; unstable layout;
  frozen absolute path regressing `cargo install`). (2) CLI_SPEC scopes
  the "`cargo build -p pycc_rt` is not sufficient" caveat to a persistent
  config-file redirect and gives the `CARGO_TARGET_DIR` recoveries;
  `CARGO_TARGET_DIR` outranking a config-file `build.target-dir` was
  measured under cargo 1.88.0 and 1.97.1. Rewording the emitted diagnostic
  itself is a behavior change and was split out as **#869** (v0.4).
  (3) A P1 asking for end-to-end coverage of the excluded inputs was
  declined as a blocker: the positive half cannot fail on CI (workspace
  build first, coverage-job symlink) and the negative half would pin the
  absence of a feature; `tests/issue_630_pycc_rt_build_dependency.rs`'s
  module doc now records that reasoning.
- Nine iEvo deep-review rounds in total (seven before the PR, two after
  the Codex-thread edits; round 8 caught a wrong `cargo::rustc-env`
  propagation claim in the new Alternatives bullet). The substantive
  corrections from the first seven: (1) D-216
  originally claimed the `CARGO_BUILD_TARGET_DIR` mapping "covers the
  config-file case indirectly", contradicting D-183's own measured statement
  that it does *not*; the claim was removed and the coverage-cost argument
  stands alone. (2) The `CARGO_TARGET_TMPDIR` availability negatives for a
  build script and a library's `#[cfg(test)]` unit-test binary were
  *measured* in a throwaway package outside the tree, under both the
  ambient cargo 1.88.0 and the pinned 1.97.1 (identical results:
  `Some("<dir>/tmp")` only in the integration-test binary), rather than
  reasoned out -- D-183's recorded lesson applied again. Of the four
  `resolve_cargo_target_root` call sites only `tests/slice0.rs` can observe
  the macro.
- The llms.txt 264 KiB aggregate budget (`site/llms-txt-context-manifest.json`,
  enforced by `scripts/check-site.sh`) had **7 bytes** of headroom on
  origin/main `afc0c13b`. The original ROADMAP wording (+99 bytes) breached
  it; the CLI-row phrase was condensed to a net -2 bytes instead of raising
  the budget again (D-200 precedent). See "Known follow-ups".

## Known follow-ups

- **llms.txt aggregate budget is effectively exhausted.** `docs/ROADMAP.md`
  is the dominant non-optional document (181 KB of the 264 KiB ceiling) and
  any future ROADMAP growth beyond a handful of bytes will trip
  `scripts/check-site.sh`. The next PR that needs ROADMAP prose should
  expect to either condense elsewhere or raise the budget through a
  D-200-style decision; the gate has now bound on 2026-08-24, 08-26, 08-29
  and 09-02 (see the `docs/AGENT_RETROSPECTIVE.md` entries for those dates
  and D-200).
- `ruby scripts/check_roadmap_evidence.rb` aborts with "invalid byte sequence
  in US-ASCII" when run from a shell without a UTF-8 locale; run it with
  `LC_ALL=en_US.UTF-8` (CI sets a UTF-8 locale, so this is local-only).

## Where to resume

- Nothing in flight for #639. Next work comes from `issue-select` over the
  open tracker; D-216 is the pointer for anyone tempted to reopen the
  target-dir question.
