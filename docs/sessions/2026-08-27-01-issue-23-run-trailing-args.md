# Session handoff: issue #23 — `pycc run PATH -- args` contract implemented

## Status

Issue #23 ("P2: Implement the documented `pycc run PATH -- args` contract"),
implemented against `origin/main` at `d94374da` (re-verified at
implementation start) on a local branch, delivered as this PR. This entry
lands with this PR's merge (D-192). Single-file, mechanically-scoped fix per
AGENTS.md's D-021 preflight step 10 — no `issue-to-plan` pass run, none
needed.

## What this PR delivers

- `src/cli.rs`'s `Command::Run` variant gained an `args: Vec<String>` field
  with `#[arg(trailing_var_arg = true, allow_hyphen_values = true)]`, so
  every value after `--` is captured instead of rejected as an unrecognized
  `pycc` option (the exact repro #23 reports: `pycc run app.py -- hello`
  previously failed with `error: unexpected argument 'hello' found`, exit
  2). Four parser unit tests cover zero, multiple, Unicode, and
  dash-prefixed trailing args.
- `src/main.rs`: `run()` now takes `args: &[String]` and forwards them via a
  new `run_command()` helper (`Command::new(binary).args(args)`) factored out
  specifically so forwarding itself has a direct test against a real child
  process — the Python language surface has no `sys.argv` yet, so a compiled
  program can't observe its own arguments, and the existing e2e tests can
  only prove the CLI parses/executes without error, not that a child
  process actually receives the values. `run_command_tests` (unix-only,
  using `/bin/echo`) proves exact forwarding, order, and the empty-slice
  case.
- `tests/slice0.rs`: four new e2e tests (`run_subcommand_accepts_{zero,
  multiple,unicode,dash_prefixed}_trailing_args*`) proving the documented
  contract now parses and executes successfully for every argument shape
  #23's completion criteria name, including the exact issue repro.
- `docs/CLI_SPEC.md`: added a paragraph documenting the forwarding contract
  (unchanged order, dash-prefixed values pass through literally past `--`,
  omitting `-- args` still runs with no arguments) next to the existing
  `pycc build`/`pycc run` command-table prose, in the same position pattern
  as the file's other per-command explanatory paragraphs.
- Issue #249 (pre-existing `.to_str().expect(...)` panic risk on non-UTF-8
  `TMPDIR` in this same `run()` function) is explicitly untouched, per this
  PR's scope boundary.

## Review

- D-068 pinned local reviewer (`ievo:deep-reviewer`) ran against the full
  diff (four files) before opening the PR. Verdict: 2 findings, both
  doc-placement only (no correctness/security/test findings):
  - `[warning]` a doc comment for `run`'s hardcoded-`false`/`--release`
    explanation had been left attached above the newly inserted
    `run_command` instead of above `run` itself, with `run` left
    undocumented. Fixed by splitting the two doc blocks back onto their
    correct functions.
  - `[note]` the new CLI_SPEC.md forwarding paragraph sat under the `##
    Exit codes` heading, unrelated to the paragraphs around it. Moved next
    to the `pycc run [PATH] [-- args]` command-table prose instead.
  Both fixes applied and re-verified (build, targeted unit/e2e tests,
  `cargo doc`, `cargo fmt --check`) before merge.

- `docs/CLI_SPEC.md` additionally documents, in the same paragraph, that a
  forwarded value must be valid UTF-8 and that a non-UTF-8 value after `--`
  is rejected with a CLI parse error (exit 2) rather than forwarded — added
  after the second review round below, and hand-verified against the built
  binary (`pycc run <path> -- $'\xff'` → `error: invalid UTF-8 was detected
  in one or more arguments`, exit 2).

## Gates (all green at this snapshot, macOS local run unless noted)

- `cargo build --bin pycc`: clean.
- `cargo test --bin pycc`: 65 passed, including the new `cli::tests::run_*`
  parser tests and `run_command_tests::*`.
