# Session handoff: #824 tighten `pycc run` trailing-args parsing

## Status: PR opened, not yet merged

This session selected, implemented, and pushed the fix for
[#824](https://github.com/rotnov/pycc/issues/824) ("tighten `pycc run`
trailing-args parsing (require --, forward non-UTF-8 args)"), then opened
a pull request. Per this task's own division of labor, this session does
**not** watch CI or merge — a separate (parent) session does that. This
file is committed in the PR branch itself; by the time `main` shows this
file, the PR that added it has already merged, so its own number/head
commit/CI run are the authoritative record (see `gh pr view` against the
branch below).

## Selection (`issue-select`)

- Baseline: fetched `origin/main`, which had already advanced to
  `248d47eb` (merge of PR #841, issue #249) since this task's dispatch
  context was written. Started a fresh branch off that exact commit.
- Zero open pull requests at selection time -- no collision screen needed.
- D-192 non-milestone ceiling: 72 open issues carry no milestone against a
  cap of 20 -- deeply breached, so no new non-milestone issue was filed
  this run, and any milestone-assigned candidate is exempt from the 4:1
  merge quota (this pick is milestone-assigned, so the quota was not
  computed).
- Milestone scope: the sole active GitHub milestone is `v0.4` (39 open
  issues), cross-checked against `docs/ROADMAP.md`'s v0.4 section, which
  carries no "met" evidence -- confirmed in scope.
- Candidates considered inside v0.4, newest-to-oldest by priority marker:
  - **#20** (P1, oldest, 7 comments) -- "Make `pycc_rt` a real build/link
    dependency and honor Cargo artifact paths." Superseded/narrowed to
    sub-issue #631, itself explicitly D-103/D-080-gated and deprioritized
    per its own latest comment. Deliberately passed over as
    deprioritized-not-excluded CI-workflow-adjacent work, not silently
    skipped.
  - **#24** (P2) -- "Add a rustfmt CI gate." Requires editing
    `.github/workflows/ci.yml`, D-080/D-103-gated. Deprioritized.
  - **#636** (P2) -- "Balance D-182's tuple-literal ingress retain."
    Explicitly blocked: its own body says "Do not start this before D-124's
    model lands." Excluded by the blocker screen.
  - **#834** (P2, newest, 0 comments) -- bigint refcounting defect
    (`retain_if_int_duplicate`'s extra reference not tracked for release
    on the exception edge) spanning ~8 call sites in
    `crates/pycc_codegen/src/{bigint_rc,lib}.rs`. Larger, riskier scope
    than #824; a legitimate runner-up, not selected.
  - **#714** (P2) -- binding a user-defined exception subclass as a value
    aborts at runtime with a `NameError` on `Exception.__init__`. Root
    cause lives in `crates/pycc_hir/src/exception.rs`; a viable secondary
    candidate needing deeper HIR/codegen familiarity than #824. Not
    selected.
  - **#414, #585, #693, #707, #733** (remaining P2 v0.4 peers) -- not
    individually re-verified against the current tree this round; #824's
    combination of small blast radius, zero prior comments, and a premise
    directly verifiable by reading the exact current source made it the
    strongest pick without exhausting the full peer set line-by-line. The
    advisor round (below) treated this as an accepted tradeoff rather than
    re-litigating it.
- **Selected: #824** -- "tighten `pycc run` trailing-args parsing (require
  --, forward non-UTF-8 args)." v0.4-scoped, unblocked, no open-PR
  collision, not a CI-workflow change, zero prior comments (fresh), and
  its premise was verified directly against `src/cli.rs` (the
  `trailing_var_arg`/`allow_hyphen_values` `Vec<String>` field) and
  `src/main.rs` (the `run`/`run_command`/`run_built_binary` chain, all
  `&[String]`-typed) before starting work.
- Adversarial advisor round: consulted this session's advisor tool with
  the full candidate set and reasoning above. The advisor confirmed the
  pick was sound, flagged two things acted on directly (see below): that
  `Fixes #824` would falsely close the issue's Item 1 checkbox unless that
  decision was recorded as a durable artifact, and that the D-014 gate
  requires accounting for the Windows leg of the new `OsString`-based
  non-UTF-8 test coverage (addressed by `#[cfg(unix)]`-gating the new
  non-UTF-8 tests exactly like the existing #249 precedent already does
  for `PathBuf`).

## What changed

- `src/cli.rs`: `Command::Run`'s `args` field changed from `Vec<String>`
  to `Vec<OsString>`. clap 4 does not UTF-8-validate an `OsString`
  positional, so a non-UTF-8 forwarded value (a byte sequence a shell can
  still pass through `$@`) is now captured and forwarded losslessly,
  matching the `PathBuf` treatment `PATH`/`OUT` already get (#249),
  instead of failing to parse. The doc comment on the `#[arg(...)]`
  attribute records both #824 decisions: item 2 (implemented, `OsString`)
  and item 1 (`--` requirement, **deliberately not implemented** -- `pycc
  run` has no flags today, so there is no `pycc`-recognized flag a bare
  trailing value could be mistaken for, and `allow_hyphen_values` already
  guarantees dash-prefixed values are captured either way; revisit if
  `run` ever gains a flag).
- `src/main.rs`: `run_command`, `run`, and `run_built_binary`'s `args`
  parameters changed from `&[String]` to `&[std::ffi::OsString]`;
  `std::process::Command::args` accepts `OsStr` directly, so no
  re-validation or lossy conversion was needed anywhere downstream.
- Tests (`src/cli.rs`): existing `run_captures_multiple_trailing_args_in_order`,
  `run_captures_unicode_trailing_args_unchanged`, and
  `run_captures_dash_prefixed_trailing_args_without_treating_them_as_pycc_options`
  updated for the `OsString` element type. Added
  `run_captures_a_trailing_arg_without_requiring_an_explicit_separator`
  (verifies the item-1 "no `--` required today" claim empirically -- this
  is what the CLI_SPEC.md wording below is grounded in, not an assumption)
  and `#[cfg(unix)] run_captures_non_utf8_trailing_args_as_opaque_bytes`
  (parser-level non-UTF-8 proof, mirroring #249's
  `run_path_preserves_non_utf8_bytes` pattern).
- Tests (`src/main.rs`): `run_command_tests` updated for `Vec<OsString>`;
  added `forwards_non_utf8_args_as_opaque_bytes_to_the_child_process`
  (forwards a non-UTF-8 arg through a real `/bin/echo` child process,
  proving the byte sequence survives end-to-end, not just at the parser).
- `docs/CLI_SPEC.md`: rewrote the trailing-args paragraph -- `pycc` now
  parses forwarded arguments as `OsString` (matching `PATH`/`OUT`'s
  `PathBuf` treatment) instead of `String`, so non-UTF-8 values are
  forwarded rather than rejected; also documents that `--` is not
  required before a trailing value today, and names the future condition
  under which that would change (`run` gaining its own flag).
- `docs/ROADMAP.md`: deliberately **not** changed -- verified explicitly
  rather than assumed. #824 is a narrow bugfix within already-shipped
  `run` CLI surface (same category the immediately preceding #249 session
  handoff recorded as not needing a ROADMAP update), and
  `docs/ROADMAP.md`'s own byte size is currently exactly at the
  `llms-txt-context-manifest.json` aggregate ceiling (270336 bytes
  total across all non-optional llms.txt documents, per the #249 PR's own
  trim commit) -- any net-positive edit would need an equal offsetting
  trim elsewhere, which is disproportionate scope for this fix. The
  normative behavior contract for this change lives in `docs/CLI_SPEC.md`,
  which was updated.

## Local gates (this session)

- `cargo build --bin pycc`: clean.
- `cargo test --bin pycc`: 75 passed, 0 failed (includes all `cli::tests`
  and `run_command_tests`/`run_built_binary_tests`).
- `cargo clippy --bin pycc --all-targets -- -D warnings`: clean (no new
  warnings; three pre-existing unrelated `slice1_codegen_depth.rs`
  escaped-newline warnings do not fail the `-D warnings` build and are not
  from this diff).
- `cargo llvm-cov --workspace --fail-under-lines 100 --fail-under-regions
  100`: **passed**, 1462 tests, 100.00% lines/regions across the entire
  workspace including `src/cli.rs` (100.00%/100.00%) and `src/main.rs`
  (100.00%/100.00%).
- `cargo doc --workspace --no-deps`: clean (one pre-existing unrelated
  `pycc_types::env::bind_class` private-intra-doc-link warning, not
  touched by this diff).

## Follow-ups / known non-issues

- Peers #414, #585, #693, #707, #733 were not individually re-verified
  against the current tree this round (see Selection above) -- if a
  future `issue-select` pass reaches them, they should get their own
  fresh premise check rather than relying on this session's pass-over.
- #834 and #714 remain open, larger-scope P2 v0.4 candidates for a future
  session; neither is blocked.
- #20/#631 (D-103/D-080-gated) and #24 (same gating) remain deprioritized,
  not excluded, exactly as `issue-select`'s own policy describes -- worth
  deliberate attention from a session that specifically wants to work the
  staged CI-workflow digest process.
- #636 stays blocked on D-124 landing; do not start it before that.