- `cargo test --test slice0 run_subcommand`: 8 passed, including the four
  new e2e tests.
- `cargo llvm-cov --workspace --fail-under-lines 100 --fail-under-regions
  100`: passed — 100.00% lines/regions/functions across the entire
  workspace, `src/cli.rs` and `src/main.rs` both fully covered by the new
  tests.
- `cargo doc --workspace --no-deps`: green (one pre-existing unrelated
  `pycc_types` private-intra-doc-link warning, not touched by this diff).
- `cargo fmt --check` (scoped to the four changed files): clean.
- CI on PR #823 (head `617528ea`): `ci-gate` and `audit` (the two required
  branch-protection contexts) both green, along with every other job.
  `native-build-test (ubuntu-latest, x86_64-unknown-linux-gnu)` failed once
  on the first CI run on this head with an nbody-benchmark performance-margin
  miss (~18.26x vs. the required 20x speedup) — non-attributable to this
  PR's diff (which touches only `src/cli.rs`, `src/main.rs`, `tests/slice0.rs`,
  `docs/CLI_SPEC.md`, this session file; no codegen/runtime/build-profile
  files), and consistent with this exact benchmark's documented
  runner-load flakiness (`docs/AGENT_RETROSPECTIVE.md`'s prior D-094 entry).
  Reran via `gh run rerun --failed` once the full workflow run reached
  `completed`; it passed cleanly on rerun with zero code changes.

## Second review round: automated Codex PR review

After the D-068 pinned-reviewer round above and before merge, GitHub's
automated `chatgpt-codex-connector` bot (not the repository's D-068
`ievo:deep-reviewer` gate, but still subject to AGENTS.md's "no unresolved
actionable review finding or pull-request thread" merge bar and to branch
protection's required-conversation-resolution setting) left 2 P2 findings,
both on `src/cli.rs`'s `Command::Run::args` field:

- **Require `--` before accepting program arguments.** `trailing_var_arg` +
  `allow_hyphen_values` means `pycc run app.py hello` or `pycc run app.py
  --typo` also succeed today, absorbing the trailing token without a literal
  `--`. Evaluated against #23's completion criteria (which only requires
  that values *after* `--` be captured and forwarded, not that values
  *without* `--` be rejected) and against the fact that `pycc run` has no
  flags of its own yet, so there is nothing a stray value could presently be
  confused with: judged non-actionable for this PR. Filed as item 1 of
  follow-up issue #824 (milestone v0.4) for when `pycc run` gains its own
  flags.
- **Preserve non-UTF-8 process arguments.** `Vec<String>` forces UTF-8
  validation, so a non-UTF-8 argument after `--` is rejected by clap rather
  than forwarded, which is in tension with CLI_SPEC's "forwarded unchanged"
  wording. `String` vs. `OsString` for `args` was an already-settled design
  choice for this PR (#23's own completion criteria name Unicode coverage,
  not raw non-UTF-8 bytes, and a `Vec<OsString>` switch is a distinct,
  separately-testable piece of work under D-014). Resolved instead by
  narrowing the CLI_SPEC wording to state the UTF-8 requirement and the
  rejection behavior explicitly (see above), keeping the documentation
  honest about current behavior. Filed as item 2 of follow-up issue #824
  for the full `OsString` forwarding work.

Both findings replied-to and resolved as GitHub review threads before
merge, per the disposition above. Neither warranted re-running the D-068
pinned reviewer (no correctness/security/test change resulted, only a
one-clause documentation addition already covered by the hand-verification
above).

## Where to resume

Issue #23 is fully closed by this PR. Issue #249 (the pre-existing
non-UTF-8 `TMPDIR` panic in the same `run()` function) remains open and
untouched, tracked separately. Issue #824 (milestone v0.4) tracks the two
Codex-review follow-ups: requiring `--` before trailing args, and forwarding
non-UTF-8 process arguments via `OsString`.
